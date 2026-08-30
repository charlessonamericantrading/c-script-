// `@encrypted` (GRAMMAR.md §3.191): AES-256-GCM sobre un campo `String`/
// `String?`, puramente a nivel de ALMACENAMIENTO -- el `Value` que ve el
// resto del programa (el intérprete, el wire JSON) sigue siendo el `String`
// plano de siempre, nunca cambia de tipo. Un solo chokepoint de cada lado
// (`write_param`/`decode_row` en `runtime/db.rs`) llama a `encrypt_field`/
// `decrypt_field` cuando `ColumnPlan::encrypted` es `true`.
//
// Formato de almacenamiento: `nonce (12 bytes) || ciphertext || tag (16
// bytes, ya concatenados por la propia crate)`, todo junto, en base64
// estándar, guardado como `TEXT`/`Cell::Text` normal -- sin `ColumnKind`
// nuevo, sin `BYTEA` en Postgres. Nonce nuevo y aleatorio en CADA
// escritura (`os_random_bytes`, el mismo CSPRNG del sistema que
// `crypto.uuid()`/`crypto.randomToken()` ya usan) -- nunca reusado, lo que
// violaría la garantía de AES-GCM.

use super::RuntimeError;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

const NONCE_LEN: usize = 12;
pub(crate) const KEY_LEN: usize = 32;

/// `--encryption-key`/`LINK_ENCRYPTION_KEY`: 32 bytes en base64 estándar.
/// Nunca se acepta una longitud distinta -- errar temprano y claro (al
/// arrancar `linkc serve`) es preferible a truncar/paddear en silencio una
/// clave del tamaño equivocado.
pub(crate) fn parse_encryption_key(raw: &str) -> Result<[u8; KEY_LEN], String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(raw.trim()).map_err(|e| format!("no es base64 válido: {e}"))?;
    let len = bytes.len();
    bytes.try_into().map_err(|_| format!("tiene que decodificar a exactamente {KEY_LEN} bytes (AES-256) -- decodificó a {len}"))
}

/// Cifra `plaintext` -- ver la doc del módulo para el formato de
/// almacenamiento completo.
pub(crate) fn encrypt_field(plaintext: &str, key: &[u8; KEY_LEN]) -> Result<String, RuntimeError> {
    use base64::Engine;
    let cipher = Aes256Gcm::new(key.into());
    let nonce_bytes = super::os_random_bytes(NONCE_LEN)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext =
        cipher.encrypt(nonce, plaintext.as_bytes()).map_err(|_| RuntimeError::new("no se pudo cifrar el campo"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

/// Inversa de `encrypt_field`. Un error acá (base64 inválido, payload más
/// corto que un nonce, o la autenticación de GCM fallando -- clave
/// incorrecta O dato corrompido/manipulado, AEAD no distingue las dos) es
/// un `RuntimeError` limpio, nunca un panic -- una fila con datos
/// inconsistentes no debería tumbar el hilo de esa request.
pub(crate) fn decrypt_field(stored: &str, key: &[u8; KEY_LEN]) -> Result<String, RuntimeError> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(stored)
        .map_err(|e| RuntimeError::new(format!("valor cifrado guardado no es base64 válido: {e}")))?;
    if raw.len() < NONCE_LEN {
        return Err(RuntimeError::new("valor cifrado guardado más corto que un nonce -- dato truncado o corrompido"));
    }
    let (nonce_bytes, ciphertext) = raw.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| RuntimeError::new("no se pudo descifrar el campo -- clave incorrecta, o el dato fue corrompido/manipulado"))?;
    String::from_utf8(plaintext).map_err(|_| RuntimeError::new("el campo descifrado no es UTF-8 válido"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; KEY_LEN] {
        [7u8; KEY_LEN]
    }

    #[test]
    fn encrypt_then_decrypt_round_trips_to_the_exact_original_plaintext() {
        let key = test_key();
        let stored = encrypt_field("123-45-6789", &key).unwrap();
        assert_eq!(decrypt_field(&stored, &key).unwrap(), "123-45-6789");
    }

    #[test]
    fn encrypting_the_same_plaintext_twice_never_produces_the_same_ciphertext() {
        // El nonce aleatorio es la garantía real -- si esto fallara, un
        // atacante con acceso de solo lectura a la base podría correlacionar
        // filas con el mismo valor en texto plano sin necesitar la clave.
        let key = test_key();
        let a = encrypt_field("mismo valor", &key).unwrap();
        let b = encrypt_field("mismo valor", &key).unwrap();
        assert_ne!(a, b, "dos cifrados del mismo texto plano no deben coincidir (nonce distinto cada vez)");
        assert_eq!(decrypt_field(&a, &key).unwrap(), "mismo valor");
        assert_eq!(decrypt_field(&b, &key).unwrap(), "mismo valor");
    }

    #[test]
    fn decrypting_with_the_wrong_key_fails_cleanly_not_with_a_panic() {
        let stored = encrypt_field("secreto", &test_key()).unwrap();
        let wrong_key = [9u8; KEY_LEN];
        assert!(decrypt_field(&stored, &wrong_key).is_err());
    }

    #[test]
    fn a_tampered_ciphertext_fails_authentication_instead_of_returning_garbage() {
        // La propiedad central de un AEAD sobre un cifrador simple: un byte
        // manipulado del lado de la base (no solo un problema de clave) se
        // detecta, en vez de descifrar en silencio a texto corrupto.
        use base64::Engine;
        let key = test_key();
        let stored = encrypt_field("secreto", &key).unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD.decode(&stored).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        let tampered = base64::engine::general_purpose::STANDARD.encode(raw);
        assert!(decrypt_field(&tampered, &key).is_err());
    }

    #[test]
    fn decrypting_a_value_shorter_than_a_nonce_fails_cleanly() {
        use base64::Engine;
        let too_short = base64::engine::general_purpose::STANDARD.encode(b"corto");
        assert!(decrypt_field(&too_short, &test_key()).is_err());
    }

    #[test]
    fn decrypting_invalid_base64_fails_cleanly() {
        assert!(decrypt_field("no es base64 %%%", &test_key()).is_err());
    }

    #[test]
    fn parse_encryption_key_accepts_exactly_32_bytes_in_standard_base64() {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode([1u8; 32]);
        assert_eq!(parse_encryption_key(&encoded).unwrap(), [1u8; 32]);
    }

    #[test]
    fn parse_encryption_key_rejects_the_wrong_length() {
        use base64::Engine;
        let too_short = base64::engine::general_purpose::STANDARD.encode([1u8; 16]);
        let err = parse_encryption_key(&too_short).unwrap_err();
        assert!(err.contains("32"), "{err}");
    }

    #[test]
    fn parse_encryption_key_rejects_invalid_base64() {
        assert!(parse_encryption_key("no es base64 %%%").is_err());
    }
}
