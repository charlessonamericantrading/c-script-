//! Auth v0 (GRAMMAR.md §3.14): sesión opaca en memoria + roles. Auth externo
//! (GRAMMAR.md §3.64): verificación de JWT HS256, para adoptar Link dentro
//! de una app con login ya existente sin correr dos sistemas de sesión en
//! paralelo.
//!
//! Se resolvió originalmente sin agregar dependencias, con lo que
//! tiny_http+serde_json ya daban; la única pieza que ese criterio dejaba
//! incómoda era la entropía del token (ver `fresh_128_bits`). Desde que
//! `crypto.hashPassword` pasó a Argon2id y el proyecto tomó `getrandom`, los
//! tokens de sesión salen del CSPRNG del sistema como corresponde. La
//! verificación de JWT (`verify_jwt`, más abajo) tampoco agrega ninguna
//! dependencia nueva: `hmac`/`sha2` ya estaban por `crypto.hmacSha256`, y
//! `base64` ya estaba por `base64.encode`/`decode`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// 128 bits del CSPRNG del sistema (BCryptGenRandom/ProcessPrng en Windows,
/// getrandom(2) en Linux, random_get en WASI).
///
/// HISTORIA, porque el reemplazo importa: esto llamaba a
/// RandomState::new() para sacar entropia. El hallazgo central de la ronda de
/// auth fue que eso NO da un secreto nuevo por llamada -- RandomState cachea
/// (k0,k1) por hilo la primera vez y despues solo incrementa k0, asi que en el
/// hilo principal (el interprete es single-threaded por diseno, ver
/// runtime/server.rs) cada token nuevo aportaba ~0 bits de entropia, no ~128.
/// La solucion de entonces fue spawnear un hilo descartable por token, porque
/// un hilo recien creado si pega contra el RNG del SO en su primer
/// RandomState::new(); funcionaba, pero era un rodeo alrededor de no tener una
/// dependencia de RNG. Ahora que crypto.hashPassword necesita sales de verdad,
/// getrandom entro al proyecto y este rodeo sobra: se pide directo al SO.
fn fresh_128_bits() -> (u64, u64) {
    let mut buf = [0u8; 16];
    // Si el SO no puede dar entropia, la respuesta correcta es cortar: emitir un
    // token de sesion "por defecto" seria emitir uno adivinable.
    getrandom::getrandom(&mut buf)
        .expect("el sistema no pudo generar entropia para un token de sesion");
    (
        u64::from_le_bytes(buf[..8].try_into().unwrap()),
        u64::from_le_bytes(buf[8..].try_into().unwrap()),
    )
}

fn fresh_token() -> String {
    let (a, b) = fresh_128_bits();
    format!("{a:016x}{b:016x}")
}

/// Verifica un JWT HS256 (RFC 7515/7519) contra `secret` y devuelve sus
/// claims si la firma es válida y no expiró (claim `exp`, si está presente).
///
/// SOLO HS256 -- cualquier otro `alg` (incluido `"none"`, la vulnerabilidad
/// de JWT más común y documentada que existe) se rechaza explícitamente.
/// Allowlist, no blocklist: la firma nunca se verifica con un algoritmo
/// distinto al que `secret` fue pensado para -- aceptar "el algoritmo que
/// diga el propio token" es exactamente el bug de confusión de algoritmo que
/// esto evita a propósito.
fn verify_jwt(token: &str, secret: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut parts = token.split('.');
    let header_b64 = parts.next()?;
    let payload_b64 = parts.next()?;
    let sig_b64 = parts.next()?;
    if parts.next().is_some() {
        return None; // más de 3 partes separadas por '.' -- no es un JWT
    }

    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header_bytes = engine.decode(header_b64).ok()?;
    let header: serde_json::Value = serde_json::from_slice(&header_bytes).ok()?;
    if header.get("alg").and_then(|v| v.as_str()) != Some("HS256") {
        return None;
    }

    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("{header_b64}.{payload_b64}").as_bytes());
    let expected_sig = mac.finalize().into_bytes();
    let actual_sig = engine.decode(sig_b64).ok()?;
    // Comparación en tiempo constante -- mismo motivo que `verifyPassword`
    // (GRAMMAR.md §3.34): un `==` de slices corta en el primer byte
    // distinto, filtrando cuánto de la firma esperada acertó quien prueba.
    if !super::constant_time_eq(&expected_sig, &actual_sig) {
        return None;
    }

    let payload_bytes = engine.decode(payload_b64).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    let claims = payload.as_object()?.clone();

    if let Some(exp) = claims.get("exp").and_then(|v| v.as_i64()) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
        if now >= exp {
            return None;
        }
    }

    Some(claims)
}

