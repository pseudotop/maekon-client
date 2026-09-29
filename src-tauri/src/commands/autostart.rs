//! Tauri IPC commands for autostart management.
//!
//! Source-of-truth: OS state is authoritative for `is_autostart_enabled`.
//! AppConfig.autostart stores ONLY onboarding state (prompt_state, counter).

use tauri::command;

use maekon_core::config::AutostartPromptState;
use maekon_core::error_codes::AutostartCode;

use crate::autostart::{self, AutostartCapabilities};
use crate::ipc_error::IpcError;
use crate::runtime_state::ConfigRuntimeState;

#[command]
pub async fn enable_autostart() -> Result<(), IpcError> {
    tokio::task::spawn_blocking(autostart::enable_autostart)
        .await
        .map_err(|e| {
            IpcError::new(
                AutostartCode::EnableFailed.as_str(),
                format!("spawn_blocking panicked: {e}"),
            )
        })?
        .map_err(|e| {
            IpcError::new(
                AutostartCode::EnableFailed.as_str(),
                format!("autostart enable failed: {e}"),
            )
        })
}

#[command]
pub async fn disable_autostart() -> Result<(), IpcError> {
    tokio::task::spawn_blocking(autostart::disable_autostart)
        .await
        .map_err(|e| {
            IpcError::new(
                AutostartCode::DisableFailed.as_str(),
                format!("spawn_blocking panicked: {e}"),
            )
        })?
        .map_err(|e| {
            IpcError::new(
                AutostartCode::DisableFailed.as_str(),
                format!("autostart disable failed: {e}"),
            )
        })
}

#[command]
pub async fn is_autostart_enabled() -> Result<bool, IpcError> {
    autostart::is_autostart_enabled().map_err(|e| {
        IpcError::new(
            AutostartCode::QueryFailed.as_str(),
            format!("autostart query failed: {e}"),
        )
    })
}

#[command]
pub async fn autostart_capabilities() -> Result<AutostartCapabilities, IpcError> {
    Ok(autostart::detect_capabilities())
}

