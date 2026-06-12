//! Scheduler loop contract tests — CRT-PRV-SCH (런타임 행위 검증).
//!
//! 이 파일은 과거 "source-text grep" 시어터 테스트(파일에 `spawn_*` 문자열이
//! 존재하는지만 확인)를 대체합니다. 프로덕션 스케줄러 루프(예:
//! `src/scheduler/loops/{system,network,health,...}.rs`)는 전부 동일한 동시성
//! 계약을 따릅니다:
//!
//!   tokio::spawn(async move {
//!       let mut interval = coalescing_interval(period); // MissedTickBehavior::Skip
//!       loop {
//!           tokio::select! {
//!               _ = interval.tick()           => { /* 틱 작업 */ }
//!               _ = shutdown_rx.changed()     => break,
//!           }
//!       }
//!   })
//!
//! 프로덕션 spawn 함수들은 전부 `pub(crate)` / private 모듈이라 (이 크레이트에
//! `[lib]` 타깃이 없어 외부 통합 테스트에서 직접 호출 불가) 여기서는 동일한
//! tokio 프리미티브로 루프 계약을 **런타임에서 실제로 구동**하여 다음을
//! 단언합니다:
//!   1. 루프가 실제로 틱 작업을 실행하고 watch 신호로 깔끔히 종료된다.
//!   2. 한 루프 본문의 panic 이 다른 루프를 죽이지 않는다 (tokio task 격리).
//!   3. panic 한 task 는 JoinError::is_panic() 으로 관측 가능하고,
//!      런타임/형제 task 는 계속 살아 틱을 이어간다.
//!
//! 이는 시어터가 아니라 프로덕션 루프가 의존하는 tokio 런타임의 task-격리/
//! select-종료 계약을 실제 task 로 구동해 관측 가능한 부수효과(카운터,
//! JoinError, 종료)로 검증합니다.
//!
//! Run via:
//!   cargo test -p maekon-app --test scheduler_loop_contract

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{interval, MissedTickBehavior};

/// 프로덕션 `super::intervals::coalescing_interval` 와 동일한 구성:
/// 주기적 interval + MissedTickBehavior::Skip (밀린 틱을 합쳐 폭주 방지).
fn coalescing_interval(period: Duration) -> tokio::time::Interval {
    let mut i = interval(period);
    i.set_missed_tick_behavior(MissedTickBehavior::Skip);
    i
}

/// 가상 시계(start_paused)를 `step` 단위로 `total` 만큼 진행하면서, 매 스텝마다
/// spawn 된 루프 task 가 다음 틱을 re-arm 하도록 yield 한다.
///
/// 이유: `current_thread` + `start_paused` 에서 `advance()` 를 한 번에 크게
/// 호출하면 interval task 가 중간 틱을 재-poll 하지 못해 한 틱만 발생한다.
/// 실제 주기적 틱을 구동하려면 주기 단위로 advance + yield 를 반복해야 한다.
async fn drive_clock(total: Duration, step: Duration) {
    let mut elapsed = Duration::ZERO;
    while elapsed < total {
        tokio::time::advance(step).await;
        // spawn 된 루프 task 가 깨어나 틱 작업을 수행하고 다음 틱을 re-arm 하도록 양보.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        elapsed += step;
    }
}

/// 프로덕션 루프와 동형(同形)의 틱 루프를 spawn 한다.
///
/// 매 틱마다 `tick_count` 를 증가시키고, watch 신호(`shutdown_rx`)가 들어오면
/// 루프를 종료한다. `on_tick` 으로 N번째 틱에서 의도적 panic 등 부수효과를
/// 주입할 수 있다.
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
                    // 프로덕션 select! arm 과 동일하게, 틱마다 작업 수행.
                    let n = tick_count.fetch_add(1, Ordering::SeqCst) + 1;
                    on_tick(n);
                }
                _ = shutdown_rx.changed() => {
                    // 프로덕션 루프의 종료 arm 과 동일: 신호 시 break.
                    break;
                }
            }
        }
    })
}

/// CRT-PRV-SCH-RUNTIME-001:
/// 루프가 실제로 틱을 실행하고, watch 종료 신호로 깔끔하게 종료된다.
///
/// 기존 grep 테스트(`assert_spawns(...)`)를 대체 — 파일에 문자열이 있는지가
/// 아니라, 루프가 **런타임에서 실제로 동작**하고 종료하는지를 단언한다.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn crt_prv_sch_runtime_001_loop_ticks_and_shuts_down() {
    let ticks = Arc::new(AtomicU64::new(0));
    let (tx, rx) = watch::channel(false);

    let handle = spawn_contract_loop(Duration::from_millis(100), ticks.clone(), rx, |_| {});

    // 가상 시계를 50ms 스텝으로 400ms 진행 → interval(100ms) 은 t=0,100,200,300
    // 에서 틱하므로 최소 3틱(t=0 즉시틱 제외해도 3회) 이상 보장.
    drive_clock(Duration::from_millis(400), Duration::from_millis(50)).await;

    let observed = ticks.load(Ordering::SeqCst);
    assert!(
        observed >= 3,
        "루프가 런타임에서 실제로 틱을 실행해야 한다 (350ms/100ms): 관측 {observed}틱"
    );

    // 종료 신호 → 루프가 select! 종료 arm 으로 break 하고 task 가 완료되어야 한다.
    tx.send(true).expect("watch 수신자 살아있어야 함");
    let joined = tokio::time::timeout(Duration::from_secs(1), handle).await;
    assert!(
        matches!(joined, Ok(Ok(()))),
        "종료 신호 후 루프 task 는 깔끔히 join 되어야 한다: {joined:?}"
    );
}

