use oneshim_api_contracts::idle::IdlePeriodResponse;
use oneshim_core::models::activity::IdlePeriod;

pub(crate) fn assemble_idle_period_response(period: IdlePeriod) -> IdlePeriodResponse {
    IdlePeriodResponse {
        start_time: period.start_time.to_rfc3339(),
        end_time: period.end_time.map(|datetime| datetime.to_rfc3339()),
        duration_secs: period.duration_secs,
    }
}