#[command]
pub async fn mark_autostart_prompt_state(
    new_state: AutostartPromptState,
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<(), IpcError> {
    state
        .config_manager()
        .update_with(|c| {
            c.autostart.prompt_state = new_state;
            Ok(())
        })
        .map(|_| ())
        .map_err(IpcError::from)
}

/// Get autostart-only config (smaller payload than full AppConfig).
#[command]
pub async fn get_autostart_config(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<maekon_core::config::AutostartConfig, IpcError> {
    Ok(state.config_manager().get().autostart)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn is_autostart_enabled_returns_bool() {
        let result = is_autostart_enabled().await;
        // is_autostart_enabled reads OS state (LaunchAgent / Registry / systemd).
        // The bool direction is env-dependent; we can only assert the call
        // succeeds and the returned value is a plain bool (#5594).
        let enabled: bool =
            result.expect("is_autostart_enabled must not error on any supported platform");
        // The signature already guarantees a bool — pin the type, no runtime
        // tautology needed.
        let _typed: bool = enabled;
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[tokio::test]
    async fn autostart_capabilities_returns_supported_on_macos_windows() {
        let result = autostart_capabilities().await.unwrap();
        assert!(
            result.supported,
            "macOS/Windows must return supported=true (Linux is environment-dependent — see autostart::tests::linux_capability_tests)"
        );
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CanaryReport {
        initial: bool,
        round_trip: Result<(), String>,
        restoration: Result<(), String>,
        readback: Result<bool, String>,
    }

    // #12441: collect errors rather than asserting before restoration. The
    // initial query must succeed before any mutation, including cleanup.
    async fn round_trip_with_restore<Q, QF, S, SF>(
        mut query: Q,
        mut set_enabled: S,
    ) -> Result<CanaryReport, String>
    where
        Q: FnMut() -> QF,
        QF: std::future::Future<Output = Result<bool, String>>,
        S: FnMut(bool) -> SF,
        SF: std::future::Future<Output = Result<(), String>>,
    {
        let initial = query().await?;
        let round_trip = async {
            set_enabled(true).await?;
            if !query().await? {
                return Err("autostart remained disabled after enable".into());
            }
            set_enabled(false).await?;
            if query().await? {
                return Err("autostart remained enabled after disable".into());
            }
            Ok(())
        }
        .await;
        // An operation may partially mutate before returning an error. Always
        // attempt restoration, even when the first enable failed.
        let restoration = set_enabled(initial).await;
        let readback = query().await;
        Ok(CanaryReport {
            initial,
            round_trip,
            restoration,
            readback,
        })
    }

    /// Manual native canary; see docs/testing/autostart-native-canary.md.
    /// Logical state restoration does not replace native artifact readback.
    #[tokio::test]
    #[ignore = "modifies OS state — run manually"]
    async fn enable_then_disable_round_trip() {
        assert_eq!(
            std::env::var("MAEKON_AUTOSTART_CANARY").as_deref(),
            Ok("disposable-account-native-snapshot-recorded"),
            "record native state in a disposable account before opting in"
        );
        let report = round_trip_with_restore(
            || async { is_autostart_enabled().await.map_err(|e| e.to_string()) },
            |enabled| async move {
                let result = if enabled {
                    enable_autostart().await
                } else {
                    disable_autostart().await
                };
                result.map_err(|e| e.to_string())
            },
        )
        .await;
        eprintln!("autostart native canary: {report:?}");
        let report = report.expect("initial query failed; no mutation attempted");
        // All assertions occur after restoration and its independent query.
        assert_eq!(report.restoration, Ok(()), "restoration failed: {report:?}");
        assert_eq!(report.readback, Ok(report.initial), "readback: {report:?}");
        assert_eq!(report.round_trip, Ok(()), "roundtrip failed: {report:?}");
    }

    async fn simulate_canary(
        queries: Vec<Result<bool, String>>,
        writes: Vec<(bool, Result<(), String>)>,
    ) -> (Result<CanaryReport, String>, Vec<String>) {
        let calls = std::cell::RefCell::new(Vec::new());
        let mut queries = queries.into_iter();
        let mut writes = writes.into_iter();
        let result = round_trip_with_restore(
            || {
                calls.borrow_mut().push("query".into());
                std::future::ready(queries.next().expect("unexpected query"))
            },
            |enabled| {
                calls.borrow_mut().push(format!("set:{enabled}"));
                let (expected, result) = writes.next().expect("unexpected mutation");
                assert_eq!(enabled, expected);
                std::future::ready(result)
            },
        )
        .await;
        assert_eq!(queries.next(), None, "missing readback");
        assert_eq!(writes.next(), None, "missing restoration");
        (result, calls.into_inner())
    }

    #[tokio::test]
    async fn canary_initial_query_failure_never_mutates() {
        let (result, calls) = simulate_canary(vec![Err("initial".into())], vec![]).await;
        assert_eq!(result, Err("initial".into()));
        assert_eq!(calls, ["query"]);
    }

    #[tokio::test]
    async fn canary_restores_both_initial_states_after_success() {
        for initial in [false, true] {
            let (result, calls) = simulate_canary(
                vec![Ok(initial), Ok(true), Ok(false), Ok(initial)],
                vec![(true, Ok(())), (false, Ok(())), (initial, Ok(()))],
            )
            .await;
            assert_eq!(
                result,
                Ok(CanaryReport {
                    initial,
                    round_trip: Ok(()),
                    restoration: Ok(()),
                    readback: Ok(initial),
                })
            );
            assert_eq!(
                calls,
                ["query", "set:true", "query", "set:false", "query"]
                    .map(str::to_owned)
                    .into_iter()
                    .chain([format!("set:{initial}"), "query".into()])
                    .collect::<Vec<_>>()
            );
        }
    }

    #[tokio::test]
    async fn canary_restores_after_every_round_trip_failure() {
        for initial in [false, true] {
            let cases = [
                (vec![], vec![(true, Err("enable".into()))], "enable"),
                (
                    vec![Err("query-enabled".into())],
                    vec![(true, Ok(()))],
                    "query-enabled",
                ),
                (
                    vec![Ok(false)],
                    vec![(true, Ok(()))],
                    "autostart remained disabled after enable",
                ),
                (
                    vec![Ok(true)],
                    vec![(true, Ok(())), (false, Err("disable".into()))],
                    "disable",
                ),
                (
                    vec![Ok(true), Err("query-disabled".into())],
                    vec![(true, Ok(())), (false, Ok(()))],
                    "query-disabled",
                ),
                (
                    vec![Ok(true), Ok(true)],
                    vec![(true, Ok(())), (false, Ok(()))],
                    "autostart remained enabled after disable",
                ),
            ];
            for (queries, mut writes, error) in cases {
                let queries = [Ok(initial)]
                    .into_iter()
                    .chain(queries)
                    .chain([Ok(initial)])
                    .collect();
                writes.push((initial, Ok(())));
                let (result, calls) = simulate_canary(queries, writes).await;
                let report = result.unwrap();
                assert_eq!(report.round_trip, Err(error.into()));
                assert_eq!(report.restoration, Ok(()));
                assert_eq!(report.readback, Ok(initial));
                assert_eq!(
                    &calls[calls.len() - 2..],
                    [format!("set:{initial}"), "query".into()]
                );
            }
        }
    }

    #[tokio::test]
    async fn canary_preserves_primary_restore_and_readback_errors() {
        for initial in [false, true] {
            for readback in [Err("readback".into()), Ok(!initial)] {
                let (result, _) = simulate_canary(
                    vec![Ok(initial), readback.clone()],
                    vec![
                        (true, Err("enable".into())),
                        (initial, Err("restore".into())),
                    ],
                )
                .await;
                assert_eq!(
                    result,
                    Ok(CanaryReport {
                        initial,
                        round_trip: Err("enable".into()),
                        restoration: Err("restore".into()),
                        readback,
                    })
                );
            }
        }
    }
}
