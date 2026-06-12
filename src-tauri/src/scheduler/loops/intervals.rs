use std::time::Duration;
use tokio::time::{Interval, MissedTickBehavior};

pub(crate) fn coalescing_interval(period: Duration) -> Interval {
    let mut interval = tokio::time::interval(period);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use tokio::time::MissedTickBehavior;

    #[tokio::test]
    async fn scheduler_intervals_skip_missed_ticks() {
        let interval = super::coalescing_interval(Duration::from_secs(1));

        assert_eq!(interval.missed_tick_behavior(), MissedTickBehavior::Skip);
    }
}
