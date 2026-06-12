//! `GuardedConnection` — consent-erasure 차단 chokepoint (#4928 PHASE 1).
//!
//! # 설계 의도 (by-construction completeness)
//!
//! 이전 시도(per-method 가드)는 형제(sibling) writer 를 빠뜨려 누출이 발생했다.
//! 본 newtype 은 **단일 SQLite 커넥션에 도달하는 유일한 경로**를 강제한다:
//! 어떤 어댑터도 barrier-free 한 raw `Mutex<Connection>` 핸들을 얻을 수 없다.
//!
//! `SqliteStorage::connection_arc()` 는 `Arc<GuardedConnection>` 를 반환하고,
//! raw `Arc<Mutex<Connection>>` 접근자는 제거되었다. 모든 공유-커넥션 어댑터
//! (`SqliteSyncMerger`/`SqliteSyncExtractor`/`SqliteRegimeManagerStateStore`/
//! `SqliteVectorStore`/`SqliteVectorIndex`/memory_graph)는 본 funnel 을 통해서만
//! 커넥션에 접근한다.
//!
//! # 동기 + 비-poison (parking_lot)
//!
//! 내부 뮤텍스는 `parking_lot::Mutex` 이다. `.lock()` 은 동기 + infallible 이며,
//! 어떤 스레드(bare-async 호출자 포함)에서도 panic 없이 동작한다. 이는 tokio
//! `RwLock::blocking_read()` 가 런타임 스레드에서 panic 하던 v1 결함(B2)을 제거한다.
//! parking_lot 가드는 절대 `.await` 를 가로질러 보유하지 않는다(동기 rusqlite
//! 연산만 수행 후 즉시 drop).
//!
//! # deletion_flag 재검사 (한 곳에서만)
//!
//! `write_lock()` 은 뮤텍스 획득 *후* `deletion_flag` 를 재검사한다. set 이면
//! no-op sentinel 을 반환하여 쓰기를 건너뛴다(`Ok` 반환, DB 미접근). 이 스킵은
//! **funnel 한 곳에만** 인코딩되어 모든 호출자가 자동으로 상속한다(호출 지점에
//! consent 검사를 흩뿌리지 않는다). 뮤텍스가 모든 SQLite 접근을 직렬화하므로
//! check-and-write 는 erase 의 wipe 에 대해 원자적이다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use rusqlite::Connection;

/// consent-erasure 차단 chokepoint 커넥션 래퍼.
///
/// `conn` 은 모든 SQLite 접근을 직렬화하는 단일 `parking_lot::Mutex` 이다.
/// `deletion_flag` 는 consent revoke 시 `true` 로 설정되는 공유 신호로,
/// PHASE 2 에서 `ConsentManager`/`revoke_consent` 가 erase 직전에 set 한다.
///
/// # 왜 `ArcSwap<AtomicBool>` 인가 (PHASE 2 seam)
///
/// `SqliteStorage.conn: Arc<GuardedConnection>` 이므로, 일단 `Arc` 로 감싸지면
/// `&mut self` 를 얻을 수 없어 `deletion_flag` 필드를 직접 교체할 수 없다. 실제
/// composition root 에서는 `SqliteStorage` 가 `ConsentManager` 보다 먼저 생성되므로
/// (storage_runtime → capture_wiring 순서) "construction 시점 주입"만으로는 동일
/// `Arc` 공유가 불가능하다. 따라서 flag 셀 자체를 `ArcSwap` 으로 만들어 `&self` 로도
/// 공유 `Arc<AtomicBool>` 를 **교체(install)** 할 수 있게 한다 (스펙 §3bis seam
/// 보강: "the field must be swappable via arc_swap").
pub struct GuardedConnection {
    conn: Mutex<Connection>,
    deletion_flag: ArcSwap<AtomicBool>,
    /// #4928 round-3 (FIX B): grant_consent-during-erase TOCTOU 차단 신호.
    ///
    /// `deletion_flag` 는 `grant_consent` 가 `false` 로 되돌릴 수 있어, erase 윈도우
    /// (Phase-1 커밋 후 ~ Phase-2 진행 중) 안에 재동의가 끼어들면 in-flight writer 가
    /// flag 가 clear 된 것을 보고 wipe 이후 잔존 행을 쓸 수 있다. `erasing` 은
    /// `erase_all_local_data` 가 시작 시 set 하고 RAII Drop 으로 모든 종료 경로(성공/
    /// 에러/패닉)에서 clear 하며, `grant_consent` 는 절대 건드리지 못한다. 따라서 재동의
    /// race 가 발생해도 erase 가 실제로 끝날 때까지 쓰기가 재개되지 않는다.
    ///
    /// write skip 술어는 `deletion_flag || erasing` 이다(둘 중 하나라도 set 이면 스킵).
    erasing: ArcSwap<AtomicBool>,
}

