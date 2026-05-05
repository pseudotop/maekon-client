use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use rand::Rng;

use super::{MAX_PENDING_NONCES, MAX_SESSIONS, NONCE_TTL, SESSION_TTL};

struct PendingNonce {
    nonce_bytes: Vec<u8>,
    peer_device_id: String,
    created_at: Instant,
}

/// An authenticated session.
struct Session {
    #[allow(dead_code)]
    peer_device_id: String,
    created_at: Instant,
}

/// Thread-safe session store for HMAC challenge-response authentication.
#[derive(Clone)]
pub(super) struct SessionStore {
    /// Pending nonces: nonce_hex -> PendingNonce
    pending: Arc<RwLock<HashMap<String, PendingNonce>>>,
    /// Active sessions: token_hex -> Session
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    pub(super) fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Generate a random nonce and store it as pending.
    pub(super) fn create_nonce(&self, peer_device_id: &str) -> Vec<u8> {
        let mut nonce = vec![0u8; 32];
        rand::rng().fill_bytes(&mut nonce);
        let hex_key = hex::encode(&nonce);

        let mut pending = self.pending.write();
        // Evict expired nonces
        let now = Instant::now();
        pending.retain(|_, v| now.duration_since(v.created_at) < NONCE_TTL);
        // Evict oldest if at capacity
        if pending.len() >= MAX_PENDING_NONCES {
            if let Some(oldest_key) = pending
                .iter()
                .min_by_key(|(_, v)| v.created_at)
                .map(|(k, _)| k.clone())
            {
                pending.remove(&oldest_key);
            }
        }
        pending.insert(
            hex_key,
            PendingNonce {
                nonce_bytes: nonce.clone(),
                peer_device_id: peer_device_id.to_string(),
                created_at: Instant::now(),
            },
        );
        nonce
    }

    /// Consume a pending nonce (one-time use). Returns (nonce_bytes, peer_device_id).
    pub(super) fn take_nonce(&self, nonce_hex: &str) -> Option<(Vec<u8>, String)> {
        let mut pending = self.pending.write();
        let entry = pending.remove(nonce_hex)?;
        // Check expiry
        if Instant::now().duration_since(entry.created_at) >= NONCE_TTL {
            return None;
        }
        Some((entry.nonce_bytes, entry.peer_device_id))
    }

    /// Create a session token for an authenticated peer.
    pub(super) fn create_session(&self, peer_device_id: &str) -> String {
        let mut token = vec![0u8; 32];
        rand::rng().fill_bytes(&mut token);
        let token_hex = hex::encode(&token);

        let mut sessions = self.sessions.write();
        // Evict expired sessions
        let now = Instant::now();
        sessions.retain(|_, v| now.duration_since(v.created_at) < SESSION_TTL);
        // Evict oldest if at capacity
        if sessions.len() >= MAX_SESSIONS {
            if let Some(oldest_key) = sessions
                .iter()
                .min_by_key(|(_, v)| v.created_at)
                .map(|(k, _)| k.clone())
            {
                sessions.remove(&oldest_key);
            }
        }
        sessions.insert(
            token_hex.clone(),
            Session {
                peer_device_id: peer_device_id.to_string(),
                created_at: Instant::now(),
            },
        );
        token_hex
    }

    /// Validate a session token. Returns true if valid and not expired.
    pub(super) fn validate_token(&self, token: &str) -> bool {
        let sessions = self.sessions.read();
        match sessions.get(token) {
            Some(session) => Instant::now().duration_since(session.created_at) < SESSION_TTL,
            None => false,
        }
    }
}
