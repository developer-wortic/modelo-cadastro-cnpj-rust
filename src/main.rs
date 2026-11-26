//! Cadastro de clientes que se preenche pelo CNPJ: modelo da Wortic em Rust.
//!
//! Um binário só. Serve a tela (static/index.html, embutida) e estas rotas:
//!   GET /api/config                     diz só se há chave (true ou false)
//!   GET /api/empresa/{cnpj}             ficha da empresa (confere o CNPJ aqui e só então consulta a API)
//!   GET /api/socios/{cnpj}              sócios da empresa
//!   GET /api/empresas?nome=&uf=         empresas pelo nome (4 letras ou mais)
//!   GET /api/pro/busca?cnae=&uf=...     busca avançada (plano Pro: sem chave, nada sai daqui)
//!   GET /api/pro/parecidas/{cnpj}       empresas parecidas (plano Pro)
//! Rode com `cargo run --release` e abra <http://127.0.0.1:8080>.
//! Variáveis: `PORTA` (padrão 8080), `ENDERECO` (padrão 127.0.0.1), `WORTIC_API_KEY` (opcional, para volume
//! acima do uso aberto), `CADASTRO_CACHE_MAX` (padrão 5000), `CADASTRO_API_BASE` (padrão a API aberta).

mod api;
mod cnpj;
#[cfg(test)]
mod suporte;

use std::net::IpAddr;
use std::sync::Arc;
use std::thread;
use tiny_http::{Header, Response, Server};

const TELA: &str = include_str!("../static/index.html");
const CAMPOS_BUSCA_NOME: &[&str] = &["nome", "uf"];
const CAMPOS_BUSCA_PRO: &[&str] = &[
    "cnae",
    "uf",
    "municipio",
    "porte",
    "capital_min",
    "capital_max",
    "cursor",
];
const AVISO_COTA: &str = "AVISO: sem WORTIC_API_KEY, todos os usuários deste servidor dividem 60 consultas por minuto e 2.000 por dia, do IP do servidor. Em produção, defina WORTIC_API_KEY com a chave do seu plano.";

fn cabecalho(nome: &str, valor: &str) -> Header {
    Header::from_bytes(nome.as_bytes(), valor.as_bytes()).expect("cabeçalho válido")
}

fn responder_api(pedido: tiny_http::Request, r: api::Resposta) {
    let mut resposta = Response::from_string(r.corpo)
        .with_status_code(r.status)
        .with_header(cabecalho("content-type", "application/json; charset=utf-8"));
    if let Some(segundos) = &r.espere {
        resposta = resposta.with_header(cabecalho("retry-after", segundos));
    }
    if let Some(base) = &r.base {
        resposta = resposta.with_header(cabecalho("x-base-cnpj", base));
    }
    let _ = pedido.respond(resposta);
}

