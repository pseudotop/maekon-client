//! Scheduler loop contract tests — CRT-PRV-SCH (runtime behavior verification).
//!
//! This file replaces the old "source-text grep" theater tests (which only
//! checked whether a `spawn_*` string was present in a file). The production
//! scheduler loops (e.g. `src/scheduler/loops/{system,network,health,...}.rs`)
//! all follow the same concurrency contract:
//!
//!   tokio::spawn(async move {
//!       let mut interval = coalescing_interval(period); // MissedTickBehavior::Skip
//!       loop {
//!           tokio::select! {
//!               _ = interval.tick()           => { /* tick work */ }
//!               _ = shutdown_rx.changed()     => break,
//!           }
//!       }
//!   })
//!
//! The production spawn functions are all in `pub(crate)` / private modules (this
//! crate has no `[lib]` target, so they cannot be called directly from external
//! integration tests). So here we **actually drive the loop contract at runtime**
//! with the same tokio primitives and assert:
//!   1. The loop actually runs tick work and shuts down cleanly on a watch signal.
//!   2. A panic in one loop body does not kill another loop (tokio task isolation).
//!   3. A panicked task is observable via JoinError::is_panic(), while the
//!      runtime/sibling tasks keep living and continue ticking.
//!
//! This is not theater: it drives the tokio runtime's task-isolation /
//! select-termination contract — which the production loops depend on — with real
//! tasks, and verifies it via observable side effects (counters, JoinError,
//! termination).
//!
//! Run via:
//!   cargo test -p maekon-app --test scheduler_loop_contract

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{interval, MissedTickBehavior};

/// Same configuration as production `super::intervals::coalescing_interval`:
/// a periodic interval + MissedTickBehavior::Skip (coalesces missed ticks to
/// prevent bursts).
fn coalescing_interval(period: Duration) -> tokio::time::Interval {
    let mut i = interval(period);
    i.set_missed_tick_behavior(MissedTickBehavior::Skip);
    i
}

/// Advances the virtual clock (start_paused) by `total` in `step` increments,
/// yielding on each step so the spawned loop task can re-arm its next tick.
///
/// Why: under `current_thread` + `start_paused`, calling `advance()` once with a
/// large value means the interval task cannot re-poll the intermediate ticks, so
/// only a single tick fires. To drive real periodic ticks, advance + yield must be
/// repeated per period.
async fn drive_clock(total: Duration, step: Duration) {
    let mut elapsed = Duration::ZERO;
    while elapsed < total {
        tokio::time::advance(step).await;
        // Yield so the spawned loop task wakes up, performs its tick work, and re-arms the next tick.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        elapsed += step;
    }
}

/// Spawns a tick loop isomorphic to the production loops.
///
/// Increments `tick_count` on every tick and terminates the loop when the watch
/// signal (`shutdown_rx`) arrives. `on_tick` can inject side effects such as an
/// intentional panic on the Nth tick.
fn spawn_contract_loop<F>(
    period: Duration,
    tick_count: Arc<AtomicU64>,
    mut shutdown_rx: watch::Receiver<bool>,
    mut on_tick: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut(u64) + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = coalescing_interval(period);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Same as the production select! arm: perform work on every tick.
                    let n = tick_count.fetch_add(1, Ordering::SeqCst) + 1;
                    on_tick(n);
                }
                _ = shutdown_rx.changed() => {
                    // Same as the production loop's termination arm: break on signal.
                    break;
                }
            }
        }
    })
}

/// CRT-PRV-SCH-RUNTIME-001:
/// The loop actually runs ticks and shuts down cleanly on a watch termination signal.
///
/// Replaces the old grep test (`assert_spawns(...)`) — instead of checking whether
/// a string is present in a file, it asserts that the loop **actually runs at
/// runtime** and terminates.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn crt_prv_sch_runtime_001_loop_ticks_and_shuts_down() {
    let ticks = Arc::new(AtomicU64::new(0));
    let (tx, rx) = watch::channel(false);

    let handle = spawn_contract_loop(Duration::from_millis(100), ticks.clone(), rx, |_| {});

    // Advance the virtual clock by 400ms in 50ms steps → interval(100ms) ticks at
    // t=0,100,200,300, guaranteeing at least 3 ticks (3 even if the t=0 immediate
    // tick is excluded).
    drive_clock(Duration::from_millis(400), Duration::from_millis(50)).await;

    let observed = ticks.load(Ordering::SeqCst);
    assert!(
        observed >= 3,
        "the loop must actually run ticks at runtime (350ms/100ms): observed {observed} ticks"
    );

    // Termination signal → the loop must break via the select! termination arm and the task must complete.
    tx.send(true).expect("watch receiver must be alive");
    let joined = tokio::time::timeout(Duration::from_secs(1), handle).await;
    assert!(
        matches!(joined, Ok(Ok(()))),
        "after the termination signal the loop task must join cleanly: {joined:?}"
    );
}