impl GuardedConnection {
    /// 공유 `deletion_flag` 를 받아 래퍼를 생성한다.
    ///
    /// PHASE 2(erase wiring)에서 composition root 가 `ConsentManager` 와 동일한
    /// `Arc<AtomicBool>` 를 주입한다.
    pub fn new(conn: Connection, deletion_flag: Arc<AtomicBool>) -> Self {
        Self {
            conn: Mutex::new(conn),
            deletion_flag: ArcSwap::new(deletion_flag),
            // #4928 round-3: 기본값 false. composition root 가 공유 `erasing` 을 install.
            erasing: ArcSwap::new(Arc::new(AtomicBool::new(false))),
        }
    }

    /// 새 `deletion_flag`(기본값 `false`)로 래퍼를 생성한다.
    ///
    /// erase wiring 이 없는 호출자(스토리지 단위 테스트, non-erase 경로)를 위한
    /// 기본 생성자. PHASE 2 에서 [`Self::set_deletion_flag`] 로 공유 flag 를 연결한다.
    pub fn new_unflagged(conn: Connection) -> Self {
        Self::new(conn, Arc::new(AtomicBool::new(false)))
    }

    /// 공유 `deletion_flag` 를 교체한다(PHASE 2 composition-root 배선용 seam).
    ///
    /// `Arc<GuardedConnection>` 를 통해 `&self` 만으로 호출 가능하다 (`ArcSwap`).
    /// composition root 가 `ConsentManager::deletion_flag()` 를 install 하면 이후
    /// 모든 `write_lock` 재검사가 동일 셀을 본다.
    pub fn set_deletion_flag(&self, flag: Arc<AtomicBool>) {
        self.deletion_flag.store(flag);
    }

    /// 현재 install 된 `deletion_flag` 를 복제하여 반환한다(ptr-eq 공유 검증/테스트용).
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.deletion_flag.load_full()
    }

    /// #4928 round-3 (FIX B): 공유 `erasing` 신호를 교체한다(composition-root 배선용 seam).
    ///
    /// `set_deletion_flag` 와 동일하게 `&self` 로 동작하며(`ArcSwap`), composition root 가
    /// `ConsentManager::erasing()`(= `FrameFileStorage` 와 동일 Arc)를 install 한다.
    pub fn set_erasing(&self, erasing: Arc<AtomicBool>) {
        self.erasing.store(erasing);
    }

    /// #4928 round-3: 현재 install 된 `erasing` 신호를 복제하여 반환한다(ptr-eq 검증/테스트용).
    pub fn erasing(&self) -> Arc<AtomicBool> {
        self.erasing.load_full()
    }

    /// 쓰기 funnel. 뮤텍스 획득 후 skip 술어 `deletion_flag || erasing` 를 재검사한다.
    ///
    /// 둘 중 하나라도 set 이면 [`WriteGuard::Skipped`] 를 반환하여 쓰기를 no-op 으로
    /// 만든다. 둘 다 clear 이면 [`WriteGuard::Active`] 가 커넥션 락을 보유한다.
    ///
    /// #4928 round-3 (FIX B): `erasing` 은 `grant_consent` 가 clear 할 수 없으므로,
    /// erase 윈도우 안에서 재동의가 `deletion_flag` 를 false 로 되돌려도 erase 가 실제로
    /// 끝날 때까지 in-flight 쓰기가 스킵된다(TOCTOU 차단).
    pub fn write_lock(&self) -> WriteGuard<'_> {
        let guard = self.conn.lock();
        // 락 획득 *후* 재검사 — erase 의 delete 와 직렬화되어 원자적이다.
        let skip = self.deletion_flag.load().load(Ordering::Acquire)
            || self.erasing.load().load(Ordering::Acquire);
        if skip {
            WriteGuard::Skipped
        } else {
            WriteGuard::Active(guard)
        }
    }

    /// 읽기 funnel. 뮤텍스만 획득하며 deletion_flag 를 검사하지 않는다(읽기는 절대 스킵 안 함).
    pub fn read_lock(&self) -> ReadGuard<'_> {
        ReadGuard {
            guard: self.conn.lock(),
        }
    }

    /// 보존(retained) 테이블 쓰기 funnel — deletion_flag 재검사 없음.
    ///
    /// `audit_log`/`session_audit_log`/`egress_ledger`/`app_meta` 등 erase 가
    /// 의도적으로 보존하는 테이블에 erase 도중/이후에도 기록해야 하는 writer 용.
    pub fn retained_write_lock(&self) -> RetainedGuard<'_> {
        RetainedGuard {
            guard: self.conn.lock(),
        }
    }

    /// 테스트 전용 직접 락 — funnel 우회. 프로덕션 코드에서는 절대 사용 금지
    /// (CI gate 가 `*/tests.rs`/`#[cfg(test)]` 만 면제한다). 기존 테스트가
    /// `storage.conn.lock()` 처럼 raw 커넥션을 다루던 패턴을 보존하기 위한 헬퍼.
    #[cfg(test)]
    pub(crate) fn test_lock(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.conn.lock()
    }

    /// erase 본체(`delete_all_data`)가 사용하는 락 — deletion_flag 재검사 없음.
    ///
    /// erase 는 flag 가 set 인 상태에서 실행되므로 재검사하면 자기 자신이 스킵되어
    /// wipe 가 일어나지 않는다. [`Self::retained_write_lock`] 와 동일 동작이며 의미
    /// 구분을 위해 별도 명명한다.
    pub fn lock_for_erase(&self) -> RetainedGuard<'_> {
        self.retained_write_lock()
    }
}