fn erro(pedido: tiny_http::Request, status: u16, codigo: &str, mensagem: &str) {
    let corpo = format!(r#"{{"erro":{{"codigo":"{codigo}","mensagem":"{mensagem}"}}}}"#);
    responder_api(
        pedido,
        api::Resposta {
            status,
            corpo,
            espere: None,
            base: None,
        },
    );
}

/// Confere o CNPJ da rota. Sem ele válido, responde 400 e não gasta consulta.
fn com_cnpj(pedido: tiny_http::Request, resto: &str, montar: impl FnOnce(&str) -> api::Resposta) {
    let numero = cnpj::limpar(&cnpj::sem_porcento(resto));
    if cnpj::valido(&numero) {
        responder_api(pedido, montar(&numero));
    } else {
        erro(
            pedido,
            400,
            "cnpj_invalido",
            "CNPJ inválido: confira os dígitos.",
        );
    }
}

/// Só passam as chaves da lista. O valor fica só com letras, números e os sinais comuns de consulta.
fn filtrar(consulta: &str, chaves: &[&str]) -> String {
    consulta
        .split('&')
        .filter_map(|par| {
            let (chave, valor) = par.split_once('=')?;
            if !chaves.contains(&chave) {
                return None;
            }
            let limpo: String = valor
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || ",.-_%+".contains(*c))
                .collect();
            Some(format!("{chave}={limpo}"))
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn atender(pedido: tiny_http::Request, api: &api::Api) {
    let url = pedido.url().to_string();
    let (caminho, consulta) = url.split_once('?').unwrap_or((url.as_str(), ""));
    if caminho == "/" {
        let _ = pedido.respond(
            Response::from_string(TELA)
                .with_header(cabecalho("content-type", "text/html; charset=utf-8")),
        );
    } else if caminho == "/api/config" {
        // só diz se há chave; a chave em si nunca sai
        responder_api(
            pedido,
            api::Resposta::ok(format!("{{\"chave\":{}}}", api.tem_chave())),
        );
    } else if let Some(resto) = caminho.strip_prefix("/api/empresa/") {
        com_cnpj(pedido, resto, |n| api.buscar(&format!("/cnpj/{n}")));
    } else if let Some(resto) = caminho.strip_prefix("/api/socios/") {
        com_cnpj(pedido, resto, |n| api.buscar(&format!("/cnpj/{n}/socios")));
    } else if let Some(resto) = caminho.strip_prefix("/api/pro/parecidas/") {
        com_cnpj(pedido, resto, |n| {
            api.pro(&format!("/v1/empresas/parecidas?cnpj={n}&k=20"))
        });
    } else if caminho == "/api/pro/busca" {
        let filtros = filtrar(consulta, CAMPOS_BUSCA_PRO);
        responder_api(pedido, api.pro(&format!("/v1/empresas/busca?{filtros}")));
    } else if caminho == "/api/empresas" {
        // só as letras, números e os sinais comuns passam adiante
        let filtros = filtrar(consulta, CAMPOS_BUSCA_NOME);
        responder_api(pedido, api.buscar(&format!("/empresas?{filtros}")));
    } else {
        erro(pedido, 404, "nao_existe", "Rota não encontrada.");
    }
}

/// A porta vem de `PORTA`; sem ela, 8080.
fn porta_de(valor: Option<String>) -> String {
    valor.unwrap_or_else(|| "8080".to_string())
}

/// O endereço vem de `ENDERECO`; sem ele, 127.0.0.1 (só a própria máquina).
fn endereco_de(valor: Option<String>) -> String {
    valor
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// É local o que é da própria máquina: `localhost` e o bloco de loopback (`127.x.x.x` e `::1`).
fn e_local(endereco: &str) -> bool {
    endereco == "localhost" || endereco.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Sem chave e fora da própria máquina, todos os usuários dividem o mesmo IP e o mesmo limite.
fn aviso_de_cota(endereco: &str, com_chave: bool) -> Option<&'static str> {
    (!com_chave && !e_local(endereco)).then_some(AVISO_COTA)
}

/// Abre a porta no endereço. Se não abrir, para com a mensagem.
fn subir(endereco: &str, porta: &str) -> Arc<Server> {
    let host = if endereco.contains(':') {
        format!("[{endereco}]")
    } else {
        endereco.to_string()
    };
    let servidor = Server::http(format!("{host}:{porta}")).expect("não consegui abrir a porta");
    println!("Cadastro pelo CNPJ no ar em http://{host}:{porta}");
    Arc::new(servidor)
}

/// Atende os pedidos com `linhas` threads. Só volta quando o servidor for desligado.
fn servir(servidor: &Arc<Server>, api: &Arc<api::Api>, linhas: usize) {
    let mut trabalhadores = Vec::new();
    for _ in 0..linhas {
        let (servidor, api) = (servidor.clone(), api.clone());
        trabalhadores.push(thread::spawn(move || {
            for pedido in servidor.incoming_requests() {
                atender(pedido, &api);
            }
        }));
    }
    for trabalhador in trabalhadores {
        let _ = trabalhador.join();
    }
}

/// Abre a porta e, se a cota compartilhada está em jogo, avisa no log. Devolve o servidor pronto para servir.
fn preparar(endereco: &str, porta: &str, api: &api::Api) -> Arc<Server> {
    let servidor = subir(endereco, porta);
    if let Some(aviso) = aviso_de_cota(endereco, api.tem_chave()) {
        println!("{aviso}");
    }
    servidor
}

fn executar(endereco: &str, porta: &str) {
    let api = Arc::new(api::Api::nova());
    let servidor = preparar(endereco, porta, &api);
    servir(&servidor, &api, 4);
}

fn main() {
    executar(
        &endereco_de(std::env::var("ENDERECO").ok()),
        &porta_de(std::env::var("PORTA").ok()),
    );
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::suporte::{falso, pedir};
    use std::time::Duration;

    const LINHAS: usize = 2;
    const LINK_PLANOS: &str = "https://cnpj.wortic.com/#planos";

    /// Um servidor de verdade numa porta livre, ligado à API falsa. Ao sair, desliga e espera as threads.
    struct Local {
        endereco: String,
        servidor: Arc<Server>,
        fio: Option<thread::JoinHandle<()>>,
    }

    impl Drop for Local {
        fn drop(&mut self) {
            // cada unblock acorda e para uma única thread de atendimento
            for _ in 0..LINHAS {
                self.servidor.unblock();
            }
            let _ = self.fio.take().map(thread::JoinHandle::join);
        }
    }

    fn sobe_com(base: &str, chave: Option<String>) -> Local {
        let servidor = subir("127.0.0.1", "0");
        let endereco = format!(
            "http://{}",
            servidor.server_addr().to_ip().expect("endereço IP")
        );
        let api = Arc::new(api::Api::com(
            base,
            chave,
            Duration::from_secs(5),
            Duration::from_secs(60),
        ));
        let no_fio = servidor.clone();
        let fio = thread::spawn(move || servir(&no_fio, &api, LINHAS));
        Local {
            endereco,
            servidor,
            fio: Some(fio),
        }
    }

    fn sobe(base: &str) -> Local {
        sobe_com(base, None)
    }

    #[test]
    fn porta_padrao_e_8080_e_a_variavel_vale() {
        assert_eq!(porta_de(None), "8080");
        assert_eq!(porta_de(Some("9000".to_string())), "9000");
    }

    #[test]
    fn endereco_padrao_e_a_propria_maquina_e_a_variavel_vale() {
        assert_eq!(endereco_de(None), "127.0.0.1");
        assert_eq!(endereco_de(Some(String::new())), "127.0.0.1");
        assert_eq!(endereco_de(Some("0.0.0.0".to_string())), "0.0.0.0");
    }

    #[test]
    fn aviso_de_cota_so_sai_sem_chave_e_fora_da_maquina() {
        let texto = aviso_de_cota("0.0.0.0", false).expect("aviso");
        assert!(texto.contains("dividem 60 consultas por minuto e 2.000 por dia"));
        assert!(texto.contains("WORTIC_API_KEY"));
        assert!(aviso_de_cota("192.168.0.10", false).is_some());
        assert!(aviso_de_cota("minha.maquina", false).is_some());
        // com chave, ou na própria máquina, não avisa
        assert!(aviso_de_cota("0.0.0.0", true).is_none());
        assert!(aviso_de_cota("127.0.0.1", false).is_none());
        assert!(aviso_de_cota("localhost", false).is_none());
        assert!(aviso_de_cota("::1", false).is_none());
        assert!(aviso_de_cota("127.0.0.9", false).is_none());
    }

    #[test]
    fn cabecalho_leva_nome_e_valor() {
        let h = cabecalho("retry-after", "30");
        assert!(h.field.equiv("retry-after"));
        assert_eq!(h.value.as_str(), "30");
    }

    #[test]
    fn subir_com_porta_livre_abre_o_servidor() {
        let servidor = subir("127.0.0.1", "0");
        assert!(servidor.server_addr().to_ip().is_some());
    }

    #[test]
    #[should_panic(expected = "não consegui abrir a porta")]
    fn subir_com_porta_invalida_para_com_a_mensagem() {
        let _servidor = subir("127.0.0.1", "abc");
    }

    #[test]
    #[should_panic(expected = "não consegui abrir a porta")]
    fn subir_com_endereco_ipv6_e_porta_invalida_para_com_a_mensagem() {
        let _servidor = subir("::1", "abc");
    }

    fn api_sem_chave() -> api::Api {
        api::Api::com(
            "http://nao-usado",
            None,
            Duration::from_secs(5),
            Duration::from_secs(60),
        )
    }

    #[test]
    fn preparar_sem_chave_fora_da_maquina_avisa_e_abre_a_porta() {
        // o aviso sai no log (println); o teste confere que a porta abriu
        let servidor = preparar("0.0.0.0", "0", &api_sem_chave());
        assert!(servidor.server_addr().to_ip().is_some());
    }

    #[test]
    fn preparar_na_propria_maquina_ou_com_chave_abre_sem_aviso() {
        let servidor = preparar("127.0.0.1", "0", &api_sem_chave());
        assert!(servidor.server_addr().to_ip().is_some());
        let com_chave = api::Api::com(
            "http://nao-usado",
            Some("chave-de-teste".to_string()),
            Duration::from_secs(5),
            Duration::from_secs(60),
        );
        let servidor = preparar("0.0.0.0", "0", &com_chave);
        assert!(servidor.server_addr().to_ip().is_some());
    }

    #[test]
    #[should_panic(expected = "não consegui abrir a porta")]
    fn executar_com_porta_invalida_para_antes_de_servir() {
        executar("127.0.0.1", "abc");
    }

    #[test]
    fn raiz_devolve_a_tela_embutida() {
        let f = falso(|_| (200, b"nao deve ser usado".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/", l.endereco));
        assert_eq!(r.status, 200);
        assert_eq!(r.corpo, TELA);
        assert_eq!(r.tipo.as_deref(), Some("text/html; charset=utf-8"));
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn config_diz_so_se_ha_chave_e_nunca_mostra_a_chave() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let sem = sobe(&f.base);
        let r = pedir(&format!("{}/api/config", sem.endereco));
        assert_eq!(r.status, 200);
        assert_eq!(r.corpo, "{\"chave\":false}");
        let com = sobe_com(&f.base, Some("segredo-123".to_string()));
        let r = pedir(&format!("{}/api/config", com.endereco));
        assert_eq!(r.corpo, "{\"chave\":true}");
        assert!(!r.corpo.contains("segredo"));
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn empresa_com_barra_codificada_consulta_so_os_digitos() {
        let f = falso(|_| (200, b"{\"razao\":\"x\"}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/empresa/37.853.407%2F0001-71", l.endereco));
        assert_eq!(r.status, 200);
        assert_eq!(r.corpo, "{\"razao\":\"x\"}");
        assert_eq!(r.tipo.as_deref(), Some("application/json; charset=utf-8"));
        assert_eq!(f.pedidos.lock().unwrap()[0].0, "/cnpj/37853407000171");
    }

    #[test]
    fn empresa_sem_mascara_consulta_o_mesmo_caminho() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        pedir(&format!("{}/api/empresa/37853407000171", l.endereco));
        assert_eq!(f.pedidos.lock().unwrap()[0].0, "/cnpj/37853407000171");
    }

    #[test]
    fn empresa_alfanumerica_em_minuscula_vira_maiuscula_na_consulta() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/empresa/12abc34501de35", l.endereco));
        assert_eq!(r.status, 200);
        assert_eq!(f.pedidos.lock().unwrap()[0].0, "/cnpj/12ABC34501DE35");
    }

    #[test]
    fn base_da_api_chega_ao_cliente_no_cabecalho() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![("x-base-cnpj", "2026-10")]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/empresa/37853407000171", l.endereco));
        assert_eq!(r.base.as_deref(), Some("2026-10"));
    }

    #[test]
    fn socios_consulta_a_rota_de_socios() {
        let f = falso(|_| (200, b"[]".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/socios/37853407000171", l.endereco));
        assert_eq!(r.status, 200);
        assert_eq!(
            f.pedidos.lock().unwrap()[0].0,
            "/cnpj/37853407000171/socios"
        );
    }

    #[test]
    fn cnpj_com_digito_errado_da_400_sem_gastar_consulta() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/empresa/37853407000172", l.endereco));
        assert_eq!(r.status, 400);
        assert!(r.corpo.contains("cnpj_invalido"));
        assert_eq!(r.tipo.as_deref(), Some("application/json; charset=utf-8"));
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn socios_com_cnpj_invalido_da_400() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/socios/123", l.endereco));
        assert_eq!(r.status, 400);
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn empresa_sem_cnpj_da_400() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/empresa/", l.endereco));
        assert_eq!(r.status, 400);
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn limite_da_api_chega_ao_cliente_com_o_retry_after() {
        let f = falso(|_| (429, b"muitas".to_vec(), vec![("retry-after", "45")]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/empresa/37853407000171", l.endereco));
        assert_eq!(r.status, 429);
        assert_eq!(r.espere.as_deref(), Some("45"));
        assert!(r.corpo.contains("\"espere\":45"));
    }

    #[test]
    fn limite_gratis_402_chega_com_espera_e_link_dos_planos() {
        let f = falso(|_| {
            (
                402,
                b"{\"erro\":{\"codigo\":\"limite_gratis\",\"mensagem\":\"x\"},\"planos\":[]}"
                    .to_vec(),
                vec![("retry-after", "37")],
            )
        });
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/empresa/37853407000171", l.endereco));
        assert_eq!(r.status, 402);
        assert_eq!(r.espere.as_deref(), Some("37"));
        assert_eq!(r.tipo.as_deref(), Some("application/json; charset=utf-8"));
        assert!(r.corpo.contains("\"codigo\":\"limite_gratis\""));
        assert!(r.corpo.contains(&format!("\"link\":\"{LINK_PLANOS}\"")));
    }

    #[test]
    fn ip_banido_403_chega_com_a_explicacao_e_o_link() {
        let f = falso(|_| {
            (
                403,
                b"{\"erro\":{\"codigo\":\"banido\",\"mensagem\":\"x\"}}".to_vec(),
                vec![],
            )
        });
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/socios/37853407000171", l.endereco));
        assert_eq!(r.status, 403);
        assert!(r.corpo.contains("foi banido"));
        assert!(r.corpo.contains(&format!("\"link\":\"{LINK_PLANOS}\"")));
    }

    #[test]
    fn busca_por_nome_repassa_o_que_e_permitido() {
        let f = falso(|_| (200, b"[]".to_vec(), vec![]));
        let l = sobe(&f.base);
        pedir(&format!(
            "{}/api/empresas?nome=padaria%20x&uf=SP",
            l.endereco
        ));
        assert_eq!(
            f.pedidos.lock().unwrap()[0].0,
            "/empresas?nome=padaria%20x&uf=SP"
        );
    }

    #[test]
    fn busca_tira_o_que_nao_esta_na_lista() {
        let f = falso(|_| (200, b"[]".to_vec(), vec![]));
        let l = sobe(&f.base);
        pedir(&format!(
            "{}/api/empresas?nome=ab~c*d!e&cidade=SP",
            l.endereco
        ));
        assert_eq!(f.pedidos.lock().unwrap()[0].0, "/empresas?nome=abcde");
    }

    #[test]
    fn busca_sem_consulta_manda_so_a_rota() {
        let f = falso(|_| (200, b"[]".to_vec(), vec![]));
        let l = sobe(&f.base);
        pedir(&format!("{}/api/empresas", l.endereco));
        // o servidor manda "/empresas?" e o ureq tira o "?" vazio ao enviar
        assert_eq!(f.pedidos.lock().unwrap()[0].0, "/empresas");
    }

    #[test]
    fn filtrar_mantem_so_as_chaves_da_lista() {
        assert_eq!(
            filtrar("a=1&nome=x&b=2&uf=SP", &["nome", "uf"]),
            "nome=x&uf=SP"
        );
        assert_eq!(filtrar("nome", &["nome"]), "");
        assert_eq!(
            filtrar("uf=S%20P&cnae=1,2", &["uf", "cnae"]),
            "uf=S%20P&cnae=1,2"
        );
        assert_eq!(filtrar("nome=a=b", &["nome"]), "nome=ab");
    }

    #[test]
    fn busca_pro_sem_chave_da_aviso_e_nao_chama_a_api() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/pro/busca?cnae=6202300&uf=SP", l.endereco));
        assert_eq!(r.status, 402);
        assert!(r.corpo.contains("\"codigo\":\"plano_pro\""));
        assert!(r.corpo.contains(&format!("\"link\":\"{LINK_PLANOS}\"")));
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn busca_pro_com_chave_repassa_so_os_filtros_com_a_chave() {
        let f = falso(|_| (200, b"{\"empresas\":[]}".to_vec(), vec![]));
        let l = sobe_com(&f.base, Some("segredo".to_string()));
        let r = pedir(&format!(
            "{}/api/pro/busca?cnae=6202300&uf=SP&municipio=3550308&porte=me&capital_min=1000&capital_max=2000,50&cursor=abcdefabcdefab&xx=1&nome=x",
            l.endereco
        ));
        assert_eq!(r.status, 200);
        assert_eq!(
            f.pedidos.lock().unwrap()[0],
            (
                "/v1/empresas/busca?cnae=6202300&uf=SP&municipio=3550308&porte=me&capital_min=1000&capital_max=2000,50&cursor=abcdefabcdefab".to_string(),
                Some("segredo".to_string())
            )
        );
    }

    #[test]
    fn parecidas_sem_chave_da_aviso_e_nao_chama_a_api() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/api/pro/parecidas/37853407000171", l.endereco));
        assert_eq!(r.status, 402);
        assert!(r.corpo.contains("\"codigo\":\"plano_pro\""));
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn parecidas_com_chave_chama_a_rota_com_o_cnpj_conferido() {
        let f = falso(|_| (200, b"{\"referencia\":{},\"empresas\":[]}".to_vec(), vec![]));
        let l = sobe_com(&f.base, Some("segredo".to_string()));
        let r = pedir(&format!(
            "{}/api/pro/parecidas/37.853.407%2F0001-71",
            l.endereco
        ));
        assert_eq!(r.status, 200);
        assert_eq!(
            f.pedidos.lock().unwrap()[0],
            (
                "/v1/empresas/parecidas?cnpj=37853407000171&k=20".to_string(),
                Some("segredo".to_string())
            )
        );
    }

    #[test]
    fn parecidas_com_cnpj_invalido_da_400_sem_gastar_consulta() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe_com(&f.base, Some("segredo".to_string()));
        let r = pedir(&format!("{}/api/pro/parecidas/123", l.endereco));
        assert_eq!(r.status, 400);
        assert!(f.pedidos.lock().unwrap().is_empty());
    }

    #[test]
    fn rota_que_nao_existe_da_404_em_json() {
        let f = falso(|_| (200, b"{}".to_vec(), vec![]));
        let l = sobe(&f.base);
        let r = pedir(&format!("{}/nao_existe", l.endereco));
        assert_eq!(r.status, 404);
        assert!(r.corpo.contains("nao_existe"));
        assert_eq!(r.tipo.as_deref(), Some("application/json; charset=utf-8"));
        assert!(f.pedidos.lock().unwrap().is_empty());
    }
}
