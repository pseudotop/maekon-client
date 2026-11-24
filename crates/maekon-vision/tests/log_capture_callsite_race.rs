//! A thread-scoped log capture against a callsite that another thread hits
//! first (#13678).
//!
//! tracing-core caches each callsite's interest when the callsite is first hit.
//! While only one dispatcher is registered, it computes that value from the
//! hitting thread's default subscriber alone, so a thread without a subscriber
//! caches "never" for every thread and a `set_default` capture loses the event.
//! The unit test `ocr_search_log_carries_query_length_not_query_text` keeps a
//! second dispatcher alive for that reason. This test pins the race on the same
//! callsite, deterministically.
//!
//! Keep this binary to one test. The race needs `find_element`'s callsites to
//! be unregistered when the test starts, and all tests in a binary share one
//! process.

use std::sync::{Arc, Mutex, PoisonError};

use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::ocr_provider::{FakeOcrProvider, OcrResult};
use maekon_vision::element_finder::OcrElementFinder;

/// Records each event's message.
#[derive(Clone, Default)]
struct MessageCapture(Arc<Mutex<Vec<String>>>);

struct Message<'a>(&'a mut String);

impl tracing::field::Visit for Message<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write as _;
            let _ = write!(self.0, "{value:?}");
        }
    }
}

impl tracing::Subscriber for MessageCapture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut message = String::new();
        event.record(&mut Message(&mut message));
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(message);
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn search_once() -> TestResult {
    let finder = OcrElementFinder::new(Arc::new(FakeOcrProvider::new(vec![OcrResult {
        text: "Save".to_string(),
        x: 10,
        y: 10,
        width: 40,
        height: 20,
        confidence: 0.9,
    }])));
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    runtime.block_on(async {
        finder.set_image(vec![0_u8; 4], "png".to_string()).await;
        finder.find_element(Some("Save"), None, None).await
    })?;
    Ok(())
}

#[test]
fn a_scoped_capture_gets_its_event_after_another_thread_hits_the_callsite() -> TestResult {
    let capture = MessageCapture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    // The same fix as the unit test: a second registered dispatcher.
    let _all_dispatchers = tracing::Dispatch::new(MessageCapture::default());

    // A thread with no subscriber of its own reaches the callsite first.
    std::thread::spawn(search_once)
        .join()
        .expect("the other search thread does not panic")?;
    assert!(
        capture.0.lock().unwrap().is_empty(),
        "the other thread's events are not this thread's"
    );

    search_once()?;

    let messages = capture.0.lock().unwrap().clone();
    assert!(
        messages.iter().any(|m| m == "OCR element search started"),
        "{messages:?}"
    );
    Ok(())
}
