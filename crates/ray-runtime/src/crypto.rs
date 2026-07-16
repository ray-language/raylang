//! **Cripto de producción** (M43, extraído a `ray-runtime` en P2.b). Envuelve `ring` (tiempo constante,
//! auditado) tras la feature `crypto`. Determinista (misma entrada → misma salida) → el oráculo VM↔nativo
//! se mantiene. Sin la feature, cada función es un *stub* inofensivo (vacío/`None`/`false`), como el build
//! slim / wasm del binario `ray`: un programa que use cripto recibe un resultado vacío (el *gating* por
//! checker del consumidor evita alcanzarlo).
//!
//! Las firmas trabajan con `&[u8]`/`Vec<u8>` — el marshalling desde/hacia el modelo de valores (VM) o los
//! `Rc<[u8]>` (binario transpilado) lo hace cada consumidor en su borde.

/// `n` octetos **criptográficamente seguros** (`ring::rand::SystemRandom`, el CSPRNG del SO). Para
/// tokens/salts/nonces — un PRNG sembrado del reloj es predecible. `n <= 0` → vacío (total).
#[cfg(feature = "crypto")]
pub fn crypto_random_bytes(n: i64) -> Vec<u8> {
    use ring::rand::SecureRandom;
    if n <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; n as usize];
    ring::rand::SystemRandom::new().fill(&mut buf).expect("the OS CSPRNG should not fail");
    buf
}
#[cfg(not(feature = "crypto"))]
pub fn crypto_random_bytes(_n: i64) -> Vec<u8> { Vec::new() }

/// SHA-256 (32 octetos). El caballo de batalla de HMAC/JWT/firmas.
#[cfg(feature = "crypto")]
pub fn sha256(data: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, data).as_ref().to_vec()
}
#[cfg(not(feature = "crypto"))]
pub fn sha256(_data: &[u8]) -> Vec<u8> { Vec::new() }

/// SHA-512 (64 octetos).
#[cfg(feature = "crypto")]
pub fn sha512(data: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA512, data).as_ref().to_vec()
}
#[cfg(not(feature = "crypto"))]
pub fn sha512(_data: &[u8]) -> Vec<u8> { Vec::new() }

/// SHA-1 (20 octetos). `ring` lo nombra `..._FOR_LEGACY_USE_ONLY`: roto para seguridad, se expone SOLO
/// para protocolos que aún lo exigen por diseño (p. ej. el accept-key de WebSocket, RFC 6455).
#[cfg(feature = "crypto")]
pub fn sha1(data: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, data).as_ref().to_vec()
}
#[cfg(not(feature = "crypto"))]
pub fn sha1(_data: &[u8]) -> Vec<u8> { Vec::new() }

/// HMAC-SHA256 (32 octetos): MAC con clave, la base de JWT (HS256), SigV4 y muchos esquemas de auth. La
/// verificación honesta se hace **recomputando** el MAC y comparando en tiempo constante — responsabilidad
/// de quien compara; aquí solo se produce la etiqueta.
#[cfg(feature = "crypto")]
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::sign(&k, msg).as_ref().to_vec()
}
#[cfg(not(feature = "crypto"))]
pub fn hmac_sha256(_key: &[u8], _msg: &[u8]) -> Vec<u8> { Vec::new() }

// --- Ed25519 (firma de curva elíptica, M43.3) ---
//
// La semilla privada es de **exactamente 32 octetos**; `ring` falla si no. Devolvemos `Option` (→ el
// consumidor etiqueta `[]`/`[valor]` y su prelude lo envuelve): un tamaño de semilla malo es un dato
// inválido, no un ICE. `verify` es **total** (nunca falla; da `false` ante clave/firma inválidas).

/// Clave pública (32 octetos) derivada de una semilla de 32 octetos. `None` si la semilla no mide 32.
#[cfg(feature = "crypto")]
pub fn ed25519_public_key(seed: &[u8]) -> Option<Vec<u8>> {
    use ring::signature::KeyPair;
    ring::signature::Ed25519KeyPair::from_seed_unchecked(seed)
        .ok()
        .map(|kp| kp.public_key().as_ref().to_vec())
}
#[cfg(not(feature = "crypto"))]
pub fn ed25519_public_key(_seed: &[u8]) -> Option<Vec<u8>> { None }

