use axum::response::sse::Event;
use futures::stream::Stream;
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::services::stream_assembler::{build_ai_runtime_status_event, build_realtime_event};
use crate::services::web_contexts::RealtimeStreamWebContext;

#[derive(Clone)]
pub struct RealtimeStreamQueryService {
    ctx: RealtimeStreamWebContext,
}

impl RealtimeStreamQueryService {
    pub fn new(ctx: RealtimeStreamWebContext) -> Self {
        Self { ctx }
    }

    pub fn event_stream(&self) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
        let initial_event = self
            .ctx
            .ai_runtime_status
            .clone()
            .and_then(build_ai_runtime_status_event);
        let rx = self.ctx.event_tx.subscribe();
        let live_stream = BroadcastStream::new(rx).filter_map(|result| match result {
            Ok(event) => build_realtime_event(event),
            Err(_) => None,
        });

        tokio_stream::iter(initial_event).chain(live_stream)
    }
}
