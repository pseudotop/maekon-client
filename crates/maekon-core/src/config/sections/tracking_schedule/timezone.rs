use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock};

use chrono::FixedOffset;
use jiff_core::{tz::tzif::TimeZone, Timestamp};

type ZoneCache = BTreeMap<&'static str, OnceLock<Option<TimeZone>>>;

// rc.11 accepted the 597 case-sensitive IANA names in 2025b.
static TRACKING_ZONES: LazyLock<ZoneCache> = LazyLock::new(|| {
    jiff_tzdb::available()
        .filter(|&name| name != "Factory")
        .map(|name| (name, OnceLock::new()))
        .collect()
});

pub(super) fn valid_tracking_zone(name: &str) -> bool {
    TRACKING_ZONES.contains_key(name)
}

pub fn tracking_zone_offset(name: &str, unix_seconds: i64) -> Option<FixedOffset> {
    // rc.11 freezes each zone's final offset after the last transition in 2099.
    const END_OF_2099: i64 = 4_102_444_799;
    let zone = TRACKING_ZONES
        .get(name)?
        .get_or_init(|| TimeZone::parse(jiff_tzdb::get(name)?.1).ok())
        .as_ref()?;
    let seconds = unix_seconds.clamp(Timestamp::MIN.as_second(), END_OF_2099);
    let instant = Timestamp::from_second(seconds).ok()?;
    FixedOffset::east_opt(zone.to_offset(instant).seconds())
}
