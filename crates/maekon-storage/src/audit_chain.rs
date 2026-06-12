//! audit_log SHA-256 해시 체인 헬퍼 (#4834, ADR-072 client mirror).
//!
//! `audit_log` 행을 tamper-**evident** 하게 만드는 해시 계산 로직을 모은다.
//! 체인 규칙: `entry_hash = SHA256(prev_hash_bytes || canonical(record))`.
//! - `prev_hash` 는 직전 체인 행의 `entry_hash` (genesis 는 [`GENESIS_PREV_HASH`]).
//! - `canonical(record)` 는 불변 필드의 **길이-접두(length-prefixed) + 필드 순서
//!   고정** 직렬화이다. serde_json 은 경계 모호성(키 순서/escape) 때문에 쓰지
//!   않는다.
//!
//! ## canonical 직렬화 형식
//! 각 필드는 `field_index(1바이트) || u32 len(big-endian, UTF-8 바이트 길이) ||
//! bytes` 로 인코딩한다. `Option` 의 None 은 길이 0 의 sentinel 과 구분하기 위해
//! 별도의 presence 바이트(0/1)를 길이 앞에 둔다. 필드 순서는 AuditEntry 의 불변
//! 필드 순서를 고정한다:
//!   0 entry_id, 1 timestamp(RFC3339), 2 session_id, 3 command_id,
//!   4 action_type, 5 status(stable string), 6 details(Option), 7 execution_time_ms(Option).
//!
//! ## hash_version seam (out-of-scope)
//! 현재 알고리즘은 plain SHA-256(서명 없음)이다. 전면 재기록 내부자 위협 방어를
//! 위한 HMAC/Ed25519 도입 시 알고리즘을 분기할 수 있도록 [`HASH_VERSION`] 상수를
//! 남겨둔다. 본 이슈(#4834)에서는 v1(SHA-256-only)만 구현한다.

use sha2::{Digest, Sha256};

/// genesis 행의 `prev_hash` — 64-char zero hex(32바이트 0).
pub const GENESIS_PREV_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// 현재 해시 알고리즘 버전. v1 = plain SHA-256 (서명 없음).
/// 향후 HMAC/Ed25519 도입 시 분기점(seam)으로 사용한다(#4834 out-of-scope).
pub const HASH_VERSION: u8 = 1;

/// 해시 체인 계산에 쓰이는 불변 감사 레코드 뷰.
///
/// [`AuditEntry`](maekon_core::models::audit::AuditEntry) 의 불변 필드만 빌려와
/// canonical 직렬화한다. 소유권 없이 참조만 받으므로 write/verify 양쪽에서 동일
/// 입력을 보장한다.
pub struct CanonicalRecord<'a> {
    pub entry_id: &'a str,
    pub timestamp_rfc3339: &'a str,
    pub session_id: &'a str,
    pub command_id: &'a str,
    pub action_type: &'a str,
    /// status 의 stable string(예: "Completed") — DB 저장 문자열과 동일.
    pub status: &'a str,
    pub details: Option<&'a str>,
    pub execution_time_ms: Option<u64>,
}

