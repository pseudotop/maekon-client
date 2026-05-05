//! `CountingStream<S>` — wraps a `Stream` and counts yielded items into an
//! `Arc<AtomicU64>`.
//!
//! Used by streaming RPC handlers (`subscribe_metrics`, `subscribe_events`) so
//! `AuditLayer` can record `response_message_count` in the Completed audit
//! entry. AuditLayer inserts the counter into the request's extensions before
//! the handler runs; the handler pulls it and wraps its outbound stream.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Stream;

pub(crate) struct CountingStream<S> {
    inner: S,
    counter: Arc<AtomicU64>,
}

impl<S> CountingStream<S> {
    pub(crate) fn new(inner: S, counter: Arc<AtomicU64>) -> Self {
        Self { inner, counter }
    }
}

impl<S: Stream + Unpin> Stream for CountingStream<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let item = Pin::new(&mut self.inner).poll_next(cx);
        if let Poll::Ready(Some(_)) = &item {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use futures::StreamExt;

    #[tokio::test]
    async fn counts_messages_on_yield() {
        let inner = stream::iter(vec![1, 2, 3, 4, 5]);
        let counter = Arc::new(AtomicU64::new(0));
        let wrapped = CountingStream::new(inner, counter.clone());
        let _: Vec<i32> = wrapped.collect().await;
        assert_eq!(counter.load(Ordering::Relaxed), 5);
    }

    #[tokio::test]
    async fn empty_stream_counts_zero() {
        let inner = stream::iter(Vec::<i32>::new());
        let counter = Arc::new(AtomicU64::new(0));
        let wrapped = CountingStream::new(inner, counter.clone());
        let _: Vec<i32> = wrapped.collect().await;
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn dropped_mid_stream_preserves_partial_count() {
        let inner = stream::iter(vec![1, 2, 3, 4, 5]);
        let counter = Arc::new(AtomicU64::new(0));
        let mut wrapped = CountingStream::new(inner, counter.clone());
        let _ = wrapped.next().await;
        let _ = wrapped.next().await;
        drop(wrapped);
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }
}