/// [`GuardedConnection::write_lock`] 가 반환하는 쓰기 가드.
///
/// `Active` 는 커넥션 락을 보유하고, `Skipped` 는 deletion_flag set 으로 인해
/// 락을 보유하지 않는 no-op sentinel 이다.
pub enum WriteGuard<'a> {
    /// deletion_flag clear — 커넥션 락 보유.
    Active(parking_lot::MutexGuard<'a, Connection>),
    /// deletion_flag set — no-op. 클로저는 실행되지 않는다.
    Skipped,
}

impl WriteGuard<'_> {
    /// `&Connection` 에 대한 클로저를 실행한다.
    ///
    /// `Skipped`(deletion_flag set)이면 `f` 를 실행하지 않고 `Ok(skipped)` 를 반환한다.
    /// `Active` 면 보유한 커넥션에서 `f` 를 실행한다.
    pub fn run<F, T, E>(self, skipped: T, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
    {
        match self {
            WriteGuard::Active(guard) => f(&guard),
            WriteGuard::Skipped => Ok(skipped),
        }
    }

    /// `&mut Connection` 에 대한 클로저를 실행한다(트랜잭션 등 가변 접근).
    ///
    /// `Skipped` 이면 `f` 를 실행하지 않고 `Ok(skipped)` 를 반환한다.
    pub fn run_mut<F, T, E>(mut self, skipped: T, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Connection) -> Result<T, E>,
    {
        match &mut self {
            WriteGuard::Active(guard) => f(guard),
            WriteGuard::Skipped => Ok(skipped),
        }
    }

    /// deletion_flag 로 인해 스킵되었는지 여부.
    pub fn is_skipped(&self) -> bool {
        matches!(self, WriteGuard::Skipped)
    }
}

/// [`GuardedConnection::read_lock`] 가 반환하는 읽기 가드.
///
/// `&Connection` 을 노출한다. 읽기는 절대 스킵하지 않는다.
///
/// # ⚠️ 읽기 전용 (CI gate 강제)
///
/// rusqlite `&Connection` 은 기술적으로 `execute(...)` 도 가능하지만, **이 가드를 통한
/// 쓰기는 금지**된다 — 쓰기는 반드시 [`GuardedConnection::write_lock`] funnel 을 거쳐야
/// `deletion_flag || erasing` 재검사(consent-erasure 차단)를 받는다. read 경로로 mutation
/// 을 밀반입하면 그 쓰기는 erasure 배리어를 우회한다. `read_lock()` scope 안에 write-SQL
/// 동사가 등장하면 `scripts/check-consent-erasure-barrier.sh` CI gate 가 차단한다
/// (#4928 round-3 FIX A).
pub struct ReadGuard<'a> {
    guard: parking_lot::MutexGuard<'a, Connection>,
}