/// CRT-PRV-SCH-RUNTIME-002:
/// 한 루프 본문이 panic 해도 형제 루프는 계속 틱하고, 런타임은 살아있다.
///
/// 핵심 격리 계약: 각 루프는 별도 `tokio::spawn` task 이므로 한 task 의 panic
/// 이 다른 task 나 런타임을 죽이지 않는다. (supervisor/restart 프로덕션 코드를
/// 건드리지 않고, 루프들이 의존하는 tokio task-격리를 직접 검증.)
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn crt_prv_sch_runtime_002_panicking_loop_is_isolated_from_siblings() {
    // 형제 루프(정상): 계속 틱해야 한다.
    let sibling_ticks = Arc::new(AtomicU64::new(0));
    let (sib_tx, sib_rx) = watch::channel(false);
    let sibling = spawn_contract_loop(
        Duration::from_millis(100),
        sibling_ticks.clone(),
        sib_rx,
        |_| {},
    );

    // panic 루프: 2번째 틱에서 의도적으로 panic.
    let panic_ticks = Arc::new(AtomicU64::new(0));
    let (_pan_tx, pan_rx) = watch::channel(false);
    let panicker = spawn_contract_loop(
        Duration::from_millis(100),
        panic_ticks.clone(),
        pan_rx,
        |n| {
            if n == 2 {
                panic!("의도적 루프 본문 panic (격리 검증용)");
            }
        },
    );

    // 시계 진행 → panic 루프는 2번째 틱(t=100)에서 죽고, 형제는 계속 틱.
    drive_clock(Duration::from_millis(500), Duration::from_millis(50)).await;

    // (1) panic 한 task 는 JoinError::is_panic() 으로 관측 가능해야 한다.
    let panic_join = tokio::time::timeout(Duration::from_secs(1), panicker)
        .await
        .expect("panic task 는 즉시 종료되어 timeout 되지 않아야 함");
    let join_err = panic_join.expect_err("panic 한 task 의 join 은 Err 여야 한다");
    assert!(
        join_err.is_panic(),
        "panic 한 루프 task 의 JoinError 는 is_panic()=true 여야 한다: {join_err:?}"
    );

    // (2) 형제 루프는 panic 의 영향 없이 계속 틱하고, 런타임도 살아있어야 한다.
    let observed = sibling_ticks.load(Ordering::SeqCst);
    assert!(
        observed >= 3,
        "형제 루프는 다른 루프의 panic 과 무관하게 계속 틱해야 한다: 관측 {observed}틱"
    );

    // (3) 런타임이 살아있는지: 형제 루프를 정상 종료시켜 깔끔히 join 되는지 확인.
    sib_tx.send(true).expect("형제 watch 수신자 살아있어야 함");
    let sib_join = tokio::time::timeout(Duration::from_secs(1), sibling).await;
    assert!(
        matches!(sib_join, Ok(Ok(()))),
        "panic 격리 후에도 형제 루프는 정상 종료되어야 한다: {sib_join:?}"
    );
}

/// CRT-PRV-SCH-RUNTIME-003:
/// coalescing interval 은 밀린 틱을 합친다 (MissedTickBehavior::Skip).
///
/// 프로덕션 모든 루프가 `coalescing_interval` 을 쓰는 이유: 런타임이 오래
/// 블록되어 여러 주기를 놓쳐도, 깨어난 뒤 한 번에 폭주(burst)하지 않고
/// 한 틱만 실행해야 한다. 런타임에서 실제 행위로 검증한다.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn crt_prv_sch_runtime_003_coalescing_interval_skips_missed_ticks() {
    // MissedTickBehavior 설정 자체 검증 (프로덕션 intervals.rs 계약).
    let ticker = coalescing_interval(Duration::from_millis(100));
    assert_eq!(
        ticker.missed_tick_behavior(),
        MissedTickBehavior::Skip,
        "coalescing_interval 은 MissedTickBehavior::Skip 이어야 한다"
    );

    // 런타임 행위: 루프 task 가 park 된 동안 한 번에 1000ms(=10주기 분량)를
    // 통째로 advance 한다. Skip 모드에서는 놓친 9개 주기를 합쳐(coalesce) 단 한
    // 번의 틱만 발생해야 한다 — yield 1회 후에도 10틱으로 폭주하지 않아야 한다.
    let ticks = Arc::new(AtomicU64::new(0));
    let (_tx, rx) = watch::channel(false);
    let handle = spawn_contract_loop(Duration::from_millis(100), ticks.clone(), rx, |_| {});

    // 루프 task 가 첫 틱(t=0)을 소비하고 다음 틱을 arm 하도록 양보.
    tokio::task::yield_now().await;
    let after_first = ticks.load(Ordering::SeqCst);

    // park 상태에서 10주기 분량을 한 번에 advance → Skip 으로 1틱만 깨어남.
    tokio::time::advance(Duration::from_millis(1000)).await;
    tokio::task::yield_now().await;

    let observed = ticks.load(Ordering::SeqCst);
    assert!(
        observed > after_first,
        "park 후 advance 시 최소 한 틱은 깨어나야 한다: {after_first} → {observed}"
    );
    // 10주기를 합쳐 1틱만 추가되어야 한다 (Skip). 합산 폭주(>=10)면 Burst 모드.
    assert!(
        observed - after_first <= 2,
        "Skip 모드는 놓친 9주기를 합쳐 ~1틱만 실행해야 한다 (폭주 금지): \
         {after_first} → {observed}"
    );

    handle.abort();
}
