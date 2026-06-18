use std::sync::{Mutex, MutexGuard};

pub(crate) fn lock_or_recover<'a, T>(mutex: &'a Mutex<T>, name: &'static str) -> MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(mutex = name, "recovering poisoned mutex");
            poisoned.into_inner()
        }
    }
}
