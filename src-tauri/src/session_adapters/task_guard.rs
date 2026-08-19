use tokio::task::{JoinError, JoinHandle};

pub(crate) struct AbortOnDropJoin<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> AbortOnDropJoin<T> {
    pub(crate) fn new(handle: JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    pub(crate) async fn join(mut self) -> Result<T, JoinError> {
        let Some(handle) = self.handle.take() else {
            panic!("abort-on-drop join handle must be present");
        };
        handle.await
    }
}

impl<T> Drop for AbortOnDropJoin<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}
