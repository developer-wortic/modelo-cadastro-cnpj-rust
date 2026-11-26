//! A conversa com a API aberta da Wortic (<https://cnpj.wortic.com>), com guarda em memória.
//! Sem chave valem 60 consultas por minuto e 2.000 por dia, por IP. Com `WORTIC_API_KEY` vale o limite do seu plano.
//! Os erros 402, 403 e 429 saem com texto claro, tempo de espera e link dos planos. Os outros passam como vieram.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BASE: &str = "https://cnpj.wortic.com";
const TEMPO: Duration = Duration::from_secs(10);
const GUARDA: Duration = Duration::from_hours(24);
/// Quantas respostas ficam guardadas. Passou disso, sai a mais antiga.
const CACHE_MAX: usize = 5000;
/// Onde se assina o plano e se pega a chave.
pub const PLANOS: &str = "https://cnpj.wortic.com/#planos";
const SEM_REDE: &str =
    r#"{"erro":{"codigo":"sem_rede","mensagem":"Não consegui falar com a API agora."}}"#;
const RESPOSTA_INVALIDA: &str = r#"{"erro":{"codigo":"resposta_invalida","mensagem":"A API respondeu algo que não consegui ler."}}"#;
const MSG_LIMITE_GRATIS: &str = "Passou do limite grátis da API: 60 consultas por minuto ou 2.000 por dia, por IP. Espere o tempo indicado ou assine um plano.";
const MSG_PLANO_PRO: &str =
    "Esta função é do plano Pro. Para usar, defina WORTIC_API_KEY com a chave do plano.";
const MSG_BANIDO: &str = "Este IP foi banido da API aberta por insistir acima do limite. Para voltar a usar, assine um plano e defina WORTIC_API_KEY.";
const MSG_SEM_PLANO_PRO: &str =
    "A chave usada não tem o plano Pro. Confira os planos e troque a chave.";
const MSG_BLOQUEADO: &str =
    "IP bloqueado por insistir acima do limite. Não insista antes do tempo indicado.";
const MSG_LIMITE_MINUTO: &str = "Muitas consultas em pouco tempo. Espere o tempo indicado.";
const MSG_LIMITE_DIA: &str = "Passou do limite diário do plano grátis. Espere o tempo indicado.";
const MSG_PLANO_ACABOU: &str =
    "O limite do seu plano acabou. Espere o tempo indicado ou veja os planos.";

/// O que voltou da API: o código HTTP, o corpo, no erro de limite quantos segundos esperar,
/// e o mês da base (cabeçalho `x-base-cnpj`), quando veio.
pub struct Resposta {
    pub status: u16,
    pub corpo: String,
    pub espere: Option<String>,
    pub base: Option<String>,
}

impl Resposta {
    /// Uma resposta 200 pronta, sem espera nem base.
    pub fn ok(corpo: String) -> Self {
        Resposta {
            status: 200,
            corpo,
            espere: None,
            base: None,
        }
    }
}

struct Guardado {
    quando: Instant,
    corpo: String,
    base: Option<String>,
}

pub struct Api {
    base: String,
    chave: Option<String>,
    tempo: Duration,
    guarda_por: Duration,
    guarda_max: usize,
    guarda: Mutex<HashMap<String, Guardado>>,
}

/// A chave vazia conta como sem chave.
pub fn chave_de(valor: Option<String>) -> Option<String> {
    valor.filter(|c| !c.is_empty())
}

/// A base da API. `CADASTRO_API_BASE` troca a API aberta por outro endereço (um espelho ou um servidor de teste).
pub fn base_de(valor: Option<String>) -> String {
    valor
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| BASE.to_string())
}

/// O tamanho da guarda: `CADASTRO_CACHE_MAX`. Sem ele, ou se não for um número maior que zero, vale 5000.
pub fn max_de(valor: Option<String>) -> usize {
    valor
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(CACHE_MAX)
}

