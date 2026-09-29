//! #12092: distinguish a clean runtime exit from a durable session-end write.

use super::{
    spawn_background_runtime, spawn_background_runtime_with_timeout, ManagedBackgroundRuntime,
};
use chrono::Utc;
use maekon_core::models::activity::SessionStats;
use maekon_core::ports::storage::MetricsStorage;
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::sqlite::SqliteStorage;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

const SESSION_ID: &str = "shutdown-persistence-control";

struct SessionFixture {
    directory: tempfile::TempDir,
    key: EncryptionKey,
    storage: Arc<SqliteStorage>,
}

impl SessionFixture {
    fn new(runtime: &ManagedBackgroundRuntime) -> Self {
        let directory = tempfile::tempdir().expect("isolated session directory");
        let key = EncryptionKey::from_bytes([37; 32]);
        let storage = Arc::new(
            SqliteStorage::open(&directory.path().join("session.db"), 30, Some(&key))
                .expect("encrypted session database"),
        );
        let mut session = SessionStats::new(SESSION_ID.to_string());
        session.total_events = 3;
        runtime
            .handle()
            .block_on(storage.upsert_session(&session))
            .expect("persist the open session");
        let fixture = Self {
            directory,
            key,
            storage,
        };
        assert!(fixture.reopen_session().ended_at.is_none());
        fixture
    }

    fn reopen_session(&self) -> SessionStats {
        // A fresh SQLCipher connection observes durable data, not a task result
        // or the original connection's in-memory state.
        let reopened = SqliteStorage::open(
            &self.directory.path().join("session.db"),
            30,
            Some(&self.key),
        )
        .expect("reopen the encrypted database");
        let session = reopened
            .list_session_stats(10)
            .expect("read persisted sessions")
            .into_iter()
            .find(|session| session.session_id == SESSION_ID)
            .expect("the exact original session still exists");
        assert_eq!(session.total_events, 3);
        session
    }
}

struct PendingSessionEnd {
    shutdown: watch::Sender<bool>,
    reached_write: mpsc::Receiver<()>,
    release: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

fn pending_session_end(
    runtime: &ManagedBackgroundRuntime,
    storage: Arc<SqliteStorage>,
) -> PendingSessionEnd {
    let (shutdown, mut shutdown_rx) = watch::channel(false);
    let (reached_tx, reached_write) = mpsc::channel();
    let (release, permit) = oneshot::channel();
    let task = runtime.handle().spawn(async move {
        shutdown_rx.changed().await.expect("shutdown signal");
        reached_tx.send(()).expect("observe the pending write");
        // A controlled suspension exposes cancellation before SQLite's actual
        // blocking write. Releasing this same future is the positive control.
        permit.await.expect("write permit");
        storage
            .end_session(SESSION_ID, Utc::now())
            .await
            .expect("persist the session end");
    });
    PendingSessionEnd {
        shutdown,
        reached_write,
        release,
        task,
    }
}

#[test]
fn session_end_untracked_teardown_leaves_durable_session_open() {
    let runtime = spawn_background_runtime().expect("background runtime");
    let fixture = SessionFixture::new(&runtime);
    let pending = pending_session_end(&runtime, fixture.storage.clone());
    pending.shutdown.send(true).expect("send shutdown");
    pending
        .reached_write
        .recv_timeout(Duration::from_secs(2))
        .expect("session-end future reached the write boundary");

    runtime.shutdown_blocking();

    assert!(pending.task.is_finished());
    assert_eq!(
        pending.release.send(()),
        Err(()),
        "the write future was cancelled"
    );
    assert!(fixture.reopen_session().ended_at.is_none());
}

#[test]
fn session_end_store_positive_control_survives_database_reopen() {
    let runtime = spawn_background_runtime().expect("background runtime");
    let fixture = SessionFixture::new(&runtime);
    let pending = pending_session_end(&runtime, fixture.storage.clone());
    pending.shutdown.send(true).expect("send shutdown");
    pending
        .reached_write
        .recv_timeout(Duration::from_secs(2))
        .expect("session-end future reached the write boundary");
    pending.release.send(()).expect("release the same write");
    runtime
        .handle()
        .block_on(pending.task)
        .expect("join writer");
    runtime.shutdown_blocking();

    assert!(fixture.reopen_session().ended_at.is_some());
}

#[test]
fn session_end_tracked_shutdown_finishes_before_database_reopen() {
    let runtime = spawn_background_runtime().expect("background runtime");
    let fixture = SessionFixture::new(&runtime);
    let pending = pending_session_end(&runtime, fixture.storage.clone());
    runtime
        .track_shutdown_task("session-end", pending.task)
        .expect("track the real storage future");
    pending.shutdown.send(true).expect("send shutdown");
    pending
        .reached_write
        .recv_timeout(Duration::from_secs(2))
        .expect("session-end future reached the write boundary");
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        pending.release.send(()).expect("writer remains alive");
    });

    runtime.shutdown_blocking();
    release.join().expect("release thread");

    assert!(fixture.reopen_session().ended_at.is_some());
}

#[test]
fn session_end_timeout_keeps_one_budget_and_unfinished_durable_state() {
    let budget = Duration::from_millis(400);
    let runtime = spawn_background_runtime_with_timeout(budget).expect("background runtime");
    let fixture = SessionFixture::new(&runtime);
    let pending = pending_session_end(&runtime, fixture.storage.clone());
    runtime
        .track_shutdown_task("session-end", pending.task)
        .expect("track the blocked writer");
    pending.shutdown.send(true).expect("send shutdown");
    pending
        .reached_write
        .recv_timeout(Duration::from_secs(2))
        .expect("session-end future reached the write boundary");
    let (started_tx, started_rx) = mpsc::channel();
    let (unblock, blocked) = mpsc::channel();
    runtime.handle().spawn_blocking(move || {
        started_tx.send(()).expect("blocking task started");
        blocked
            .recv_timeout(Duration::from_secs(3))
            .expect("unblock");
    });
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("blocking task is running");
    let started = std::time::Instant::now();

    runtime.shutdown_blocking();

    let elapsed = started.elapsed();
    unblock.send(()).expect("release the owned blocking task");
    assert!(elapsed >= budget);
    // Giving runtime teardown a second full budget takes at least 800 ms.
    assert!(
        elapsed < Duration::from_millis(700),
        "shutdown took {elapsed:?}"
    );
    assert_eq!(pending.release.send(()), Err(()));
    assert!(fixture.reopen_session().ended_at.is_none());
}

#[test]
fn session_end_registration_after_shutdown_is_rejected() {
    let runtime = spawn_background_runtime().expect("background runtime");
    runtime.shutdown_blocking();
    let task = runtime.handle().spawn(async {});
    let error = runtime
        .track_shutdown_task("late-task", task)
        .expect_err("shutdown has already begun");
    assert_eq!(
        error.to_string(),
        "cannot register a task after background shutdown begins"
    );
}
