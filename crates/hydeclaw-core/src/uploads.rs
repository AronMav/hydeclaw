//! Phase 64 SEC-03 — HMAC-signed URL mint/verify for `/uploads/*`.
//!
//! # Why
//! Agents routinely embed uploaded media in responses (e.g. Telegram
//! `send_photo` tool result). Before this module, anyone who learned an
//! upload UUID could fetch the file forever. HMAC + TTL limits the blast
//! radius; constant-time compare closes the timing side channel.
//!
//! # Contract
//! * URL format: `{base}/uploads/{filename}?sig={b64url-nopad}&exp={unix}`
//! * Signature payload: `"{filename}:{exp_unix}"` (bytes).
//! * HMAC algorithm: `HMAC-SHA256` with a 32-byte key.
//! * Key derivation: `HKDF-SHA256(ikm = master_key, salt = None, info = b"uploads-v1")`.
//!   Using `info` as a domain separator lets us later rotate to `"uploads-v2"`
//!   (or mint other per-domain keys like `"session-v1"`) without touching the
//!   master key.
//! * Base64 alphabet: `URL_SAFE_NO_PAD` (matches `tests/support/signed_url_helper.rs`).
//!
//! # Leaf-ness
//! This module has zero `crate::*` references. It pulls only `std`, `base64`,
//! `hmac`, `sha2`, `hkdf`, `subtle`, and `thiserror`. That lets
//! `src/lib.rs` re-export it for integration tests without cascading the
//! 10-module lib-facade cap (see `src/lib.rs` for the budgeting comment).

use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// Parsed `?sig=&exp=` query parameters.
///
/// Axum extractors can be used upstream; this struct keeps the leaf module
/// free of axum-specific types so it compiles in `lib.rs` without the gateway
/// cascade.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SignedUploadQuery {
    pub sig: Option<String>,
    pub exp: Option<u64>,
}

/// Verification outcome for `/uploads/{file}` requests.
///
/// Mapping to HTTP:
///   * `Missing` → 403 Forbidden (only when `require_signature=true`)
///   * `Invalid` → 403 Forbidden
///   * `Expired` → 410 Gone
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum UploadSignatureError {
    #[error("missing signature")]
    Missing,
    #[error("invalid signature")]
    Invalid,
    #[error("signature expired")]
    Expired,
}

/// Derive a per-domain 32-byte HMAC key from the master key via `HKDF-SHA256`.
///
/// * `ikm`   = the 32-byte master key
/// * `salt`  = `None` (master key is already high-entropy uniform random)
/// * `info`  = `b"uploads-v1"` — domain separator for future rotation
///
/// Expanding 32 bytes is well within HKDF's `255 * HashLen` ceiling, so the
/// expansion never fails and `expect()` here is a true invariant, not a
/// runtime error path.
pub fn derive_upload_key(master_key: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, master_key);
    let mut okm = [0u8; 32];
    hk.expand(b"uploads-v1", &mut okm)
        .expect("32-byte okm is always within HKDF output length limit");
    okm
}

/// HMAC payload layout. Kept private so the mint/verify pair are the only
/// code paths that can produce it — prevents accidental drift between the
/// two sides of the signature.
fn payload(filename: &str, exp_unix: u64) -> Vec<u8> {
    format!("{filename}:{exp_unix}").into_bytes()
}

/// Build `"{base}/uploads/{filename}?sig={b64url-nopad}&exp={unix}"`.
///
/// `base` may be absolute (`"http://host"`) or empty (`""`) — the caller
/// decides whether the URL is public-facing or relative. Trailing slashes on
/// `base` are NOT stripped here; the test helper strips them, but production
/// callers pass a pre-normalized base or an empty string.
///
/// # Panics
/// Panics only if the system clock is before the Unix epoch (essentially
/// never) or if `Hmac::new_from_slice` rejects a 32-byte key (impossible —
/// HMAC-SHA256 accepts any key length, and 32 bytes is the canonical size).
pub fn mint_signed_url(base: &str, filename: &str, key: &[u8; 32], ttl_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_secs();
    let exp = now + ttl_secs;

    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .expect("HMAC-SHA256 accepts 32-byte key");
    mac.update(&payload(filename, exp));
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(mac.finalize().into_bytes());

    format!("{base}/uploads/{filename}?sig={sig}&exp={exp}")
}

/// Constant-time HMAC verification.
///
/// Returns `Ok(())` iff:
///   1. Both `sig` and `exp` are present in the query.
///   2. `now_unix <= exp` (not expired).
///   3. `sig` (after base64url-no-pad decode) matches the HMAC of
///      `"{filename}:{exp}"` computed with `key`.
///
/// The final comparison uses `subtle::ConstantTimeEq`, which runs in time
/// independent of how many leading bytes match — the defense required by
/// Phase 64 CONTEXT.md.
///
/// Malformed base64 is collapsed to `Invalid` (not a separate variant) so
/// attackers can't distinguish "your sig isn't even base64" from "your sig
/// decoded but didn't match" via HTTP status.
pub fn verify_signed_url(
    filename: &str,
    query: &SignedUploadQuery,
    key: &[u8; 32],
    now_unix: u64,
) -> Result<(), UploadSignatureError> {
    let (sig_b64, exp) = match (query.sig.as_deref(), query.exp) {
        (Some(s), Some(e)) => (s, e),
        _ => return Err(UploadSignatureError::Missing),
    };

    if now_unix > exp {
        return Err(UploadSignatureError::Expired);
    }

    let submitted = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64) {
        Ok(v) => v,
        Err(_) => return Err(UploadSignatureError::Invalid),
    };

    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .map_err(|_| UploadSignatureError::Invalid)?;
    mac.update(&payload(filename, exp));
    let expected = mac.finalize().into_bytes();

    // Constant-time compare. `ct_eq` also handles length mismatches without
    // branching on the differing index.
    if submitted.ct_eq(&expected).into() {
        Ok(())
    } else {
        Err(UploadSignatureError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn mint_contains_filename_sig_and_exp() {
        let key = [0u8; 32];
        let url = mint_signed_url("http://h", "abc.png", &key, 60);
        assert!(url.starts_with("http://h/uploads/abc.png?"), "{url}");
        assert!(url.contains("sig="));
        assert!(url.contains("&exp="));
    }

    #[test]
    fn roundtrip_ok() {
        let key = [1u8; 32];
        let url = mint_signed_url("", "file.jpg", &key, 3600);
        let q = SignedUploadQuery {
            sig: url
                .split("sig=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .map(|s| s.to_string()),
            exp: url
                .split("exp=")
                .nth(1)
                .and_then(|s| s.parse().ok()),
        };
        assert!(verify_signed_url("file.jpg", &q, &key, now()).is_ok());
    }

    #[test]
    fn hkdf_output_is_not_the_ikm() {
        // Regression guard: HKDF-SHA256 of all-zero ikm produces nonzero okm.
        // If it did equal the ikm, we'd be leaking the master key directly.
        let out = derive_upload_key(&[0u8; 32]);
        assert_ne!(out, [0u8; 32]);
    }
}