impl Api {
    /// A API com o que vem do ambiente: base, chave e tamanho da guarda.
    pub fn nova() -> Self {
        Self::com(
            &base_de(std::env::var("CADASTRO_API_BASE").ok()),
            chave_de(std::env::var("WORTIC_API_KEY").ok()),
            TEMPO,
            GUARDA,
        )
        .com_cache(max_de(std::env::var("CADASTRO_CACHE_MAX").ok()))
    }

    /// Uma API com base, chave, tempo limite e tempo da guarda escolhidos. Os testes usam isso.
    pub fn com(base: &str, chave: Option<String>, tempo: Duration, guarda_por: Duration) -> Self {
        Api {
            base: base.to_string(),
            chave,
            tempo,
            guarda_por,
            guarda_max: CACHE_MAX,
            guarda: Mutex::new(HashMap::new()),
        }
    }

    /// Muda quantas respostas a guarda aceita (pelo menos uma).
    pub fn com_cache(mut self, entradas: usize) -> Self {
        self.guarda_max = entradas.max(1);
        self
    }

    /// Diz só se há chave. A chave nunca sai daqui.
    pub fn tem_chave(&self) -> bool {
        self.chave.is_some()
    }

    /// GET no caminho pedido. Resposta 200 fica guardada: a segunda consulta nem sai daqui.
    pub fn buscar(&self, caminho: &str) -> Resposta {
        if let Some(g) = self.guarda.lock().unwrap().get(caminho)
            && g.quando.elapsed() < self.guarda_por
        {
            return Resposta {
                status: 200,
                corpo: g.corpo.clone(),
                espere: None,
                base: g.base.clone(),
            };
        }
        let mut pedido = ureq::get(&format!("{}{caminho}", self.base)).timeout(self.tempo);
        if let Some(chave) = &self.chave {
            pedido = pedido.set("x-api-key", chave);
        }
        match pedido.call() {
            Ok(r) => {
                let base = base_valida(r.header("x-base-cnpj"));
                match r.into_string() {
                    Ok(corpo) => {
                        self.guardar(caminho, &corpo, base.as_deref());
                        Resposta {
                            status: 200,
                            corpo,
                            espere: None,
                            base,
                        }
                    }
                    // corpo grande demais para ler (mais de 10 MB): 502, e nada fica guardado
                    Err(_) => Resposta {
                        status: 502,
                        corpo: RESPOSTA_INVALIDA.to_string(),
                        espere: None,
                        base: None,
                    },
                }
            }
            Err(ureq::Error::Status(status, r)) => {
                let base = base_valida(r.header("x-base-cnpj"));
                let espere = so_digitos(r.header("retry-after"));
                let corpo = r.into_string().unwrap_or_default();
                traduzir(status, corpo, espere, base)
            }
            Err(_) => Resposta {
                status: 502,
                corpo: SEM_REDE.to_string(),
                espere: None,
                base: None,
            },
        }
    }

    /// Rota do plano Pro. Sem chave, não sai daqui: responde 402 `plano_pro` com o link dos planos.
    pub fn pro(&self, caminho: &str) -> Resposta {
        if self.chave.is_none() {
            return Resposta {
                status: 402,
                corpo: erro_json("plano_pro", MSG_PLANO_PRO, None, Some(PLANOS)),
                espere: None,
                base: None,
            };
        }
        self.buscar(caminho)
    }

    /// Guarda a resposta. Se a guarda está cheia e a chave é nova, sai a entrada mais antiga.
    fn guardar(&self, caminho: &str, corpo: &str, base: Option<&str>) {
        let mut g = self.guarda.lock().unwrap();
        if !g.contains_key(caminho)
            && g.len() >= self.guarda_max
            && let Some(antiga) = g
                .iter()
                .min_by_key(|(_, v)| v.quando)
                .map(|(k, _)| k.clone())
        {
            g.remove(&antiga);
        }
        g.insert(
            caminho.to_string(),
            Guardado {
                quando: Instant::now(),
                corpo: corpo.to_string(),
                base: base.map(str::to_string),
            },
        );
    }
}

