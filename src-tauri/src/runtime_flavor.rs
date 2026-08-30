use std::ffi::{OsStr, OsString};

const BUILD_APP_FLAVOR: Option<&str> = option_env!("MAEKON_BUILD_APP_FLAVOR");

fn resolve(build_flavor: Option<&str>, runtime_flavor: Option<&OsStr>) -> OsString {
    build_flavor
        .map(str::trim)
        .filter(|flavor| !flavor.is_empty())
        .map(OsString::from)
        .or_else(|| runtime_flavor.map(OsString::from))
        .unwrap_or_else(|| OsString::from("dev"))
}

pub(crate) fn configure() {
    // Keep local debug clients from opening the release install's data directory.
    // A bundle-specific QC flavor wins over the launcher environment so a
    // LaunchServices start cannot silently fall back to the shared `dev`
    // profile and its older Keychain ACLs (#11618).
    let flavor = resolve(
        BUILD_APP_FLAVOR,
        std::env::var_os("MAEKON_APP_FLAVOR").as_deref(),
    );
    std::env::set_var("MAEKON_APP_FLAVOR", flavor);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_runtime_flavor_defaults_to_dev() {
        assert_eq!(resolve(None, None), OsStr::new("dev"));
    }

    #[test]
    fn debug_runtime_flavor_preserves_runtime_override_without_build_flavor() {
        assert_eq!(
            resolve(None, Some(OsStr::new("tc-runtime"))),
            OsStr::new("tc-runtime")
        );
    }

    #[test]
    fn debug_runtime_flavor_build_identity_wins_for_launchservices() {
        assert_eq!(
            resolve(Some("qc-demo-20260827"), Some(OsStr::new("dev"))),
            OsStr::new("qc-demo-20260827")
        );
    }

    #[test]
    fn compiled_build_app_flavor_matches_the_cargo_build_environment() {
        if let Ok(expected) = std::env::var("MAEKON_BUILD_APP_FLAVOR") {
            assert_eq!(BUILD_APP_FLAVOR, Some(expected.as_str()));
            assert_eq!(
                resolve(BUILD_APP_FLAVOR, Some(OsStr::new("dev"))),
                OsStr::new(&expected)
            );
        }
    }
}
