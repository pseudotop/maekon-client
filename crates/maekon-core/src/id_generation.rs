//! ADR-022 (client) prefix+ULID entity ID generation.
//!
//! 클라이언트 ID 생성 규약은 maekon-client 레지스트리의 ADR-022 가 정본이며,
//! prefix+ULID 형태(shape)는 server ADR-055 와 동일하다.

use thiserror::Error;

/// `generate_id_checked` 의 prefix 검증 실패 사유.
///
/// 동적(런타임) prefix 를 다루는 호출자가 panic 없이 오류를 처리할 수 있도록
/// 제공된다. 정적 리터럴 prefix 만 쓰는 기존 호출자들은 [`generate_id`] 를
/// 그대로 사용한다 (#4344).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdError {
    /// ADR-022 id prefix 가 규칙을 위반함 (비어 있음 / 63바이트 초과 /
    /// 소문자 ASCII 로 시작하지 않음 / `a-z`,`0-9`,`_` 외 문자 포함).
    #[error("invalid ADR-022 id prefix `{prefix}`")]
    InvalidPrefix { prefix: String },
}

/// Generates a `{prefix}_{ULID}` identifier.
///
/// The Rust client keeps the ADR-022 prefix+ULID shape (identical to the
/// server-side ADR-055 shape) while allowing client-local prefixes that are
/// not part of the server domain registry.
///
/// # 정적 prefix 전용 (#4344)
///
/// 이 함수는 **컴파일 타임에 알려진 정적 리터럴 prefix** 전용이다. 모든
/// 호출자는 `"sug"`, `"req"` 등 검증된 정적 리터럴을 넘기므로 실제 panic
/// 가능성이 없다. 런타임에 결정되는 동적 prefix 를 다뤄야 한다면 panic 하지
/// 않는 [`generate_id_checked`] 를 사용하라.
///
/// # Panics
///
/// Panics when `prefix` is empty, longer than 63 bytes, does not start with a
/// lowercase ASCII letter, or contains characters outside `a-z`, `0-9`, and
/// `_`.
#[must_use]
pub fn generate_id(prefix: &str) -> String {
    assert!(
        is_valid_prefix(prefix),
        "invalid ADR-022 id prefix `{prefix}`"
    );
    format!("{prefix}_{}", ulid::Ulid::new())
}

/// [`generate_id`] 의 panic 하지 않는 변형 (#4344).
///
/// 런타임에 결정되는 동적 prefix 를 다루는 미래 호출자를 위해 제공된다.
/// prefix 가 ADR-022 규칙을 위반하면 panic 대신 [`IdError::InvalidPrefix`] 를
/// 반환한다.
///
/// # Errors
///
/// `prefix` 가 비어 있거나, 63바이트를 초과하거나, 소문자 ASCII 로 시작하지
/// 않거나, `a-z`/`0-9`/`_` 외 문자를 포함하면 [`IdError::InvalidPrefix`] 를
/// 반환한다.
pub fn generate_id_checked(prefix: &str) -> Result<String, IdError> {
    if is_valid_prefix(prefix) {
        Ok(format!("{prefix}_{}", ulid::Ulid::new()))
    } else {
        Err(IdError::InvalidPrefix {
            prefix: prefix.to_owned(),
        })
    }
}

/// ADR-022 prefix 규칙 검증 (panic/Result 양쪽에서 공유하는 순수 함수).
fn is_valid_prefix(prefix: &str) -> bool {
    let starts_with_lowercase = prefix
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase);
    let chars_are_supported = prefix
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');

    starts_with_lowercase && prefix.len() <= 63 && chars_are_supported
}

#[cfg(test)]
mod tests {
    use super::{generate_id, generate_id_checked, IdError};

    /// 워크스페이스 전역에서 `generate_id(prefix)` 에 실제로 전달되는 정적
    /// 리터럴 prefix 전체 목록 (#4344).
    ///
    /// `grep -rhoE 'generate_id\("[^"]+"\)'` 로 추출한 등록/사용 prefix 집합.
    /// 신규 prefix 도입 시 이 목록도 갱신해 registry 가 panic 으로 drift 하지
    /// 않도록 보장한다.
    const USED_PREFIXES: &[&str] = &[
        "ann", "aud", "cch", "clip", "clm", "consent", "ctx", "edg", "env", "evt", "fa", "flow",
        "hl", "input", "ovr", "pomo", "proc", "ptr", "q", "rcpt", "rect", "req", "scene", "ses",
        "sug", "tkt", "win",
    ];

    #[test]
    fn generate_id_uses_prefix_and_ulid_shape() {
        let id = generate_id("sug");

        assert!(id.starts_with("sug_"));
        assert_eq!(id.len(), "sug_".len() + 26);
        assert!(id["sug_".len()..]
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
    }

    #[test]
    fn generate_id_uniqueness() {
        let a = generate_id("req");
        let b = generate_id("req");
        assert_ne!(
            a, b,
            "consecutive generate_id calls must produce different IDs"
        );
    }

    #[test]
    #[should_panic(expected = "invalid ADR-022 id prefix")]
    fn generate_id_rejects_uppercase_prefixes() {
        let _ = generate_id("Sug");
    }

    /// #4344 회귀 방지: 실제 사용 중인 모든 정적 prefix 는 `generate_id` 에서
    /// panic 하지 않아야 한다 (registry 가 panic 으로 drift 하는 것을 차단).
    #[test]
    fn generate_id_does_not_panic_on_any_used_prefix() {
        for &prefix in USED_PREFIXES {
            let id = generate_id(prefix);
            assert!(
                id.starts_with(&format!("{prefix}_")),
                "generate_id({prefix:?}) must yield a `{prefix}_`-prefixed id"
            );
            assert_eq!(
                id.len(),
                prefix.len() + 1 + 26,
                "generate_id({prefix:?}) must be `{{prefix}}_{{26-char ULID}}`"
            );
        }
    }

    /// #4344: `generate_id_checked` 는 모든 사용 중 prefix 에 대해 Ok 를
    /// 반환하고, `generate_id` 와 동일한 형태의 id 를 만들어야 한다.
    #[test]
    fn generate_id_checked_accepts_all_used_prefixes() {
        for &prefix in USED_PREFIXES {
            let id = generate_id_checked(prefix)
                .unwrap_or_else(|e| panic!("checked variant rejected used prefix {prefix:?}: {e}"));
            assert!(id.starts_with(&format!("{prefix}_")));
            assert_eq!(id.len(), prefix.len() + 1 + 26);
        }
    }

    /// #4344: `generate_id_checked` 는 잘못된 prefix 에 panic 하지 않고
    /// [`IdError::InvalidPrefix`] 를 반환한다.
    #[test]
    fn generate_id_checked_rejects_invalid_prefixes_without_panic() {
        for bad in [
            "Sug",
            "",
            "1abc",
            "has space",
            "tooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooo_long",
        ] {
            let err = generate_id_checked(bad).expect_err("invalid prefix must error");
            assert_eq!(
                err,
                IdError::InvalidPrefix {
                    prefix: bad.to_owned()
                }
            );
        }
    }

    /// #4344: 검증 통과 prefix 에 대해 panic 변형과 checked 변형이 동일하게
    /// 동작함을 명시 (양쪽이 동일한 `is_valid_prefix` 를 공유).
    #[test]
    fn generate_id_checked_uniqueness() {
        let a = generate_id_checked("req").expect("valid prefix");
        let b = generate_id_checked("req").expect("valid prefix");
        assert_ne!(a, b, "consecutive generate_id_checked calls must differ");
    }
}