impl ReadGuard<'_> {
    /// `&Connection` 에 대한 읽기 클로저를 실행한다.
    pub fn run<F, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
    {
        f(&self.guard)
    }

    /// 읽기 전용 `&Connection` 참조를 반환한다(인라인 SELECT 호출용).
    pub fn conn(&self) -> &Connection {
        &self.guard
    }
}

/// [`GuardedConnection::retained_write_lock`]/[`GuardedConnection::lock_for_erase`]
/// 가 반환하는 가드 — deletion_flag 검사 없이 항상 기록한다.
pub struct RetainedGuard<'a> {
    guard: parking_lot::MutexGuard<'a, Connection>,
}

impl RetainedGuard<'_> {
    /// `&Connection` 에 대한 클로저를 실행한다(항상 실행, 스킵 없음).
    pub fn run<F, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
    {
        f(&self.guard)
    }

    /// `&mut Connection` 에 대한 클로저를 실행한다(트랜잭션 등, 항상 실행).
    pub fn run_mut<F, T, E>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Connection) -> Result<T, E>,
    {
        f(&mut self.guard)
    }

    /// `&Connection` 참조를 반환한다(인라인 호출용).
    pub fn conn(&self) -> &Connection {
        &self.guard
    }

    /// `&mut Connection` 참조를 반환한다(인라인 트랜잭션용).
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.guard
    }
}

// RetainedGuard 는 deletion_flag 를 검사하지 않는 "항상 실행" 가드이므로,
// 인라인 `guard.execute(...)`/`guard.transaction()` 호출 편의를 위해 Deref 를 제공한다.
// (write/read 가드는 deletion-skip / 읽기전용 의미를 강제하기 위해 Deref 를 제공하지 않는다.)
impl std::ops::Deref for RetainedGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.guard
    }
}