/// Firma un JWT HS256 de verdad -- inversa de `verify_jwt`, mismo esqueleto
/// que el helper de test `make_jwt` (más abajo, `#[cfg(test)]`) pero de
/// PRODUCCIÓN: GRAMMAR.md §3.203 (sesión MCP) es el primer caso donde este
/// servidor EMITE un JWT propio, no solo verifica uno externo (§3.64) --
/// `verify_jwt`/`sign_jwt` son deliberadamente funciones libres separadas
/// de `SessionStore`, sin compartir ningún estado, para que quede claro que
/// firmar y verificar son operaciones simétricas pero independientes (quien
/// firma no necesita `SessionStore.jwt`, que es la config de VERIFICACIÓN
/// de JWT externos).
fn sign_jwt(claims: &serde_json::Map<String, serde_json::Value>, secret: &str) -> String {
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header_b64 = engine.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload_b64 = engine.encode(serde_json::to_vec(claims).expect("un serde_json::Map siempre serializa"));
    let signing_input = format!("{header_b64}.{payload_b64}");
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC-SHA256 acepta cualquier longitud de clave");
    mac.update(signing_input.as_bytes());
    let sig_b64 = engine.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{sig_b64}")
}

/// `--jwt-secret`/`--jwt-role-claim`/`--jwt-user-id-claim` (o sus env vars),
/// resueltos UNA vez al arrancar (`main.rs::resolve_jwt_config`). `None` en
/// el `SessionStore` entero (el default): el comportamiento es IDÉNTICO al
/// de antes de esta ronda, cero JWT nunca se intenta verificar.
struct JwtConfig {
    secret: String,
    role_claim: String,
    user_id_claim: String,
}

/// Una sesión guardada -- rol como par de `String`, NO el `Value` completo
/// del intérprete (evita cualquier discusión sobre Send/Sync de `Value`, hoy
/// no es Send por `Value::Closure` que contiene `Rc`, y es exactamente lo
/// mismo que ya se serializa al wire para un enum simple), un `user_id`
/// opcional (`i64`), más CUÁNDO expira -- `None` significa "nunca" (comportamiento
/// de siempre, sin `--session-ttl`). Struct con nombres de campo, no una
/// tupla más grande -- mismo criterio que `RequestContext` (`db.rs`): más legible que posiciones.
struct SessionEntry {
    enum_name: String,
    variant_name: String,
    user_id: Option<i64>,
    expires_at: Option<Instant>,
}

