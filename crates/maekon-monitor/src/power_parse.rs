//! Cfg-free parsing of macOS `pmset -g batt` output into a `PowerStatus`.
//!
//! `/usr/bin/pmset` only exists on macOS, but turning its text report into the
//! cross-platform `PowerStatus` model — detecting the power source, the battery
//! percentage, and the low-battery / battery-saver derivation — is pure string
//! logic. Extracting it here (no `#[cfg]`, no subprocess) makes it unit-testable
//! on any OS (#5138), so a regression in the AC/battery detection or the 20%
//! low-battery threshold is caught by the ordinary test run, not only on a macOS
//! runner. Sibling of `active_window_parse` (the osascript parser, #5126).

// Called by the macOS power adapter (`super::macos`) plus the tests below; it has
// no non-test caller on other targets.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use maekon_core::models::system::PowerStatus;

/// Low-battery threshold (percent, inclusive) used to derive
/// `low_battery` / `battery_saver_active`.
const LOW_BATTERY_PERCENT: f32 = 20.0;

/// Parse `pmset -g batt` stdout into a `PowerStatus`.
///
/// Power source comes from the `'AC Power'` / `'Battery Power'` marker (case
/// insensitive); the battery percentage is the first whitespace/`;`/`\t`-
/// delimited token ending in `%`. `battery_saver_active` is true only when the
/// machine is on battery AND at/below [`LOW_BATTERY_PERCENT`] — a desktop (no
/// battery line, on AC) is therefore reported as safe.
pub(crate) fn parse_pmset_batt_output(output: &str) -> PowerStatus {
    let lower = output.to_ascii_lowercase();
    let external_power_connected = if lower.contains("'ac power'") {
        Some(true)
    } else if lower.contains("'battery power'") {
        Some(false)
    } else {
        None
    };
    let battery_percent = output
        .split(|c: char| c.is_whitespace() || c == ';' || c == '\t')
        .find_map(|token| {
            token
                .trim()
                .strip_suffix('%')
                .and_then(|value| value.parse::<f32>().ok())
        });
    let low_battery = battery_percent.is_some_and(|percent| percent <= LOW_BATTERY_PERCENT);

    PowerStatus {
        external_power_connected,
        battery_percent,
        low_battery,
        battery_saver_active: external_power_connected == Some(false) && low_battery,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_low_battery_pmset_output_as_battery_saver_active() {
        let status = parse_pmset_batt_output(
            "Now drawing from 'Battery Power'\n\
             -InternalBattery-0 (id=1234567)\t15%; discharging; 1:02 remaining present: true\n",
        );
        assert_eq!(status.external_power_connected, Some(false));
        assert_eq!(status.battery_percent, Some(15.0));
        assert!(status.low_battery);
        assert!(status.battery_saver_active);
    }

    #[test]
    fn parses_ac_pmset_output_as_not_battery_saver_active() {
        let status = parse_pmset_batt_output(
            "Now drawing from 'AC Power'\n\
             -InternalBattery-0 (id=1234567)\t100%; charged; 0:00 remaining present: true\n",
        );
        assert_eq!(status.external_power_connected, Some(true));
        assert_eq!(status.battery_percent, Some(100.0));
        assert!(!status.low_battery);
        assert!(!status.battery_saver_active);
    }

    #[test]
    fn parses_desktop_pmset_output_as_unknown_but_safe() {
        let status = parse_pmset_batt_output("Now drawing from 'AC Power'\n");
        assert_eq!(status.external_power_connected, Some(true));
        assert_eq!(status.battery_percent, None);
        assert!(!status.low_battery);
        assert!(!status.battery_saver_active);
    }

    #[test]
    fn empty_output_is_all_unknown_and_safe() {
        let status = parse_pmset_batt_output("");
        assert_eq!(status.external_power_connected, None);
        assert_eq!(status.battery_percent, None);
        assert!(!status.low_battery);
        assert!(!status.battery_saver_active);
    }

    #[test]
    fn low_battery_threshold_is_inclusive_at_20() {
        // 20% is low (<=), 21% is not. Boundary guards the derivation.
        let at = parse_pmset_batt_output("Now drawing from 'Battery Power'\n 20%; discharging\n");
        assert!(
            at.low_battery,
            "20% must count as low (threshold inclusive)"
        );
        assert!(at.battery_saver_active);

        let above =
            parse_pmset_batt_output("Now drawing from 'Battery Power'\n 21%; discharging\n");
        assert!(!above.low_battery, "21% must NOT be low");
        assert!(!above.battery_saver_active);
    }

    #[test]
    fn low_battery_on_ac_is_not_battery_saver() {
        // On AC at a low percentage, the saver is NOT engaged (it keys on being
        // on battery), but low_battery still reflects the charge level.
        let status = parse_pmset_batt_output("Now drawing from 'AC Power'\n 10%; charging\n");
        assert_eq!(status.external_power_connected, Some(true));
        assert!(status.low_battery);
        assert!(
            !status.battery_saver_active,
            "battery saver requires being on battery, not just low"
        );
    }

    #[test]
    fn takes_the_first_percentage_token() {
        // The first %-suffixed token wins (the battery charge precedes any other).
        let status = parse_pmset_batt_output(
            "Now drawing from 'Battery Power'\n 42%; discharging; health 87%\n",
        );
        assert_eq!(status.battery_percent, Some(42.0));
    }
}