/// Firma (64 octetos) de `msg` con la semilla de 32 octetos. `None` si la semilla no mide 32. Ed25519 es
/// **determinista** (RFC 8032: el nonce se deriva por hash) → misma entrada, misma firma → el oráculo vale.
#[cfg(feature = "crypto")]
pub fn ed25519_sign(seed: &[u8], msg: &[u8]) -> Option<Vec<u8>> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(seed)
        .ok()
        .map(|kp| kp.sign(msg).as_ref().to_vec())
}
#[cfg(not(feature = "crypto"))]
pub fn ed25519_sign(_seed: &[u8], _msg: &[u8]) -> Option<Vec<u8>> { None }

/// Verifica que `sig` es una firma de `msg` bajo `pubkey`. Total: `false` ante cualquier entrada inválida.
#[cfg(feature = "crypto")]
pub fn ed25519_verify(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pubkey)
        .verify(msg, sig)
        .is_ok()
}
#[cfg(not(feature = "crypto"))]
pub fn ed25519_verify(_pubkey: &[u8], _msg: &[u8], _sig: &[u8]) -> bool { false }

// --- ChaCha20-Poly1305 AEAD (cifrado autenticado, M43.4) ---
//
// La clave son 32 octetos y el nonce 12; `ring` falla si no. `seal` devuelve `texto_cifrado || etiqueta`
// (la etiqueta de 16 octetos va anexada); `open` la verifica y devuelve el texto plano, o `None` si la
// autenticación falla (dato manipulado) o los tamaños no cuadran. Usamos `LessSafeKey` porque el nonce lo
// aporta quien llama (la API "segura" de `ring` gestiona el nonce por secuencia; aquí es de más bajo nivel).

/// Cifra y autentica `plaintext` con `key` (32) y `nonce` (12), ligando `aad` (datos autenticados no
/// cifrados). Devuelve `texto_cifrado || etiqueta(16)`; `None` si `key`/`nonce` no miden lo debido.
#[cfg(feature = "crypto")]
pub fn chacha20poly1305_seal(key: &[u8], nonce: &[u8], aad: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    let unbound = ring::aead::UnboundKey::new(&ring::aead::CHACHA20_POLY1305, key).ok()?;
    let key = ring::aead::LessSafeKey::new(unbound);
    let nonce = ring::aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut in_out = plaintext.to_vec();
    key.seal_in_place_append_tag(nonce, ring::aead::Aad::from(aad), &mut in_out).ok()?;
    Some(in_out)
}
#[cfg(not(feature = "crypto"))]
pub fn chacha20poly1305_seal(_key: &[u8], _nonce: &[u8], _aad: &[u8], _plaintext: &[u8]) -> Option<Vec<u8>> { None }

/// Descifra y verifica `ciphertext_and_tag` (`texto_cifrado || etiqueta`) con `key`/`nonce`/`aad`. Devuelve
/// el texto plano, o `None` si la autenticación falla (manipulación) o los tamaños no cuadran.
#[cfg(feature = "crypto")]
pub fn chacha20poly1305_open(key: &[u8], nonce: &[u8], aad: &[u8], ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> {
    let unbound = ring::aead::UnboundKey::new(&ring::aead::CHACHA20_POLY1305, key).ok()?;
    let key = ring::aead::LessSafeKey::new(unbound);
    let nonce = ring::aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut in_out = ciphertext_and_tag.to_vec();
    let plaintext = key.open_in_place(nonce, ring::aead::Aad::from(aad), &mut in_out).ok()?;
    Some(plaintext.to_vec())
}
#[cfg(not(feature = "crypto"))]
pub fn chacha20poly1305_open(_key: &[u8], _nonce: &[u8], _aad: &[u8], _ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> { None }
