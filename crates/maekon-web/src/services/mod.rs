pub(crate) mod ai_model_catalog_assembler;
pub(crate) mod ai_model_catalog_auth;
pub(crate) mod ai_model_catalog_endpoint;
pub mod ai_model_catalog_service;
pub mod ai_model_catalog_web_service;
pub mod ai_provider_spec_service;
pub mod ai_provider_spec_web_service;
pub(crate) mod automation_assembler;
pub(crate) mod automation_gui_assembler;
pub mod automation_gui_service;
pub mod automation_service;
pub(crate) mod backup_assembler;
pub mod backup_service;
pub mod bug_report_service;
pub mod dashboard_service;
pub mod data_web_service;
pub mod egress_ledger_service;
pub(crate) mod events_assembler;
pub mod events_service;
pub(crate) mod export_assembler;
pub mod export_service;
pub(crate) mod focus_assembler;
pub mod focus_service;
pub(crate) mod frames_assembler;
pub mod frames_service;
pub(crate) mod idle_assembler;
pub mod idle_service;
pub(crate) mod integration_assembler;
pub mod integration_service;
pub mod memory_claims_service;
pub(crate) mod metrics_assembler;
pub mod metrics_service;
pub(crate) mod onboarding_assembler;
pub mod onboarding_service;
pub(crate) mod processes_assembler;
pub mod processes_service;
pub mod provider_cli_diagnostics;
pub(crate) mod reports_assembler;
pub(crate) mod reports_query_support;
pub mod reports_service;
pub(crate) mod search_assembler;
pub mod search_service;
pub mod semantic_search_service;
pub(crate) mod sessions_assembler;
pub mod sessions_service;
pub(crate) mod settings_assembler;
pub(crate) mod settings_config_mutation;
pub(crate) mod settings_endpoint;
pub(crate) mod settings_policy_service;
pub mod settings_query_service;
pub(crate) mod settings_secret_persistence;
pub mod settings_service;
pub(crate) mod settings_update_flow;
pub(crate) mod settings_validation;
pub mod settings_web_service;
pub(crate) mod stats_assembler;
pub(crate) mod stats_query_support;
pub mod stats_service;
pub(crate) mod stream_assembler;
pub mod stream_service;
pub(crate) mod suggestions_assembler;
pub mod suggestions_service;
pub(crate) mod support_assembler;
pub mod support_service;
pub(crate) mod tags_assembler;
pub mod tags_service;
pub(crate) mod timeline_assembler;
pub mod timeline_service;
pub mod update_service;
pub mod web_contexts;

fn current_metadata_timestamp() -> String {
    format_metadata_timestamp(time::UtcDateTime::now())
}

// rc.11 wrote a numeric UTC offset and zero, three, six, or nine fractional digits.
fn format_metadata_timestamp(utc: time::UtcDateTime) -> String {
    use std::fmt::Write;

    let (hour, minute, second, nanos) = utc.as_hms_nano();
    let mut text = format!("{}T{hour:02}:{minute:02}:{second:02}", utc.date());
    if nanos != 0 {
        let (fraction, width) = if nanos.is_multiple_of(1_000_000) {
            (nanos / 1_000_000, 3)
        } else if nanos.is_multiple_of(1_000) {
            (nanos / 1_000, 6)
        } else {
            (nanos, 9)
        };
        let _ = write!(text, ".{fraction:0width$}");
    }
    text.push_str("+00:00");
    text
}

#[cfg(test)]
mod tests {
    use super::format_metadata_timestamp;
    use time::{Date, Month, Time, UtcDateTime};

    #[test]
    fn metadata_preserves_rc11_utc_offset_and_fraction_groups() {
        for (unix_nanos, expected) in [
            (-1, "1969-12-31T23:59:59.999999999+00:00"),
            (0, "1970-01-01T00:00:00+00:00"),
            (1, "1970-01-01T00:00:00.000000001+00:00"),
            (10_000, "1970-01-01T00:00:00.000010+00:00"),
            (100_000_000, "1970-01-01T00:00:00.100+00:00"),
            (1_709_164_800_000_000_000, "2024-02-29T00:00:00+00:00"),
        ] {
            let utc = UtcDateTime::from_unix_timestamp_nanos(unix_nanos).unwrap();
            assert_eq!(format_metadata_timestamp(utc), expected);
        }
    }

    #[test]
    fn metadata_preserves_rc11_signed_years() {
        for (year, expected) in [
            (-1, "-0001-01-01T00:00:00+00:00"),
            (0, "0000-01-01T00:00:00+00:00"),
            (10_000, "+10000-01-01T00:00:00+00:00"),
        ] {
            let date = Date::from_calendar_date(year, Month::January, 1).unwrap();
            assert_eq!(
                format_metadata_timestamp(UtcDateTime::new(date, Time::MIDNIGHT)),
                expected
            );
        }
    }
}