/// Sesiones en memoria: token opaco -> `SessionEntry`.
///
/// `Mutex`, no `RefCell` -- Pilar 1 del roadmap de concurrencia (26/08/2026):
/// con un hilo por request (`runtime/server.rs`), dos requests pueden crear/
/// destruir/consultar sesiones AL MISMO TIEMPO de verdad. `parking_lot`, no
/// `std::sync` -- no necesita sostenerse a través de una llamada anidada
/// (a diferencia del candado de conexión de `Db`), así que el `Mutex` común
/// alcanza; se usa `parking_lot` en vez de `std::sync::Mutex` por
/// consistencia con el resto del roadmap (sin poisoning en pánico, API más
/// simple).
pub struct SessionStore {
    sessions: parking_lot::Mutex<HashMap<String, SessionEntry>>,
    /// Bloqueo de cuenta (GRAMMAR.md §3.152) -- timestamps de intentos
    /// fallidos por `identifier` (email, user id como String, lo que decida
    /// quien llama), más viejo primero para que podar por ventana sea
    /// barato (cortar del frente). `Mutex`, mismo criterio que `sessions`.
    failed_logins: parking_lot::Mutex<HashMap<String, std::collections::VecDeque<Instant>>>,
    /// Configurado UNA vez al construir el store (`--session-ttl`/
    /// `LINK_SESSION_TTL`, GRAMMAR.md §3.50) -- nunca cambia sesión a
    /// sesión, así que vivir en el store entero (no en cada `create`) es
    /// aditivo puro: todo caller existente que no pasa TTL (`linkc test`,
    /// los tests unitarios de este archivo) sigue viendo `None`, sin
    /// cambiar una sola firma.
    ttl: Option<Duration>,
    /// `Some` solo si `--jwt-secret`/`LINK_JWT_SECRET` está configurado
    /// (GRAMMAR.md §3.64) -- `None` es el default, comportamiento idéntico
    /// al de antes de esta ronda.
    jwt: Option<JwtConfig>,
    /// GRAMMAR.md §3.203 (MCP, Pieza A): `jti` de cada sesión MCP terminada
    /// explícitamente vía `DELETE /mcp`. Un JWT autocontenido no se puede
    /// invalidar antes de su propia expiración sin ALGO guardado del lado
    /// del servidor -- este set es ese "algo", deliberadamente chico (solo
    /// ids, nunca la sesión completa). Mismo molde que `failed_logins`
    /// arriba: `parking_lot::Mutex`, propio candado, nunca sostenido más
    /// allá de la operación puntual que lo pide.
    mcp_revoked_jti: parking_lot::Mutex<HashSet<String>>,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore {
            sessions: parking_lot::Mutex::new(HashMap::new()),
            failed_logins: parking_lot::Mutex::new(HashMap::new()),
            ttl: None,
            jwt: None,
            mcp_revoked_jti: parking_lot::Mutex::new(HashSet::new()),
        }
    }

    /// Como `new()`, pero cada sesión que este store cree expira `ttl`
    /// después de `create()` -- GRAMMAR.md §3.50. Único caller real:
    /// `runtime::server::serve`, cuando `--session-ttl`/`LINK_SESSION_TTL`
    /// están configurados.
    pub fn with_ttl(ttl: Duration) -> Self {
        SessionStore {
            sessions: parking_lot::Mutex::new(HashMap::new()),
            failed_logins: parking_lot::Mutex::new(HashMap::new()),
            ttl: Some(ttl),
            jwt: None,
            mcp_revoked_jti: parking_lot::Mutex::new(HashSet::new()),
        }
    }

    /// `auth.recordFailedLogin(identifier)` (GRAMMAR.md §3.152) -- agrega
    /// un timestamp AHORA a la lista de `identifier`. No poda acá (lo hace
    /// `failed_login_count`, con la ventana real que el caller recién en
    /// ESE momento decide) -- una entrada vieja sin consultar nunca más
    /// simplemente queda sin usarse hasta que algo la pode; un lockout real
    /// consulta seguido, así que esto no crece sin límite en la práctica.
    pub fn record_failed_login(&self, identifier: &str) {
        self.failed_logins.lock().entry(identifier.to_string()).or_default().push_back(Instant::now());
    }

    /// `auth.failedLoginCount(identifier, windowSeconds)` -- cuántos
    /// intentos fallidos quedan DENTRO de los últimos `window` (podando los
    /// que ya vencieron, del frente de la cola -- más viejo primero por
    /// construcción, así que cortar en el primer que todavía entra es
    /// correcto sin recorrer toda la lista).
    pub fn failed_login_count(&self, identifier: &str, window: Duration) -> i64 {
        let now = Instant::now();
        let mut map = self.failed_logins.lock();
        let Some(times) = map.get_mut(identifier) else { return 0 };
        while let Some(&front) = times.front() {
            if now.saturating_duration_since(front) > window {
                times.pop_front();
            } else {
                break;
            }
        }
        times.len() as i64
    }

    /// `auth.resetFailedLogins(identifier)` -- llamado tras un login
    /// EXITOSO (GRAMMAR.md §3.152): un intento bueno borra el historial de
    /// fallos previos, no los deja acumulándose contra un usuario legítimo
    /// que solo se equivocó de contraseña un par de veces.
    pub fn reset_failed_logins(&self, identifier: &str) {
        self.failed_logins.lock().remove(identifier);
    }

    /// Habilita verificar JWTs externos ADEMÁS de las sesiones propias de
    /// este store (GRAMMAR.md §3.64) -- builder consumidor, se encadena con
    /// `new()`/`with_ttl()`: `SessionStore::with_ttl(d).with_jwt(...)`. Las
    /// dos cosas conviven a propósito -- una migración real no reemplaza su
    /// login existente de un día para el otro, y `auth.createSession(WithId)`
    /// sigue funcionando exactamente igual para cualquier endpoint nuevo
    /// escrito directamente en c-script.
    pub fn with_jwt(mut self, secret: String, role_claim: String, user_id_claim: String) -> Self {
        self.jwt = Some(JwtConfig { secret, role_claim, user_id_claim });
        self
    }

    pub fn create(&self, enum_name: String, variant_name: String) -> String {
        self.create_with_user_id(enum_name, variant_name, None)
    }

    pub fn create_with_user_id(&self, enum_name: String, variant_name: String, user_id: Option<i64>) -> String {
        let token = fresh_token();
        let expires_at = self.ttl.map(|ttl| Instant::now() + ttl);
        self.sessions.lock().insert(token.clone(), SessionEntry { enum_name, variant_name, user_id, expires_at });
        token
    }

    /// Idempotente -- destruir una sesión que ya no existe (o nunca existió)
    /// no es un error.
    pub fn destroy(&self, token: &str) {
        self.sessions.lock().remove(token);
    }

    /// Destruye TODAS las sesiones abiertas con ese `user_id` (GRAMMAR.md
    /// §3.84) -- a diferencia de `destroy` (que opera sobre `current_token`,
    /// la sesión que ya autenticó la request actual), esta SÍ toma un
    /// identificador como argumento: `user_id` no es un secreto adivinable
    /// como un token de sesión, es una clave de aplicación (mismo criterio
    /// que ya vale para `createSessionWithId`, que también recibe un
    /// `user_id` explícito). Quién puede LLAMAR a esto es responsabilidad
    /// de quien escribe el `.link` -- típicamente gateado con
    /// `@requires(Role.Admin)` en el rpc que lo envuelve, este método no
    /// impone ninguna política propia. Devuelve cuántas sesiones se
    /// borraron -- 0 si el usuario no tenía ninguna sesión abierta (nunca un
    /// error). Cada sesión JWT externa (GRAMMAR.md §3.64), si las hay, no
    /// pasa por acá -- este store nunca las guardó, no hay nada que borrar.
    pub fn destroy_all_for_user(&self, user_id: i64) -> usize {
        let mut sessions = self.sessions.lock();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.user_id != Some(user_id));
        before - sessions.len()
    }

    /// `None` tanto para un token que nunca existió como para uno que ya
    /// expiró -- desde afuera de este módulo las dos cosas son
    /// indistinguibles a propósito (el mismo 401 "se requiere
    /// autenticación" para ambas, `check_auth_gate` en `runtime/server.rs`,
    /// nunca revela CUÁL de las dos pasó). Un token expirado se BORRA acá
    /// -- limpieza perezosa en el próximo acceso, no un barrido de fondo:
    /// este intérprete no tiene ningún hilo de mantenimiento (single-
    /// threaded por diseño), así que "en el próximo acceso" es la única
    /// oportunidad real de liberar la memoria de una sesión vencida sin
    /// inventar un timer aparte.
    ///
    /// Sesión propia primero; si el token no está ahí Y hay `jwt`
    /// configurado (GRAMMAR.md §3.64), lo intenta como JWT externo --
    /// `verify_jwt` ya devuelve `None` sola para cualquier string que no
    /// tenga la forma de un JWT (un token opaco de este mismo store, hex,
    /// nunca tiene un '.'), así que no hace falta distinguir los dos casos
    /// antes de intentar. Para un JWT válido, el `enum_name` devuelto es
    /// `""` -- a propósito: un token externo no tiene ningún enum de
    /// c-script asociado, así que no hay identidad de enum que devolver.
    /// `check_auth_gate` (`runtime/server.rs`) sabe leer ese sentinel:
    /// matchea el `@requires` por NOMBRE de variante nada más, sin la
    /// comparación de identidad de enum que sí aplica a una sesión creada
    /// por este mismo programa.
    pub fn role_for(&self, token: &str) -> Option<(String, String)> {
        {
            let mut sessions = self.sessions.lock();
            if let Some(entry) = sessions.get(token) {
                if entry.expires_at.is_some_and(|exp| Instant::now() >= exp) {
                    sessions.remove(token);
                    return None;
                }
                return Some((entry.enum_name.clone(), entry.variant_name.clone()));
            }
        }
        let jwt = self.jwt.as_ref()?;
        let claims = verify_jwt(token, &jwt.secret)?;
        let variant = claims.get(&jwt.role_claim)?.as_str()?.to_string();
        Some((String::new(), variant))
    }

    /// Devuelve el `user_id` guardado en la sesión (`Some(id)` si se creó con
    /// `create_with_user_id` o `createSessionWithId`, `None` si se creó sin id,
    /// si no existe o si ya expiró) -- o, igual que `role_for`, el claim
    /// configurado de un JWT externo válido. Acepta el claim como número JSON
    /// O como string de dígitos (`"sub": "42"`), el formato más común en JWTs
    /// reales -- `sub` casi siempre es string por convención de OIDC.
    pub fn user_id_for(&self, token: &str) -> Option<i64> {
        {
            let mut sessions = self.sessions.lock();
            if let Some(entry) = sessions.get(token) {
                if entry.expires_at.is_some_and(|exp| Instant::now() >= exp) {
                    sessions.remove(token);
                    return None;
                }
                return entry.user_id;
            }
        }
        let jwt = self.jwt.as_ref()?;
        let claims = verify_jwt(token, &jwt.secret)?;
        match claims.get(&jwt.user_id_claim)? {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.parse::<i64>().ok(),
            _ => None,
        }
    }

    /// GRAMMAR.md §3.197: accessor genérico de un claim JWT por NOMBRE --
    /// mismo mecanismo exacto que `role_for`/`user_id_for` (mismo mapa
    /// COMPLETO que `verify_jwt` ya devuelve, nada se re-parsea), pero el
    /// nombre del claim llega en cada llamada en vez de fijarse una vez al
    /// arrancar (a diferencia de `--jwt-role-claim`/`--jwt-user-id-claim`,
    /// que fijan un nombre para siempre en un slot de significado fijo).
    ///
    /// Una sesión INTERNA (creada por este mismo programa) nunca tiene un
    /// mapa de claims genérico -- solo `enum_name`/`variant_name`/`user_id`
    /// -- así que da `None` explícitamente para un token interno, reflejando
    /// a propósito el mismo criterio que `role_for`/`user_id_for` ya
    /// aplican, en vez de asumir que nunca se va a llamar así.
    ///
    /// Conversión a `String` consciente del caso real (revocar un token
    /// comparando un claim `tokenVersion` contra el valor real en DB), no
    /// una repr JSON genérica: un `Number` sin parte fraccionaria se
    /// imprime como `"3"`, no `"3.0"` -- así `auth.claim("tokenVersion") ==
    /// user.tokenVersion.toString()` compara igual sin importar si el
    /// emisor del JWT serializó el entero como float. `Object`/`Array`/
    /// `Null`/claim ausente -> `None`, sin una forma plana sensata.
    pub fn claim_for(&self, token: &str, name: &str) -> Option<String> {
        {
            let mut sessions = self.sessions.lock();
            if let Some(entry) = sessions.get(token) {
                if entry.expires_at.is_some_and(|exp| Instant::now() >= exp) {
                    sessions.remove(token);
                }
                return None;
            }
        }
        let jwt = self.jwt.as_ref()?;
        let claims = verify_jwt(token, &jwt.secret)?;
        match claims.get(name)? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.as_i64().map(|i| i.to_string()).unwrap_or_else(|| n.to_string())),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// GRAMMAR.md §3.203 (MCP, Pieza A) -- `initialize` firma un JWT propio
    /// de sesión MCP, embebiendo el MISMO rol/`user_id` que ya autenticó la
    /// request (un `Authorization: Bearer` normal, resuelto vía
    /// `role_for`/`user_id_for` ANTES de llamar acá) -- así un `tools/call`
    /// posterior autentica el `rpc` subyacente con el header
    /// `Mcp-Session-Id` solo, sin pedir un segundo token. `jti` (128 bits
    /// del mismo CSPRNG que `fresh_token`) es lo único que
    /// `revoke_mcp_session` necesita guardar para poder terminar esta
    /// sesión antes de que expire sola.
    pub fn sign_mcp_session(&self, role: &str, user_id: Option<i64>, secret: &str) -> String {
        let jti = fresh_token();
        let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 + MCP_SESSION_TTL_SECS;
        let mut claims = serde_json::Map::new();
        claims.insert("jti".to_string(), serde_json::json!(jti));
        claims.insert("role".to_string(), serde_json::json!(role));
        if let Some(uid) = user_id {
            claims.insert("sub".to_string(), serde_json::json!(uid));
        }
        claims.insert("exp".to_string(), serde_json::json!(exp));
        sign_jwt(&claims, secret)
    }

    /// Verifica un `Mcp-Session-Id` -- firma/expiración vía `verify_jwt`
    /// (mismo camino que un JWT externo, pero con `secret` propio de MCP,
    /// nunca `self.jwt`), MÁS el chequeo de revocación que un JWT
    /// autocontenido no puede hacer solo. `None` para cualquier falla --
    /// firma inválida, expirado, revocado, o sin los claims que
    /// `sign_mcp_session` siempre pone -- indistinguibles desde afuera, mismo
    /// criterio que `role_for` ya aplica para una sesión normal.
    pub fn verify_mcp_session(&self, token: &str, secret: &str) -> Option<(String, Option<i64>, String)> {
        let claims = verify_jwt(token, secret)?;
        let jti = claims.get("jti")?.as_str()?.to_string();
        if self.mcp_revoked_jti.lock().contains(&jti) {
            return None;
        }
        let role = claims.get("role")?.as_str()?.to_string();
        let user_id = match claims.get("sub") {
            Some(serde_json::Value::Number(n)) => n.as_i64(),
            Some(serde_json::Value::String(s)) => s.parse::<i64>().ok(),
            _ => None,
        };
        Some((role, user_id, jti))
    }

    /// `DELETE /mcp` -- idempotente, mismo criterio que `destroy`: revocar
    /// un `jti` ya revocado (o uno que nunca existió) no es un error.
    pub fn revoke_mcp_session(&self, jti: &str) {
        self.mcp_revoked_jti.lock().insert(jti.to_string());
    }
}

