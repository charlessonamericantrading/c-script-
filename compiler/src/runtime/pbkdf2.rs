// PBKDF2-HMAC-SHA256 (RFC 8018 §5.2), hand-rolleado sobre `hmac`+`sha2` -- las
// dos ya son dependencias de este binario (`crypto.hmacSha256`/`hashSha256`),
// así que esto no es una excepción nueva a "cero dependencias nuevas": es la
// misma composición que un dev haría a mano en cualquier lenguaje sin un
// `pbkdf2` builtin, sobre primitivos ya presentes. Único consumidor:
// `crypto.verifyPbkdf2Sha256` (GRAMMAR.md §3.284, PLAN.md §9.24 Fase 2 ítem
// D1) -- migrar `ADMIN_PASSWORD` (formato `pbkdf2:<iter>:<salt>:<hash>`) sin
// romper la contraseña real de producción.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Deriva `output_len` bytes -- NO un largo fijo: el largo de la clave
/// derivada original (`hashHex.len() / 2` en el llamador) determina cuántos
/// bytes hacen falta acá, así que esto funciona sea cual sea el `dkLen` con
/// el que el valor guardado se generó originalmente, sin necesidad de
/// adivinarlo de antemano.
pub(crate) fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iterations: u32, output_len: usize) -> Vec<u8> {
    const HASH_LEN: usize = 32; // SHA-256 produce 32 bytes por bloque.
    let block_count = output_len.div_ceil(HASH_LEN);
    let mut derived = Vec::with_capacity(block_count * HASH_LEN);
    for block_index in 1..=block_count as u32 {
        // U_1 = HMAC(password, salt || INT_BE(block_index)). Una instancia
        // NUEVA de `Hmac` por cada cómputo (no `finalize_reset`, que exige
        // un bound `FixedOutputReset` que choca con la versión de `hmac`
        // que otra dependencia trae al árbol) -- mismo patrón que
        // `crypto.hmacSha256` ya usa más abajo en este mismo módulo.
        let mut mac = HmacSha256::new_from_slice(password).expect("HMAC-SHA256 acepta claves de cualquier largo");
        mac.update(salt);
        mac.update(&block_index.to_be_bytes());
        let mut u = mac.finalize().into_bytes();
        let mut t = u;
        // U_2..U_c, XOR acumulado en T.
        for _ in 1..iterations {
            let mut mac = HmacSha256::new_from_slice(password).expect("HMAC-SHA256 acepta claves de cualquier largo");
            mac.update(&u);
            u = mac.finalize().into_bytes();
            for (t_byte, u_byte) in t.iter_mut().zip(u.iter()) {
                *t_byte ^= u_byte;
            }
        }
        derived.extend_from_slice(&t);
    }
    derived.truncate(output_len);
    derived
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    // Vectores generados con `hashlib.pbkdf2_hmac('sha256', ...)` de Python
    // (implementación de referencia de la librería estándar) -- los tres
    // primeros coinciden con los vectores de RFC 7914 apéndice A, ya
    // ampliamente citados para PBKDF2-HMAC-SHA256.
    #[test]
    fn matches_the_reference_vector_for_one_iteration() {
        assert_eq!(
            hex(&pbkdf2_hmac_sha256(b"password", b"salt", 1, 32)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
    }

    #[test]
    fn matches_the_reference_vector_for_two_iterations() {
        assert_eq!(
            hex(&pbkdf2_hmac_sha256(b"password", b"salt", 2, 32)),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }

    #[test]
    fn matches_the_reference_vector_for_four_thousand_ninety_six_iterations() {
        assert_eq!(
            hex(&pbkdf2_hmac_sha256(b"password", b"salt", 4096, 32)),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn matches_the_reference_vector_with_a_derived_length_not_a_multiple_of_the_hash_block() {
        assert_eq!(
            hex(&pbkdf2_hmac_sha256(b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, 40)),
            "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9"
        );
    }

    #[test]
    fn matches_the_reference_vector_for_a_realistic_sixty_four_byte_derived_key() {
        assert_eq!(
            hex(&pbkdf2_hmac_sha256(b"hunter2", b"abcd1234abcd1234abcd1234abcd1234", 100000, 64)),
            "30b678ed24d68cc6263cb65536e208e3f6984c209a7191d36ceed347a557e0124b0b59f6e8c9dadcdb1b9e79ad4a749a960510bebad8a2e574fe0733bc1a57ad"
        );
    }

    #[test]
    fn different_passwords_never_derive_the_same_key() {
        let a = pbkdf2_hmac_sha256(b"correct horse", b"salt", 10, 32);
        let b = pbkdf2_hmac_sha256(b"wrong horse", b"salt", 10, 32);
        assert_ne!(a, b);
    }
}