/// O mês da base só passa se for curto e com dígitos e hífen (ex.: 2026-10). Cabeçalho estranho não chega à tela.
fn base_valida(valor: Option<&str>) -> Option<String> {
    valor
        .filter(|v| {
            (1..=10).contains(&v.len()) && v.chars().all(|c| c.is_ascii_digit() || c == '-')
        })
        .map(str::to_string)
}

/// Só passa o tempo de espera se for número. Vai ao cabeçalho `retry-after`, então não pode ter lixo.
fn so_digitos(valor: Option<&str>) -> Option<String> {
    valor
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

/// O código de erro do corpo da API (`"codigo":"..."`), só se for minúsculas e `_`.
fn codigo_do_corpo(corpo: &str) -> Option<String> {
    let (_, resto) = corpo.split_once("\"codigo\":\"")?;
    let (codigo, _) = resto.split_once('"')?;
    let bom = !codigo.is_empty() && codigo.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    bom.then(|| codigo.to_string())
}

/// O erro no formato que a tela lê: código, mensagem, segundos de espera (ou null) e link (ou null).
fn erro_json(codigo: &str, mensagem: &str, espere: Option<&str>, link: Option<&str>) -> String {
    let espere = espere.unwrap_or("null");
    let link = link.map_or_else(|| "null".to_string(), |l| format!("\"{l}\""));
    format!(
        r#"{{"erro":{{"codigo":"{codigo}","mensagem":"{mensagem}","espere":{espere},"link":{link}}}}}"#
    )
}

/// Dá texto claro aos erros 402, 403 e 429. Os outros códigos passam como vieram.
fn traduzir(status: u16, corpo: String, espere: Option<String>, base: Option<String>) -> Resposta {
    let lido = codigo_do_corpo(&corpo).unwrap_or_default();
    let (codigo, mensagem, com_planos) = match (status, lido.as_str()) {
        (402, "plano_pro") => ("plano_pro", MSG_PLANO_PRO, true),
        (402, _) => ("limite_gratis", MSG_LIMITE_GRATIS, true),
        (403, "sem_plano_pro") => ("sem_plano_pro", MSG_SEM_PLANO_PRO, true),
        (403, _) => ("banido", MSG_BANIDO, true),
        (429, "bloqueado") => ("bloqueado", MSG_BLOQUEADO, false),
        (429, "limite_dia") => ("limite_dia", MSG_LIMITE_DIA, false),
        (429, "limite_do_plano") => ("limite_do_plano", MSG_PLANO_ACABOU, false),
        (429, _) => ("limite", MSG_LIMITE_MINUTO, false),
        _ => {
            return Resposta {
                status,
                corpo,
                espere: None,
                base,
            };
        }
    };
    let corpo = erro_json(
        codigo,
        mensagem,
        espere.as_deref(),
        com_planos.then_some(PLANOS),
    );
    Resposta {
        status,
        corpo,
        espere,
        base,
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::suporte::falso;
    use std::net::TcpListener;
    use std::thread;

    fn api_para(base: &str) -> Api {
        Api::com(base, None, Duration::from_secs(5), GUARDA)
    }

    #[test]
    fn chave_vazia_conta_como_sem_chave() {
        assert_eq!(chave_de(None), None);
        assert_eq!(chave_de(Some(String::new())), None);
        assert_eq!(chave_de(Some("k".to_string())), Some("k".to_string()));
    }

    #[test]
    fn base_de_padrao_e_o_override_sem_barra_final() {
        assert_eq!(base_de(None), BASE);
        assert_eq!(base_de(Some(String::new())), BASE);
        assert_eq!(base_de(Some("  ".to_string())), BASE);
        assert_eq!(
            base_de(Some(" http://127.0.0.1:9/ ".to_string())),
            "http://127.0.0.1:9"
        );
    }

    #[test]
    fn max_de_padrao_e_so_aceita_numero_positivo() {
        assert_eq!(max_de(None), 5000);
        assert_eq!(max_de(Some("12".to_string())), 12);
        assert_eq!(max_de(Some(" 7 ".to_string())), 7);
        assert_eq!(max_de(Some("0".to_string())), 5000);
        assert_eq!(max_de(Some("abc".to_string())), 5000);
        assert_eq!(max_de(Some("-3".to_string())), 5000);
    }

    #[test]
    fn nova_aponta_para_a_api_aberta_com_os_tempos_padrao() {
        let api = Api::nova();
        assert_eq!(api.base, BASE);
        assert_eq!(api.tempo, TEMPO);
        assert_eq!(api.guarda_por, GUARDA);
        assert_eq!(api.guarda_max, CACHE_MAX);
    }

    #[test]
    fn tem_chave_so_diz_se_ha_e_nunca_mostra_a_chave() {
        assert!(!api_para("http://x").tem_chave());
        let com = Api::com(
            "http://x",
            Some("segredo".to_string()),
            Duration::from_secs(5),
            GUARDA,
        );
        assert!(com.tem_chave());
    }

    #[test]
    fn sucesso_devolve_o_corpo_e_nao_manda_chave_sem_ela() {
        let f = falso(|_| (200, b"{\"ok\":1}".to_vec(), vec![]));
        let r = api_para(&f.base).buscar("/cnpj/37853407000171");
        assert_eq!(r.status, 200);
        assert_eq!(r.corpo, "{\"ok\":1}");
        assert_eq!(r.espere, None);
        assert_eq!(r.base, None);
        let pedidos = f.pedidos.lock().unwrap();
        assert_eq!(pedidos[0], ("/cnpj/37853407000171".to_string(), None));
    }

    #[test]
    fn mes_da_base_do_cabecalho_chega_e_fica_na_guarda() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![("x-base-cnpj", "2026-10")]));
        let api = api_para(&f.base);
        assert_eq!(api.buscar("/cnpj/1").base, Some("2026-10".to_string()));
        // a segunda sai da guarda e leva a mesma base
        assert_eq!(api.buscar("/cnpj/1").base, Some("2026-10".to_string()));
        assert_eq!(f.pedidos.lock().unwrap().len(), 1);
    }

    #[test]
    fn base_que_nao_e_mes_nao_passa() {
        assert_eq!(base_valida(None), None);
        assert_eq!(base_valida(Some("")), None);
        assert_eq!(base_valida(Some("a b")), None);
        assert_eq!(base_valida(Some("1234567890123")), None);
        assert_eq!(base_valida(Some("2026-10")), Some("2026-10".to_string()));
    }

    #[test]
    fn retry_after_que_nao_e_numero_nao_vai_adiante() {
        assert_eq!(so_digitos(None), None);
        assert_eq!(so_digitos(Some("")), None);
        assert_eq!(so_digitos(Some("abc")), None);
        assert_eq!(so_digitos(Some("45")), Some("45".to_string()));
    }

    #[test]
    fn codigo_do_corpo_so_aceita_minusculas_e_underscore() {
        assert_eq!(
            codigo_do_corpo("{\"erro\":{\"codigo\":\"bloqueado\"}}"),
            Some("bloqueado".to_string())
        );
        assert_eq!(
            codigo_do_corpo("{\"erro\":{\"codigo\":\"sem_plano_pro\"}}"),
            Some("sem_plano_pro".to_string())
        );
        assert_eq!(codigo_do_corpo("sem código nenhum"), None);
        assert_eq!(codigo_do_corpo("{\"codigo\":\"\"}"), None);
        assert_eq!(codigo_do_corpo("{\"codigo\":\"ABC\"}"), None);
        assert_eq!(codigo_do_corpo("{\"codigo\":\"a1\"}"), None);
        assert_eq!(codigo_do_corpo("{\"codigo\":\"sem fim"), None);
    }

    #[test]
    fn com_chave_manda_o_cabecalho_x_api_key() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let api = Api::com(
            &f.base,
            Some("chave-de-teste".to_string()),
            Duration::from_secs(5),
            GUARDA,
        );
        api.buscar("/empresas?nome=padaria");
        let pedidos = f.pedidos.lock().unwrap();
        assert_eq!(
            pedidos[0],
            (
                "/empresas?nome=padaria".to_string(),
                Some("chave-de-teste".to_string())
            )
        );
    }

    #[test]
    fn segunda_consulta_do_mesmo_caminho_sai_da_guarda() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let api = api_para(&f.base);
        api.buscar("/cnpj/1");
        let segunda = api.buscar("/cnpj/1");
        assert_eq!(segunda.status, 200);
        assert_eq!(segunda.corpo, "{}");
        api.buscar("/cnpj/2");
        assert_eq!(f.pedidos.lock().unwrap().len(), 2);
    }

    #[test]
    fn guarda_vencida_consulta_a_api_de_novo() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let api = Api::com(&f.base, None, Duration::from_secs(5), Duration::ZERO);
        api.buscar("/cnpj/1");
        api.buscar("/cnpj/1");
        assert_eq!(f.pedidos.lock().unwrap().len(), 2);
    }

    #[test]
    fn guarda_tem_teto_e_sai_a_entrada_mais_antiga() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let api = api_para(&f.base).com_cache(2);
        api.buscar("/cnpj/a");
        api.buscar("/cnpj/b");
        api.buscar("/cnpj/c"); // cheia: sai a "a", a mais antiga
        assert_eq!(f.pedidos.lock().unwrap().len(), 3);
        // "b" e "c" continuam na guarda
        api.buscar("/cnpj/b");
        api.buscar("/cnpj/c");
        assert_eq!(f.pedidos.lock().unwrap().len(), 3);
        // "a" saiu: consulta a API de novo, e aí "b" é a mais antiga
        api.buscar("/cnpj/a");
        assert_eq!(f.pedidos.lock().unwrap().len(), 4);
        api.buscar("/cnpj/c");
        assert_eq!(f.pedidos.lock().unwrap().len(), 4);
    }

    #[test]
    fn guarda_com_teto_zero_guarda_pelo_menos_uma() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let api = api_para(&f.base).com_cache(0);
        api.buscar("/cnpj/a");
        api.buscar("/cnpj/a");
        assert_eq!(f.pedidos.lock().unwrap().len(), 1);
        api.buscar("/cnpj/b");
        api.buscar("/cnpj/a");
        assert_eq!(f.pedidos.lock().unwrap().len(), 3);
    }

    #[test]
    fn erro_404_passa_com_o_corpo_e_nao_fica_guardado() {
        let f = falso(|_| (404, b"{\"erro\":\"nao achei\"}".to_vec(), vec![]));
        let api = api_para(&f.base);
        let r = api.buscar("/cnpj/1");
        assert_eq!(r.status, 404);
        assert_eq!(r.corpo, "{\"erro\":\"nao achei\"}");
        assert_eq!(r.espere, None);
        api.buscar("/cnpj/1");
        assert_eq!(f.pedidos.lock().unwrap().len(), 2);
    }

    #[test]
    fn erro_429_com_retry_after_leva_os_segundos_e_texto_claro() {
        let f = falso(|_| (429, b"muitas".to_vec(), vec![("retry-after", "30")]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 429);
        assert_eq!(r.espere, Some("30".to_string()));
        assert!(r.corpo.contains("\"codigo\":\"limite\""));
        assert!(r.corpo.contains("\"espere\":30"));
        assert!(r.corpo.contains("Muitas consultas"));
        assert!(r.corpo.contains("\"link\":null"));
    }

    #[test]
    fn erro_429_sem_retry_after_nao_inventa_espera() {
        let f = falso(|_| (429, b"muitas".to_vec(), vec![]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 429);
        assert_eq!(r.espere, None);
        assert!(r.corpo.contains("\"espere\":null"));
    }

    #[test]
    fn retry_after_com_lixo_nao_vai_ao_cabecalho() {
        let f = falso(|_| (429, b"muitas".to_vec(), vec![("retry-after", "abc")]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 429);
        assert_eq!(r.espere, None);
        assert!(r.corpo.contains("\"espere\":null"));
    }

    #[test]
    fn erro_429_bloqueado_diz_que_o_ip_esta_bloqueado() {
        let f = falso(|_| {
            (
                429,
                b"{\"erro\":{\"codigo\":\"bloqueado\",\"mensagem\":\"x\"}}".to_vec(),
                vec![("retry-after", "3500")],
            )
        });
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 429);
        assert_eq!(r.espere, Some("3500".to_string()));
        assert!(r.corpo.contains("\"codigo\":\"bloqueado\""));
        assert!(r.corpo.contains("IP bloqueado"));
    }

    #[test]
    fn erro_429_de_dia_e_de_plano_tem_o_seu_texto() {
        let f = falso(|caminho| {
            let codigo = if caminho == "/dia" {
                "limite_dia"
            } else {
                "limite_do_plano"
            };
            (
                429,
                format!("{{\"erro\":{{\"codigo\":\"{codigo}\",\"mensagem\":\"x\"}}}}").into_bytes(),
                vec![],
            )
        });
        let api = api_para(&f.base);
        let dia = api.buscar("/dia");
        assert!(dia.corpo.contains("\"codigo\":\"limite_dia\""));
        assert!(dia.corpo.contains("limite diário"));
        let plano = api.buscar("/plano");
        assert!(plano.corpo.contains("\"codigo\":\"limite_do_plano\""));
        assert!(plano.corpo.contains("limite do seu plano"));
    }

    #[test]
    fn erro_402_vira_limite_gratis_com_espera_e_link() {
        let f = falso(|_| {
            (
                402,
                b"{\"erro\":{\"codigo\":\"limite_gratis\",\"mensagem\":\"x\"},\"planos\":[]}"
                    .to_vec(),
                vec![("retry-after", "37")],
            )
        });
        let api = api_para(&f.base);
        let r = api.buscar("/cnpj/1");
        assert_eq!(r.status, 402);
        assert_eq!(r.espere, Some("37".to_string()));
        assert!(r.corpo.contains("\"codigo\":\"limite_gratis\""));
        assert!(r.corpo.contains("\"espere\":37"));
        assert!(
            r.corpo
                .contains("\"link\":\"https://cnpj.wortic.com/#planos\"")
        );
        // o 402 não fica guardado: a próxima consulta sai
        api.buscar("/cnpj/1");
        assert_eq!(f.pedidos.lock().unwrap().len(), 2);
    }

    #[test]
    fn erro_402_sem_corpo_conhecido_ainda_e_limite_gratis() {
        let f = falso(|_| (402, b"??".to_vec(), vec![]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 402);
        assert_eq!(r.espere, None);
        assert!(r.corpo.contains("\"codigo\":\"limite_gratis\""));
        assert!(r.corpo.contains("\"espere\":null"));
    }

    #[test]
    fn erro_402_plano_pro_do_upstream_mantem_o_codigo() {
        let f = falso(|_| {
            (
                402,
                b"{\"erro\":{\"codigo\":\"plano_pro\",\"mensagem\":\"x\"}}".to_vec(),
                vec![],
            )
        });
        let r = api_para(&f.base).buscar("/v1/empresas/busca");
        assert!(r.corpo.contains("\"codigo\":\"plano_pro\""));
        assert!(r.corpo.contains("é do plano Pro"));
    }

    #[test]
    fn erro_403_banido_explica_e_mostra_o_link() {
        let f = falso(|_| {
            (
                403,
                b"{\"erro\":{\"codigo\":\"banido\",\"mensagem\":\"x\"}}".to_vec(),
                vec![],
            )
        });
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 403);
        assert!(r.corpo.contains("\"codigo\":\"banido\""));
        assert!(r.corpo.contains("foi banido"));
        assert!(
            r.corpo
                .contains("\"link\":\"https://cnpj.wortic.com/#planos\"")
        );
    }

    #[test]
    fn erro_403_sem_codigo_conhecido_vira_banido() {
        let f = falso(|_| (403, b"nope".to_vec(), vec![]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert!(r.corpo.contains("\"codigo\":\"banido\""));
    }

    #[test]
    fn erro_403_sem_plano_pro_diz_da_chave() {
        let f = falso(|_| {
            (
                403,
                b"{\"erro\":{\"codigo\":\"sem_plano_pro\",\"mensagem\":\"x\"}}".to_vec(),
                vec![],
            )
        });
        let r = api_para(&f.base).buscar("/v1/empresas/busca");
        assert!(r.corpo.contains("\"codigo\":\"sem_plano_pro\""));
        assert!(r.corpo.contains("não tem o plano Pro"));
    }

    #[test]
    fn erro_5xx_passa_o_status_da_api() {
        let f = falso(|_| (503, b"manutencao".to_vec(), vec![]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 503);
        assert_eq!(r.corpo, "manutencao");
    }

    #[test]
    fn corpo_que_nao_e_utf8_sai_com_caractere_de_substituicao() {
        // o ureq lê o corpo com troca de caractere inválido: não é erro para o modelo
        let f = falso(|_| (200, vec![b'{', 0xff, b'}'], vec![]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 200);
        assert_eq!(r.corpo, "{\u{FFFD}}");
    }

    #[test]
    fn corpo_acima_de_10_mb_vira_502_e_nao_fica_guardado() {
        let f = falso(|_| (200, vec![b' '; 11 * 1024 * 1024], vec![]));
        let api = api_para(&f.base);
        let r = api.buscar("/cnpj/1");
        assert_eq!(r.status, 502);
        assert_eq!(r.corpo, RESPOSTA_INVALIDA);
        api.buscar("/cnpj/1");
        assert_eq!(f.pedidos.lock().unwrap().len(), 2);
    }

    #[test]
    fn json_malformado_da_api_passa_como_veio() {
        // o modelo não interpreta o corpo: quem valida o JSON é quem consome
        let f = falso(|_| (200, b"{\"cnpj\": ".to_vec(), vec![]));
        let r = api_para(&f.base).buscar("/cnpj/1");
        assert_eq!(r.status, 200);
        assert_eq!(r.corpo, "{\"cnpj\": ");
    }

    #[test]
    fn pro_sem_chave_nao_chama_a_api() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let r = api_para(&f.base).pro("/v1/empresas/busca?cnae=6202300");
        assert_eq!(r.status, 402);
        assert!(r.corpo.contains("\"codigo\":\"plano_pro\""));
        assert!(
            r.corpo
                .contains("\"link\":\"https://cnpj.wortic.com/#planos\"")
        );
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn pro_com_chave_chama_a_rota_mandando_a_chave() {
        let f = falso(|_| {
            (
                200,
                b"{\"empresas\":[]}".to_vec(),
                vec![("x-base-cnpj", "2026-10")],
            )
        });
        let api = Api::com(
            &f.base,
            Some("chave-pro".to_string()),
            Duration::from_secs(5),
            GUARDA,
        );
        let r = api.pro("/v1/empresas/parecidas?cnpj=37853407000171&k=20");
        assert_eq!(r.status, 200);
        assert_eq!(r.base, Some("2026-10".to_string()));
        assert_eq!(
            f.pedidos.lock().unwrap()[0],
            (
                "/v1/empresas/parecidas?cnpj=37853407000171&k=20".to_string(),
                Some("chave-pro".to_string())
            )
        );
    }

    #[test]
    fn tempo_esgotado_vira_502_sem_rede() {
        let f = falso(|_| {
            thread::sleep(Duration::from_millis(1500));
            (200, b"{}".to_vec(), vec![])
        });
        let api = Api::com(&f.base, None, Duration::from_millis(100), GUARDA);
        let r = api.buscar("/cnpj/1");
        assert_eq!(r.status, 502);
        assert_eq!(r.corpo, SEM_REDE);
    }

    #[test]
    fn nada_escutando_vira_502_sem_rede() {
        // porta que acabou de ser liberada: ninguém escuta nela
        let porta = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let r = api_para(&format!("http://{porta}")).buscar("/cnpj/1");
        assert_eq!(r.status, 502);
        assert_eq!(r.corpo, SEM_REDE);
    }
}