/// GRAMMAR.md §3.203: sin flag/env var propio en v1 -- alcance angosto a
/// propósito, mismo criterio que otros límites v1 de esta ronda (PDF/Excel,
/// GRAMMAR.md §3.201/§3.202). Una hora alcanza para una sesión de trabajo
/// real con un cliente MCP; renovar es tan simple como llamar `initialize`
/// de nuevo.
const MCP_SESSION_TTL_SECS: i64 = 3600;

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_login_count_is_zero_for_an_identifier_never_recorded() {
        let store = SessionStore::new();
        assert_eq!(store.failed_login_count("nadie@x.com", Duration::from_secs(900)), 0);
    }

    #[test]
    fn record_failed_login_accumulates_within_the_window() {
        let store = SessionStore::new();
        store.record_failed_login("a@x.com");
        store.record_failed_login("a@x.com");
        store.record_failed_login("a@x.com");
        assert_eq!(store.failed_login_count("a@x.com", Duration::from_secs(900)), 3);
        // Otro identifier, contador propio.
        assert_eq!(store.failed_login_count("b@x.com", Duration::from_secs(900)), 0);
    }

    #[test]
    fn failed_login_count_excludes_attempts_outside_the_window() {
        let store = SessionStore::new();
        store.record_failed_login("a@x.com");
        // Ventana de 0 segundos: el intento que se acaba de grabar ya está
        // "afuera" (el chequeo es `> window`, así que cualquier duración
        // positiva transcurrida excede una ventana de 0).
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(store.failed_login_count("a@x.com", Duration::from_secs(0)), 0);
    }

    #[test]
    fn reset_failed_logins_clears_the_count() {
        let store = SessionStore::new();
        store.record_failed_login("a@x.com");
        store.record_failed_login("a@x.com");
        store.reset_failed_logins("a@x.com");
        assert_eq!(store.failed_login_count("a@x.com", Duration::from_secs(900)), 0);
    }

    #[test]
    fn create_then_role_for_round_trips() {
        let store = SessionStore::new();
        let token = store.create("Role".to_string(), "Admin".to_string());
        assert_eq!(store.role_for(&token), Some(("Role".to_string(), "Admin".to_string())));
    }

    #[test]
    fn unknown_token_has_no_role() {
        let store = SessionStore::new();
        assert_eq!(store.role_for("no-existe"), None);
    }

    #[test]
    fn destroy_removes_the_session() {
        let store = SessionStore::new();
        let token = store.create("Role".to_string(), "Admin".to_string());
        store.destroy(&token);
        assert_eq!(store.role_for(&token), None);
    }

    #[test]
    fn destroying_an_unknown_token_does_not_panic() {
        let store = SessionStore::new();
        store.destroy("no-existe"); // no debería paniquear
    }

    // ---- revocar TODAS las sesiones de un usuario (GRAMMAR.md §3.84) ----

    #[test]
    fn destroy_all_for_user_removes_every_session_of_that_user_and_returns_the_count() {
        let store = SessionStore::new();
        let a1 = store.create_with_user_id("Role".to_string(), "Admin".to_string(), Some(1));
        let a2 = store.create_with_user_id("Role".to_string(), "Admin".to_string(), Some(1));
        let b1 = store.create_with_user_id("Role".to_string(), "Member".to_string(), Some(2));

        let removed = store.destroy_all_for_user(1);
        assert_eq!(removed, 2, "user 1 tenía exactamente 2 sesiones abiertas");
        assert_eq!(store.role_for(&a1), None, "sesión de user 1 debe estar borrada");
        assert_eq!(store.role_for(&a2), None, "la SEGUNDA sesión de user 1 también");
        assert_eq!(store.role_for(&b1), Some(("Role".to_string(), "Member".to_string())), "user 2 no debe verse afectado");
    }

    #[test]
    fn destroy_all_for_user_with_no_sessions_returns_zero_without_touching_anything() {
        let store = SessionStore::new();
        let other = store.create_with_user_id("Role".to_string(), "Admin".to_string(), Some(1));
        assert_eq!(store.destroy_all_for_user(999), 0, "user 999 nunca tuvo sesiones");
        assert!(store.role_for(&other).is_some(), "no debería haber tocado la sesión de otro usuario");
    }

    #[test]
    fn a_session_created_without_a_user_id_is_never_matched_by_destroy_all_for_user() {
        // `create` (sin id) guarda `user_id: None` -- ninguna llamada con un
        // `user_id` real (`Some(_)`) debería poder alcanzarla.
        let store = SessionStore::new();
        let anonymous = store.create("Role".to_string(), "Admin".to_string());
        assert_eq!(store.destroy_all_for_user(0), 0);
        assert!(store.role_for(&anonymous).is_some());
    }

    #[test]
    fn tokens_are_128_bits_of_hex_and_distinct_across_many_calls() {
        // No prueba "es criptográficamente seguro" (no se puede probar con
        // un test) -- prueba las dos propiedades mecánicas que si fallaran
        // serían un bug real: longitud (32 hex = 128 bits) y que llamadas
        // repetidas no devuelvan siempre el mismo valor.
        let store = SessionStore::new();
        let tokens: Vec<String> =
            (0..20).map(|_| store.create("Role".to_string(), "Admin".to_string())).collect();
        for t in &tokens {
            assert_eq!(t.len(), 32, "token debería ser 32 caracteres hex (128 bits): {t}");
            assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        }
        let unique: std::collections::HashSet<&String> = tokens.iter().collect();
        assert_eq!(unique.len(), tokens.len(), "20 tokens generados no deberían tener duplicados");
    }

    #[test]
    fn a_store_without_ttl_never_expires_a_session() {
        // Comportamiento de siempre, sin `--session-ttl`: `new()` sigue sin
        // TTL -- esta ronda no le cambia nada a nadie que no pida esto
        // explícitamente.
        let store = SessionStore::new();
        let token = store.create("Role".to_string(), "Admin".to_string());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(store.role_for(&token), Some(("Role".to_string(), "Admin".to_string())));
    }

    #[test]
    fn a_session_is_valid_before_the_ttl_and_gone_after_it() {
        let store = SessionStore::with_ttl(Duration::from_millis(30));
        let token = store.create("Role".to_string(), "Admin".to_string());
        assert_eq!(
            store.role_for(&token),
            Some(("Role".to_string(), "Admin".to_string())),
            "recién creada, todavía no expiró"
        );
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(store.role_for(&token), None, "pasado el TTL, la sesión ya no es válida");
    }

    #[test]
    fn an_expired_session_is_indistinguishable_from_one_that_never_existed() {
        // GRAMMAR.md §3.50: `check_auth_gate` da el mismo 401 para los dos
        // casos -- el contrato real es que `role_for` tampoco los
        // distinga, así que un caller no puede armar ese distingo por
        // accidente comparando `Option` contra algo más específico.
        let store = SessionStore::with_ttl(Duration::from_millis(10));
        let token = store.create("Role".to_string(), "Admin".to_string());
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(store.role_for(&token), store.role_for("un-token-que-nunca-existio"));
    }

    #[test]
    fn create_with_user_id_persists_and_returns_user_id() {
        let store = SessionStore::new();
        let token_with_id = store.create_with_user_id("Role".to_string(), "Member".to_string(), Some(42));
        assert_eq!(store.user_id_for(&token_with_id), Some(42));
        assert_eq!(store.role_for(&token_with_id), Some(("Role".to_string(), "Member".to_string())));

        let token_without_id = store.create("Role".to_string(), "Admin".to_string());
        assert_eq!(store.user_id_for(&token_without_id), None);
        assert_eq!(store.role_for(&token_without_id), Some(("Role".to_string(), "Admin".to_string())));

        assert_eq!(store.user_id_for("token-inexistente"), None);
    }

    #[test]
    fn user_id_for_expires_with_ttl() {
        let store = SessionStore::with_ttl(Duration::from_millis(20));
        let token = store.create_with_user_id("Role".to_string(), "Member".to_string(), Some(101));
        assert_eq!(store.user_id_for(&token), Some(101));
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(store.user_id_for(&token), None);
    }

    /// Arma un JWT HS256 DE VERDAD -- mismo algoritmo que produciría
    /// `jsonwebtoken` de Node o `PyJWT`, no un atajo interno -- para probar
    /// que `verify_jwt` interopera con lo que un backend externo real
    /// emitiría, no solo con su propio round-trip.
    fn make_jwt(secret: &str, alg: &str, claims_json: &str) -> String {
        use base64::Engine;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = format!(r#"{{"alg":"{alg}","typ":"JWT"}}"#);
        let header_b64 = engine.encode(header.as_bytes());
        let payload_b64 = engine.encode(claims_json.as_bytes());
        let signing_input = format!("{header_b64}.{payload_b64}");
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(signing_input.as_bytes());
        let sig_b64 = engine.encode(mac.finalize().into_bytes());
        format!("{signing_input}.{sig_b64}")
    }

    #[test]
    fn a_valid_jwt_resolves_role_and_user_id() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":42}"#);
        assert_eq!(store.role_for(&jwt), Some(("".to_string(), "Admin".to_string())), "enum_name vacío: sentinel de JWT externo");
        assert_eq!(store.user_id_for(&jwt), Some(42));
    }

    #[test]
    fn a_string_sub_claim_parses_as_int() {
        // "sub" como string es la convención real de OIDC -- si esto no
        // parseara, la mayoría de los JWTs emitidos por proveedores de auth
        // reales (Auth0, Clerk, Firebase) no funcionarían nunca.
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Member","sub":"7"}"#);
        assert_eq!(store.user_id_for(&jwt), Some(7));
    }

    #[test]
    fn configurable_claim_names_are_honored() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "perfil".to_string(), "usuarioId".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"perfil":"Admin","usuarioId":5}"#);
        assert_eq!(store.role_for(&jwt), Some(("".to_string(), "Admin".to_string())));
        assert_eq!(store.user_id_for(&jwt), Some(5));
    }

    #[test]
    fn a_jwt_signed_with_a_different_secret_is_rejected() {
        let store = SessionStore::new().with_jwt("secreto-real".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("secreto-adivinado", "HS256", r#"{"role":"Admin","sub":1}"#);
        assert_eq!(store.role_for(&jwt), None, "la firma no matchea, tiene que rechazarse");
    }

    #[test]
    fn alg_none_is_rejected_even_with_a_technically_matching_signature() {
        // La vulnerabilidad de JWT más común y documentada: aceptar
        // `"alg":"none"` (sin firma) porque el verificador confía en lo que
        // el token DICE que es su propio algoritmo. Acá se rechaza ANTES de
        // siquiera calcular una firma esperada.
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "none", r#"{"role":"Admin","sub":1}"#);
        assert_eq!(store.role_for(&jwt), None);
    }

    #[test]
    fn an_unsupported_algorithm_is_rejected() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "RS256", r#"{"role":"Admin","sub":1}"#);
        assert_eq!(store.role_for(&jwt), None, "solo HS256 -- allowlist, no blocklist");
    }

    #[test]
    fn an_expired_jwt_is_rejected() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":1,"exp":1}"#); // exp: 1 segundo después del epoch
        assert_eq!(store.role_for(&jwt), None, "exp en el pasado tiene que rechazarse");
    }

    #[test]
    fn a_jwt_without_an_exp_claim_never_expires() {
        // "exp" es OPCIONAL en el spec (RFC 7519) -- ausente no significa
        // "inválido", significa "sin vencimiento".
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":1}"#);
        assert!(store.role_for(&jwt).is_some());
    }

    #[test]
    fn garbage_input_does_not_panic() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        assert_eq!(store.role_for("ni-siquiera-tiene-puntos"), None);
        assert_eq!(store.role_for("a.b"), None, "solo 2 partes, no 3");
        assert_eq!(store.role_for("a.b.c.d"), None, "4 partes, no 3");
        assert_eq!(store.role_for("no-es-base64!.no-es-base64!.no-es-base64!"), None);
        assert_eq!(store.role_for(""), None);
    }

    #[test]
    fn without_jwt_configured_a_jwt_shaped_token_is_just_unknown() {
        // El comportamiento por default (sin --jwt-secret): idéntico al de
        // antes de esta ronda, un token con FORMA de JWT no se verifica --
        // no hay secreto contra el cual hacerlo.
        let store = SessionStore::new();
        let jwt = make_jwt("cualquier-cosa", "HS256", r#"{"role":"Admin","sub":1}"#);
        assert_eq!(store.role_for(&jwt), None);
    }

    #[test]
    fn an_internal_session_takes_precedence_over_jwt_verification() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let token = store.create_with_user_id("Role".to_string(), "Agent".to_string(), Some(99));
        // El token interno NUNCA tiene forma de JWT (hex, sin '.'), así que
        // esto en la práctica siempre cae al camino de sesión propia -- este
        // test lo deja explícito igual.
        assert_eq!(store.role_for(&token), Some(("Role".to_string(), "Agent".to_string())));
        assert_eq!(store.user_id_for(&token), Some(99));
    }

    // ---- GRAMMAR.md §3.197: auth.claim(name) ----

    #[test]
    fn claim_for_reads_a_string_claim() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":1,"plan":"pro"}"#);
        assert_eq!(store.claim_for(&jwt, "plan"), Some("pro".to_string()));
    }

    /// El caso real motivador (MyFinance): `tokenVersion` es un número
    /// entero en el JWT -- tiene que imprimirse como `"3"`, NUNCA `"3.0"`,
    /// para que `auth.claim("tokenVersion") == user.tokenVersion.toString()`
    /// compare igual sin importar cómo el emisor serializó el entero.
    #[test]
    fn claim_for_stringifies_an_integer_valued_number_without_a_trailing_decimal() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":1,"tokenVersion":3}"#);
        assert_eq!(store.claim_for(&jwt, "tokenVersion"), Some("3".to_string()), "no '3.0'");
    }

    #[test]
    fn claim_for_stringifies_a_genuinely_fractional_number() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":1,"score":3.5}"#);
        assert_eq!(store.claim_for(&jwt, "score"), Some("3.5".to_string()));
    }

    #[test]
    fn claim_for_stringifies_a_bool_claim() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":1,"verified":true}"#);
        assert_eq!(store.claim_for(&jwt, "verified"), Some("true".to_string()));
    }

    #[test]
    fn claim_for_gives_none_for_an_absent_claim_or_a_non_scalar_value() {
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let jwt = make_jwt("shh", "HS256", r#"{"role":"Admin","sub":1,"meta":{"a":1},"tags":[1,2]}"#);
        assert_eq!(store.claim_for(&jwt, "noExiste"), None);
        assert_eq!(store.claim_for(&jwt, "meta"), None, "un objeto no tiene una forma plana sensata");
        assert_eq!(store.claim_for(&jwt, "tags"), None, "un array tampoco");
    }

    #[test]
    fn claim_for_gives_none_for_an_internal_session_token() {
        // Una sesión interna nunca tiene un mapa de claims genérico -- solo
        // enum_name/variant_name/user_id -- reflejado explícitamente, no
        // asumido.
        let store = SessionStore::new().with_jwt("shh".to_string(), "role".to_string(), "sub".to_string());
        let token = store.create_with_user_id("Role".to_string(), "Agent".to_string(), Some(99));
        assert_eq!(store.claim_for(&token, "anything"), None);
    }

    #[test]
    fn claim_for_gives_none_without_jwt_configured() {
        let store = SessionStore::new();
        let jwt = make_jwt("cualquier-cosa", "HS256", r#"{"role":"Admin","sub":1,"plan":"pro"}"#);
        assert_eq!(store.claim_for(&jwt, "plan"), None);
    }
}

