//! PKCE (Proof Key for Code Exchange) S256 implementation.
//!
//! Generates a cryptographically random code verifier and its SHA-256
//! derived challenge per RFC 7636.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};

/// PKCE challenge pair.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    /// The code verifier (sent during token exchange).
    pub verifier: String,
    /// The S256 challenge (sent during authorization request).
    pub challenge: String,
}

/// Generate a PKCE S256 code verifier and challenge.
///
/// The verifier is a 32-byte random value, base64url-encoded (43 chars).
/// The challenge is the SHA-256 hash of the verifier, base64url-encoded.
pub fn generate_pkce() -> PkceChallenge {
    let mut verifier_bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    PkceChallenge {
        verifier,
        challenge,
    }
}

/// Generate a random state parameter for CSRF protection.
pub fn generate_state() -> String {
    let mut state_bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut state_bytes);
    URL_SAFE_NO_PAD.encode(state_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_is_43_chars() {
        let pkce = generate_pkce();
        assert_eq!(pkce.verifier.len(), 43);
    }

    #[test]
    fn pkce_challenge_is_valid_s256() {
        let pkce = generate_pkce();

        // Manually derive challenge from verifier
        let mut hasher = Sha256::new();
        hasher.update(pkce.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());

        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn pkce_challenge_is_43_chars() {
        let pkce = generate_pkce();
        assert_eq!(pkce.challenge.len(), 43);
    }

    #[test]
    fn each_generation_is_unique() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
    }

    #[test]
    fn state_is_22_chars() {
        let state = generate_state();
        assert_eq!(state.len(), 22);
    }

    #[test]
    fn each_state_is_unique() {
        let a = generate_state();
        let b = generate_state();
        assert_ne!(a, b);
    }
}