/// 문자열 필드를 `index || u32 len(BE) || bytes` 로 buffer 에 push 한다.
fn push_str_field(buf: &mut Vec<u8>, index: u8, value: &str) {
    buf.push(index);
    let bytes = value.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// Option<str> 필드를 `index || presence(0/1) || (있으면) u32 len(BE) || bytes` 로 push.
fn push_opt_str_field(buf: &mut Vec<u8>, index: u8, value: Option<&str>) {
    buf.push(index);
    match value {
        Some(v) => {
            buf.push(1);
            let bytes = v.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            buf.extend_from_slice(bytes);
        }
        None => buf.push(0),
    }
}

/// Option<u64> 필드를 `index || presence(0/1) || (있으면) u64 value(BE)` 로 push.
fn push_opt_u64_field(buf: &mut Vec<u8>, index: u8, value: Option<u64>) {
    buf.push(index);
    match value {
        Some(v) => {
            buf.push(1);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        None => buf.push(0),
    }
}

/// 레코드의 결정적 canonical 바이트열을 만든다.
///
/// 동일 입력은 항상 동일 출력을 낸다(키 순서/escape 모호성 없음). 필드 순서는
/// 모듈 doc 의 0..=7 인덱스를 따른다.
pub fn canonical_bytes(record: &CanonicalRecord<'_>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    // 알고리즘 버전을 prefix 해 향후 형식 분기를 무결성에 결속한다.
    buf.push(HASH_VERSION);
    push_str_field(&mut buf, 0, record.entry_id);
    push_str_field(&mut buf, 1, record.timestamp_rfc3339);
    push_str_field(&mut buf, 2, record.session_id);
    push_str_field(&mut buf, 3, record.command_id);
    push_str_field(&mut buf, 4, record.action_type);
    push_str_field(&mut buf, 5, record.status);
    push_opt_str_field(&mut buf, 6, record.details);
    push_opt_u64_field(&mut buf, 7, record.execution_time_ms);
    buf
}

/// `entry_hash = hex(SHA256(prev_hash_bytes || canonical(record)))` 를 계산한다.
///
/// `prev_hash_hex` 는 직전 체인 행의 `entry_hash`(genesis 는 [`GENESIS_PREV_HASH`]).
/// hex 문자열을 그대로 바이트열로 흡수하여 해시에 섞는다(hex→raw 디코드가 아니라
/// 문자열 바이트를 쓰므로 prev_hash 형식이 잘못돼도 결정적이다).
pub fn compute_entry_hash(prev_hash_hex: &str, record: &CanonicalRecord<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash_hex.as_bytes());
    hasher.update(canonical_bytes(record));
    let digest = hasher.finalize();
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CanonicalRecord<'static> {
        CanonicalRecord {
            entry_id: "e-1",
            timestamp_rfc3339: "2026-06-03T00:00:00+00:00",
            session_id: "sess-1",
            command_id: "cmd-1",
            action_type: "MouseClick",
            status: "Completed",
            details: Some("ok"),
            execution_time_ms: Some(42),
        }
    }

    #[test]
    fn genesis_prev_hash_is_64_zero_hex() {
        assert_eq!(GENESIS_PREV_HASH.len(), 64);
        assert!(GENESIS_PREV_HASH.chars().all(|c| c == '0'));
    }

    #[test]
    fn canonical_is_deterministic() {
        let r = sample();
        assert_eq!(canonical_bytes(&r), canonical_bytes(&sample()));
    }

    #[test]
    fn entry_hash_is_deterministic_and_hex64() {
        let r = sample();
        let h1 = compute_entry_hash(GENESIS_PREV_HASH, &r);
        let h2 = compute_entry_hash(GENESIS_PREV_HASH, &sample());
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn changing_any_field_changes_hash() {
        let base = compute_entry_hash(GENESIS_PREV_HASH, &sample());

        let mut r = sample();
        r.details = Some("tampered");
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r), base);

        let mut r = sample();
        r.timestamp_rfc3339 = "2026-06-03T00:00:01+00:00";
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r), base);

        let mut r = sample();
        r.status = "Failed";
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r), base);

        let mut r = sample();
        r.execution_time_ms = None;
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r), base);
    }

    #[test]
    fn changing_prev_hash_changes_hash() {
        let r = sample();
        let g = compute_entry_hash(GENESIS_PREV_HASH, &r);
        let other = compute_entry_hash(&"a".repeat(64), &r);
        assert_ne!(g, other);
    }

    #[test]
    fn none_details_differs_from_empty_details() {
        // None 과 Some("") 는 presence 바이트로 구분되어야 한다.
        let mut none = sample();
        none.details = None;
        let mut empty = sample();
        empty.details = Some("");
        assert_ne!(
            compute_entry_hash(GENESIS_PREV_HASH, &none),
            compute_entry_hash(GENESIS_PREV_HASH, &empty)
        );
    }

    #[test]
    fn length_prefix_prevents_field_boundary_ambiguity() {
        // ("ab","c") 와 ("a","bc") 가 같은 해시를 내면 안 된다.
        let mut r1 = sample();
        r1.entry_id = "ab";
        r1.session_id = "c";
        let mut r2 = sample();
        r2.entry_id = "a";
        r2.session_id = "bc";
        assert_ne!(
            compute_entry_hash(GENESIS_PREV_HASH, &r1),
            compute_entry_hash(GENESIS_PREV_HASH, &r2)
        );
    }
}
