//! Roda o binário de verdade (o `main` só é coberto assim). Nada sai da máquina.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Uma porta livre agora (ela é solta logo depois, para o binário usar).
fn porta_livre() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Sobe o binário com o ambiente dado e devolve o processo e um canal com as linhas da saída.
fn subir_binario(ambiente: &[(&str, &str)], sem_chave: bool) -> (Child, mpsc::Receiver<String>) {
    let mut comando = Command::new(env!("CARGO_BIN_EXE_cadastro-cnpj"));
    comando
        .env("PORTA", porta_livre().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if sem_chave {
        comando.env_remove("WORTIC_API_KEY");
    }
    for (chave, valor) in ambiente {
        comando.env(chave, valor);
    }
    let mut filho = comando.spawn().expect("sobe o binário");
    let saida = filho.stdout.take().expect("saída do binário");
    let (envio, recebe) = mpsc::channel();
    thread::spawn(move || {
        for linha in BufReader::new(saida).lines().map_while(Result::ok) {
            if envio.send(linha).is_err() {
                break;
            }
        }
    });
    (filho, recebe)
}

#[test]
fn porta_que_nao_abre_para_o_binario_com_a_mensagem() {
    let saida = Command::new(env!("CARGO_BIN_EXE_cadastro-cnpj"))
        .env("PORTA", "abc")
        .output()
        .expect("roda o binário");
    assert!(!saida.status.success());
    let erro = String::from_utf8_lossy(&saida.stderr);
    assert!(
        erro.contains("não consegui abrir a porta"),
        "stderr: {erro}"
    );
}

#[test]
fn binario_sobe_na_porta_pedida_e_serve_a_tela() {
    // pega uma porta livre, solta, e pede ao binário que use ela
    let porta = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut filho = Command::new(env!("CARGO_BIN_EXE_cadastro-cnpj"))
        .env("PORTA", porta.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("sobe o binário");
    let url = format!("http://127.0.0.1:{porta}/");
    let inicio = Instant::now();
    let mut corpo = None;
    let mut ultimo = String::new();
    while inicio.elapsed() < Duration::from_secs(10) {
        match ureq::get(&url).timeout(Duration::from_secs(2)).call() {
            Ok(r) => {
                corpo = r.into_string().ok();
                break;
            }
            Err(e) => ultimo = e.to_string(),
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = filho.kill();
    let _ = filho.wait();
    let corpo = corpo.unwrap_or_else(|| panic!("o binário não respondeu na raiz: {ultimo}"));
    assert!(
        corpo.contains("<html") || corpo.contains("<!DOCTYPE"),
        "tela: {corpo:.80}"
    );
}

#[test]
fn sem_chave_e_fora_da_maquina_o_log_avisa_a_cota_compartilhada() {
    let (mut filho, linhas) = subir_binario(&[("ENDERECO", "0.0.0.0")], true);
    let no_ar = linhas.recv_timeout(Duration::from_secs(10));
    let aviso = linhas.recv_timeout(Duration::from_secs(10));
    let _ = filho.kill();
    let _ = filho.wait();
    assert!(no_ar.expect("linha de no ar").contains("no ar"));
    let aviso = aviso.expect("o aviso de cota no log");
    assert!(
        aviso.contains("dividem 60 consultas por minuto e 2.000 por dia"),
        "aviso: {aviso}"
    );
}

#[test]
fn com_chave_fora_da_maquina_nao_ha_aviso_de_cota() {
    let (mut filho, linhas) = subir_binario(
        &[
            ("ENDERECO", "0.0.0.0"),
            ("WORTIC_API_KEY", "chave-de-teste"),
        ],
        false,
    );
    let no_ar = linhas.recv_timeout(Duration::from_secs(10));
    // a segunda linha só viria com o aviso: se não vem em 1,5 s, não há aviso
    let segunda = linhas.recv_timeout(Duration::from_millis(1500));
    let _ = filho.kill();
    let _ = filho.wait();
    assert!(no_ar.expect("linha de no ar").contains("no ar"));
    assert!(
        segunda.is_err(),
        "não devia haver segunda linha: {segunda:?}"
    );
}

#[test]
fn na_propria_maquina_sem_chave_nao_ha_aviso_de_cota() {
    let (mut filho, linhas) = subir_binario(&[("ENDERECO", "127.0.0.1")], true);
    let no_ar = linhas.recv_timeout(Duration::from_secs(10));
    let segunda = linhas.recv_timeout(Duration::from_millis(1500));
    let _ = filho.kill();
    let _ = filho.wait();
    assert!(no_ar.expect("linha de no ar").contains("no ar"));
    assert!(
        segunda.is_err(),
        "não devia haver segunda linha: {segunda:?}"
    );
}
