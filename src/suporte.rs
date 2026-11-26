//! Suporte dos testes: um servidor falso local que faz de conta que é a API da Wortic.
//! Não sai da máquina, então os testes são determinísticos e não gastam consulta de ninguém.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Response, Server};

/// Status, corpo e cabeçalhos extras que o servidor falso devolve.
pub type Roteiro = (u16, Vec<u8>, Vec<(&'static str, &'static str)>);

/// Um pedido recebido pelo servidor falso: o caminho e o valor de `x-api-key`, se veio.
pub type Pedido = (String, Option<String>);

pub struct Falso {
    pub base: String,
    pub pedidos: Arc<Mutex<Vec<Pedido>>>,
    servidor: Arc<Server>,
}

impl Drop for Falso {
    fn drop(&mut self) {
        self.servidor.unblock();
    }
}

/// Sobe um servidor falso numa porta livre. `roteiro` decide a resposta para cada caminho pedido.
pub fn falso<F>(roteiro: F) -> Falso
where
    F: Fn(&str) -> Roteiro + Send + Sync + 'static,
{
    let servidor = Arc::new(Server::http("127.0.0.1:0").expect("porta livre"));
    let base = format!(
        "http://{}",
        servidor.server_addr().to_ip().expect("endereço IP")
    );
    let pedidos = Arc::new(Mutex::new(Vec::new()));
    let (no_fio, gravados) = (servidor.clone(), pedidos.clone());
    thread::spawn(move || {
        for pedido in no_fio.incoming_requests() {
            let url = pedido.url().to_string();
            let chave = pedido
                .headers()
                .iter()
                .find(|h| h.field.equiv("x-api-key"))
                .map(|h| h.value.as_str().to_string());
            gravados.lock().unwrap().push((url.clone(), chave));
            let (status, corpo, cabecalhos) = roteiro(&url);
            let mut resposta = Response::from_data(corpo).with_status_code(status);
            for (nome, valor) in cabecalhos {
                resposta =
                    resposta.with_header(Header::from_bytes(nome, valor).expect("cabeçalho"));
            }
            let _ = pedido.respond(resposta);
        }
    });
    Falso {
        base,
        pedidos,
        servidor,
    }
}

/// O que o teste viu da resposta: status, corpo, `retry-after`, tipo do conteúdo e `x-base-cnpj`.
pub struct Visto {
    pub status: u16,
    pub corpo: String,
    pub espere: Option<String>,
    pub tipo: Option<String>,
    pub base: Option<String>,
}

/// GET de verdade contra um endereço local. Resposta de erro (4xx e 5xx) também volta como `Visto`.
pub fn pedir(url: &str) -> Visto {
    let resposta = match ureq::get(url).timeout(Duration::from_secs(5)).call() {
        Ok(r) => r,
        Err(e) => e.into_response().expect("resposta do servidor local"),
    };
    let status = resposta.status();
    let espere = resposta.header("retry-after").map(str::to_string);
    let tipo = resposta.header("content-type").map(str::to_string);
    let base = resposta.header("x-base-cnpj").map(str::to_string);
    let corpo = resposta.into_string().expect("corpo em texto");
    Visto {
        status,
        corpo,
        espere,
        tipo,
        base,
    }
}
