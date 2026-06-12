const CUA_SAFE_MODE_ENV: &str = "MAEKON_CUA_SAFE_MODE";

pub(crate) fn cua_safe_mode_enabled() -> bool {
    cua_safe_mode_enabled_from(std::env::var(CUA_SAFE_MODE_ENV).ok().as_deref())
}

fn cua_safe_mode_enabled_from(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| {
        matches!(value, "1")
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    })
}

pub(super) fn initial_capture_flags(show_indicator: bool, cua_safe_mode: bool) -> (bool, bool) {
    let capture_paused = cua_safe_mode;
    let indicator_visible = show_indicator;
    (capture_paused, indicator_visible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cua_safe_mode_parser_accepts_explicit_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on", " On "] {
            assert!(
                cua_safe_mode_enabled_from(Some(value)),
                "{value:?} should enable CUA safe mode"
            );
        }
    }

    #[test]
    fn cua_safe_mode_parser_rejects_absent_or_falsey_values() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("off"),
            Some("no"),
        ] {
            assert!(
                !cua_safe_mode_enabled_from(value),
                "{value:?} should not enable CUA safe mode"
            );
        }
    }

    #[test]
    fn cua_safe_mode_starts_paused_but_keeps_indicator_available() {
        assert_eq!(initial_capture_flags(true, true), (true, true));
        assert_eq!(initial_capture_flags(false, true), (true, false));
        assert_eq!(initial_capture_flags(true, false), (false, true));
    }
}
