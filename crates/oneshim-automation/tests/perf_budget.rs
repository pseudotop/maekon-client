use oneshim_automation::audit::{AuditLogger, AuditStatus};
use std::time::Instant;

fn perf_gates_enabled() -> bool {
    std::env::var("ONESHIM_ENABLE_PERF_GATES")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn perf_budget_ms(env_key: &str, default_ms: u128) -> u128 {
    std::env::var(env_key)
        .ok()
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or(default_ms)
}

#[test]
fn perf_budget_audit_insert_and_query() {
    if !perf_gates_enabled() {
        return;
    }

    let mut logger = AuditLogger::new(50_000, 500);
    let insert_budget_ms = perf_budget_ms("ONESHIM_PERF_BUDGET_AUDIT_INSERT_MS", 1_800);
    let query_budget_ms = perf_budget_ms("ONESHIM_PERF_BUDGET_AUDIT_QUERY_MS", 250);

    let insert_started = Instant::now();
    for _ in 0..20_000 {
        logger.log_start("perf-cmd", "perf-session", "MouseClick");
    }
    let insert_elapsed_ms = insert_started.elapsed().as_millis();

    let query_started = Instant::now();
    let recent = logger.recent_entries(500);
    let started = logger.entries_by_status(&AuditStatus::Started, 500);
    let query_elapsed_ms = query_started.elapsed().as_millis();

    assert_eq!(recent.len(), 500);
    assert_eq!(started.len(), 500);
    assert!(
        insert_elapsed_ms <= insert_budget_ms,
        "audit insert perf budget exceeded: elapsed={}ms budget={}ms",
        insert_elapsed_ms,
        insert_budget_ms
    );
    assert!(
        query_elapsed_ms <= query_budget_ms,
        "audit query perf budget exceeded: elapsed={}ms budget={}ms",
        query_elapsed_ms,
        query_budget_ms
    );
}