/// CRT-PRV-SCH-RUNTIME-002:
/// Even if one loop body panics, sibling loops keep ticking and the runtime stays alive.
///
/// Core isolation contract: each loop is a separate `tokio::spawn` task, so a panic
/// in one task does not kill another task or the runtime. (Verifies the tokio
/// task-isolation that the loops depend on directly, without touching the
/// supervisor/restart production code.)
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn crt_prv_sch_runtime_002_panicking_loop_is_isolated_from_siblings() {
    // Sibling loop (healthy): must keep ticking.
    let sibling_ticks = Arc::new(AtomicU64::new(0));
    let (sib_tx, sib_rx) = watch::channel(false);
    let sibling = spawn_contract_loop(
        Duration::from_millis(100),
        sibling_ticks.clone(),
        sib_rx,
        |_| {},
    );

    // Panic loop: intentionally panics on the 2nd tick.
    let panic_ticks = Arc::new(AtomicU64::new(0));
    let (_pan_tx, pan_rx) = watch::channel(false);
    let panicker = spawn_contract_loop(
        Duration::from_millis(100),
        panic_ticks.clone(),
        pan_rx,
        |n| {
            if n == 2 {
                panic!("intentional loop-body panic (for isolation verification)");
            }
        },
    );

    // Advance the clock → the panic loop dies on the 2nd tick (t=100), the sibling keeps ticking.
    drive_clock(Duration::from_millis(500), Duration::from_millis(50)).await;

    // (1) The panicked task must be observable via JoinError::is_panic().
    let panic_join = tokio::time::timeout(Duration::from_secs(1), panicker)
        .await
        .expect("the panic task must terminate immediately and not time out");
    let join_err = panic_join.expect_err("the join of a panicked task must be Err");
    assert!(
        join_err.is_panic(),
        "the JoinError of a panicked loop task must have is_panic()=true: {join_err:?}"
    );

    // (2) The sibling loop must keep ticking unaffected by the panic, and the runtime must stay alive.
    let observed = sibling_ticks.load(Ordering::SeqCst);
    assert!(
        observed >= 3,
        "the sibling loop must keep ticking regardless of another loop's panic: observed {observed} ticks"
    );

    // (3) Confirm the runtime is alive: terminate the sibling loop normally and verify it joins cleanly.
    sib_tx
        .send(true)
        .expect("sibling watch receiver must be alive");
    let sib_join = tokio::time::timeout(Duration::from_secs(1), sibling).await;
    assert!(
        matches!(sib_join, Ok(Ok(()))),
        "even after panic isolation the sibling loop must terminate normally: {sib_join:?}"
    );
}

/// CRT-PRV-SCH-RUNTIME-003:
/// The coalescing interval coalesces missed ticks (MissedTickBehavior::Skip).
///
/// Why every production loop uses `coalescing_interval`: even if the runtime is
/// blocked for a long time and misses several periods, after waking up it must run
/// only one tick instead of bursting all at once. Verified by real behavior at
/// runtime.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn crt_prv_sch_runtime_003_coalescing_interval_skips_missed_ticks() {
    // Verify the MissedTickBehavior setting itself (production intervals.rs contract).
    let ticker = coalescing_interval(Duration::from_millis(100));
    assert_eq!(
        ticker.missed_tick_behavior(),
        MissedTickBehavior::Skip,
        "coalescing_interval must be MissedTickBehavior::Skip"
    );

    // Runtime behavior: while the loop task is parked, advance 1000ms (= 10 periods'
    // worth) all at once. In Skip mode the 9 missed periods must be coalesced into a
    // single tick — even after one yield, it must not burst to 10 ticks.
    let ticks = Arc::new(AtomicU64::new(0));
    let (_tx, rx) = watch::channel(false);
    let handle = spawn_contract_loop(Duration::from_millis(100), ticks.clone(), rx, |_| {});

    // Yield so the loop task consumes the first tick (t=0) and arms the next tick.
    tokio::task::yield_now().await;
    let after_first = ticks.load(Ordering::SeqCst);

    // While parked, advance 10 periods' worth at once → only 1 tick wakes up under Skip.
    tokio::time::advance(Duration::from_millis(1000)).await;
    tokio::task::yield_now().await;

    let observed = ticks.load(Ordering::SeqCst);
    assert!(
        observed > after_first,
        "advancing after a park must wake at least one tick: {after_first} → {observed}"
    );
    // Only 1 tick must be added by coalescing 10 periods (Skip). An aggregate burst (>=10) means Burst mode.
    assert!(
        observed - after_first <= 2,
        "Skip mode must coalesce the 9 missed periods into ~1 tick (no burst): \
         {after_first} → {observed}"
    );

    handle.abort();
}
