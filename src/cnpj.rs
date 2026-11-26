//! O CNPJ: limpar o que a pessoa digitou e conferir os dígitos verificadores.
//! Vale para o CNPJ de números e para o alfanumérico (letras nas 12 primeiras posições).

/// Tira pontuação e espaços e põe as letras em maiúsculas.
pub fn limpar(texto: &str) -> String {
    texto
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Confere tamanho, caracteres e os dois dígitos verificadores.
pub fn valido(cnpj: &str) -> bool {
    const PESOS: [u32; 13] = [6, 5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2];
    let b = cnpj.as_bytes();
    if b.len() != 14 {
        return false;
    }
    let corpo_ok = b[..12]
        .iter()
        .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
    let dv_ok = b[12..].iter().all(u8::is_ascii_digit);
    if !corpo_ok || !dv_ok || b.iter().all(|c| *c == b[0]) {
        return false;
    }
    for n in [12usize, 13] {
        let soma: u32 = (0..n)
            .map(|i| (u32::from(b[i]) - 48) * PESOS[13 - n + i])
            .sum();
        let resto = soma % 11;
        let esperado = if resto < 2 { 0 } else { 11 - resto };
        if u32::from(b[n]) - 48 != esperado {
            return false;
        }
    }
    true
}

/// Desfaz o %XX de um trecho de endereço (o navegador manda "/" como %2F).
/// O que não for %XX com dois dígitos hexadecimais passa como veio.
pub fn sem_porcento(texto: &str) -> String {
    let b = texto.as_bytes();
    let mut saida = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(hex) = texto.get(i + 1..i + 3)
            && hex.bytes().all(|c| c.is_ascii_hexdigit())
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            saida.push(v);
            i += 3;
            continue;
        }
        saida.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&saida).into_owned()
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn limpar_tira_pontuacao_espaco_e_poe_maiusculas() {
        assert_eq!(limpar("37.853.407/0001-71"), "37853407000171");
        assert_eq!(limpar("12.abc.345/01de-35"), "12ABC34501DE35");
        assert_eq!(limpar("  12 ABC\t345\n"), "12ABC345");
        assert_eq!(limpar("ção-1"), "O1");
        assert_eq!(limpar(""), "");
    }

    #[test]
    fn valido_aceita_cnpj_numerico_com_e_sem_mascara() {
        assert!(valido(&limpar("37.853.407/0001-71")));
        assert!(valido("37853407000171"));
        assert!(valido("12345678000195"));
    }

    #[test]
    fn valido_recusa_dv_errado_no_primeiro_e_no_segundo() {
        assert!(!valido("37853407000172"));
        assert!(!valido("37853407000161"));
    }

    #[test]
    fn valido_recusa_tamanho_errado_e_vazio() {
        assert!(!valido(""));
        assert!(!valido("123"));
        assert!(!valido("3785340700017"));
        assert!(!valido("378534070001710"));
    }

    #[test]
    fn valido_recusa_todos_os_digitos_iguais() {
        assert!(!valido("00000000000000"));
        assert!(!valido("11111111111111"));
    }

    #[test]
    fn valido_recusa_simbolo_e_mascara_sem_limpar() {
        assert!(!valido("3785340700017-"));
        assert!(!valido("3785340700-171"));
        assert!(!valido("37.853.407/0001-71"));
    }

    #[test]
    fn valido_recusa_minuscula_sem_limpar_mas_limpar_resolve() {
        assert!(!valido("12abc34501de35"));
        assert!(valido(&limpar("12abc34501de35")));
    }

    #[test]
    fn valido_recusa_letra_nos_digitos_verificadores() {
        assert!(!valido("12ABC34501DEA5"));
    }

    #[test]
    fn valido_recusa_caractere_estranho_no_corpo() {
        assert!(!valido("12ABC3450!DE35"));
    }

    #[test]
    fn valido_aceita_o_alfanumerico_da_receita_2026() {
        // exemplo público da Receita Federal para o CNPJ alfanumérico
        assert!(valido(&limpar("12.ABC.345/01DE-35")));
        assert!(valido("ABCDEFGHIJKL80"));
        assert!(valido("A1B2C3D4E5F668"));
        assert!(valido("ZZZZZZZZZZZZ62"));
    }

    #[test]
    fn valido_recusa_alfanumerico_com_dv_errado() {
        assert!(!valido("12ABC34501DE34"));
        assert!(!valido("ABCDEFGHIJKL81"));
    }

    #[test]
    fn sem_porcento_desfaz_a_barra_codificada() {
        assert!(valido(&limpar(&sem_porcento("37.853.407%2F0001-71"))));
        assert_eq!(sem_porcento("a%2Fb"), "a/b");
        assert_eq!(sem_porcento("%41%42"), "AB");
    }

    #[test]
    fn sem_porcento_deixa_como_veio_o_que_nao_e_hex_valido() {
        assert_eq!(sem_porcento("abc%"), "abc%");
        assert_eq!(sem_porcento("%4"), "%4");
        assert_eq!(sem_porcento("%zz1"), "%zz1");
        assert_eq!(sem_porcento("%+1"), "%+1");
        assert_eq!(sem_porcento("%"), "%");
    }

    #[test]
    fn sem_porcento_nao_quebra_com_texto_acentuado_depois_do_porcento() {
        assert_eq!(sem_porcento("%1é"), "%1é");
        assert_eq!(sem_porcento("%é1"), "%é1");
    }
}