impl std::ops::DerefMut for RetainedGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> GuardedConnection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);")
            .unwrap();
        GuardedConnection::new_unflagged(conn)
    }

    fn count(gc: &GuardedConnection) -> i64 {
        gc.read_lock()
            .run::<_, i64, rusqlite::Error>(|c| {
                c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            })
            .unwrap()
    }

    #[test]
    fn write_lock_writes_when_flag_clear() {
        let gc = fresh();
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('a')", []));
        assert_eq!(res.unwrap(), 1);
        assert_eq!(count(&gc), 1);
    }

    #[test]
    fn write_lock_skips_when_flag_set() {
        let gc = fresh();
        gc.deletion_flag().store(true, Ordering::Release);
        // 클로저는 실행되지 않아야 하며(0 반환), row 수는 변하지 않아야 한다.
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('b')", []));
        assert_eq!(res.unwrap(), 0, "skipped write returns the skip sentinel");
        assert_eq!(count(&gc), 0, "skipped write must not touch the DB");
    }

    #[test]
    fn write_lock_run_mut_skips_when_flag_set() {
        let gc = fresh();
        gc.deletion_flag().store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc.write_lock().run_mut(0, |c| {
            let tx = c.transaction()?;
            tx.execute("INSERT INTO t (v) VALUES ('c')", [])?;
            tx.commit()?;
            Ok(1)
        });
        assert_eq!(res.unwrap(), 0);
        assert_eq!(count(&gc), 0);
    }

    #[test]
    fn read_lock_never_skips() {
        let gc = fresh();
        gc.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute("INSERT INTO t (v) VALUES ('a')", [])
            })
            .unwrap();
        // flag set 이어도 읽기는 정상 동작한다.
        gc.deletion_flag().store(true, Ordering::Release);
        assert_eq!(count(&gc), 1);
    }

    #[test]
    fn retained_write_lock_writes_even_when_flag_set() {
        let gc = fresh();
        gc.deletion_flag().store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc
            .retained_write_lock()
            .run(|c| c.execute("INSERT INTO t (v) VALUES ('retained')", []));
        assert_eq!(
            res.unwrap(),
            1,
            "retained write must bypass the deletion flag"
        );
        assert_eq!(count(&gc), 1);
    }

    #[test]
    fn lock_for_erase_runs_when_flag_set() {
        let gc = fresh();
        gc.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute("INSERT INTO t (v) VALUES ('a')", [])
            })
            .unwrap();
        gc.deletion_flag().store(true, Ordering::Release);
        // erase 본체는 flag set 상태에서도 실행되어야 한다.
        gc.lock_for_erase()
            .run_mut::<_, (), rusqlite::Error>(|c| {
                let tx = c.transaction()?;
                tx.execute("DELETE FROM t", [])?;
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        assert_eq!(count(&gc), 0, "erase must wipe regardless of the flag");
    }

    #[test]
    fn set_deletion_flag_shares_signal() {
        let gc = fresh();
        let shared = Arc::new(AtomicBool::new(false));
        // #4928 PHASE 2: set_deletion_flag 는 이제 &self 로 동작한다 (ArcSwap).
        gc.set_deletion_flag(shared.clone());
        // 외부에서 공유 flag 를 set 하면 funnel 이 스킵해야 한다.
        shared.store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('x')", []));
        assert_eq!(res.unwrap(), 0);
        assert_eq!(count(&gc), 0);
    }

    /// #4928 round-3 (FIX B): `erasing` 이 set 이면 deletion_flag 가 clear 여도 write 가
    /// 스킵되어야 한다(grant_consent-during-erase TOCTOU 차단의 funnel-level 증명).
    #[test]
    fn write_lock_skips_when_erasing_set_even_if_deletion_flag_clear() {
        let gc = fresh();
        let erasing = Arc::new(AtomicBool::new(false));
        gc.set_erasing(erasing.clone());
        // deletion_flag 는 clear, erasing 만 set.
        assert!(!gc.deletion_flag().load(Ordering::Acquire));
        erasing.store(true, Ordering::Release);

        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('e')", []));
        assert_eq!(res.unwrap(), 0, "erasing set 시 write 는 스킵되어야 한다");
        assert_eq!(count(&gc), 0);

        // erasing clear 후에는 다시 쓰기가 재개된다(deletion_flag 도 clear 이므로).
        erasing.store(false, Ordering::Release);
        let res2: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('ok')", []));
        assert_eq!(res2.unwrap(), 1);
        assert_eq!(count(&gc), 1);
    }

    /// #4928 round-3 (FIX B): `set_erasing` 이 install 한 셀이 ptr-eq 로 공유되는지 검증.
    #[test]
    fn set_erasing_through_arc_is_ptr_eq() {
        let gc = Arc::new(fresh());
        let shared = Arc::new(AtomicBool::new(false));
        gc.set_erasing(shared.clone());
        let observed = gc.erasing();
        assert!(
            Arc::ptr_eq(&observed, &shared),
            "install 된 erasing 과 erasing() 반환값은 동일 Arc 여야 한다 (ptr-eq)"
        );
    }

    /// #4928 round-3 (FIX B): read 는 erasing set 이어도 절대 스킵하지 않는다.
    #[test]
    fn read_lock_never_skips_even_when_erasing_set() {
        let gc = fresh();
        gc.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute("INSERT INTO t (v) VALUES ('a')", [])
            })
            .unwrap();
        let erasing = Arc::new(AtomicBool::new(true));
        gc.set_erasing(erasing);
        assert_eq!(
            count(&gc),
            1,
            "erasing set 이어도 읽기는 정상 동작해야 한다"
        );
    }

    /// #4928 PHASE 2: `set_deletion_flag` 를 `Arc<GuardedConnection>` 를 통해 `&self`
    /// 로 호출할 수 있고, install 후 `deletion_flag()` 가 동일 `Arc` (ptr-eq)를
    /// 반환하는지 검증한다 — seam 이 LIVE flag 를 실제로 재배선함을 증명한다.
    #[test]
    fn set_deletion_flag_through_arc_is_ptr_eq() {
        let gc = Arc::new(fresh());
        let shared = Arc::new(AtomicBool::new(false));
        // &self 로 호출 (Arc 안에서) — &mut 가 아니어도 교체된다.
        gc.set_deletion_flag(shared.clone());
        let observed = gc.deletion_flag();
        assert!(
            Arc::ptr_eq(&observed, &shared),
            "install 된 flag 와 deletion_flag() 반환값은 동일 Arc 여야 한다 (ptr-eq)"
        );
        // install 한 셀을 set 하면 funnel 이 즉시 스킵한다.
        shared.store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('y')", []));
        assert_eq!(res.unwrap(), 0);
        assert_eq!(count(&gc), 0);
    }
}
