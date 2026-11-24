//! `element_finder.rs` tests, split out under ADR-013 so the parent file stays
//! under the 900-line cap without a baseline entry.

use super::*;

#[test]
fn text_similarity_exact_match() {
    assert!((text_similarity("save", "save") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn text_similarity_partial_match() {
    let sim = text_similarity("file save", "save");
    assert!(sim > 0.0);
    assert!(sim < 1.0);
}

#[test]
fn text_similarity_no_match() {
    assert!((text_similarity("save", "open")).abs() < f64::EPSILON);
}

#[test]
fn text_similarity_case_insensitive() {
    assert!((text_similarity("Save", "save") - 1.0).abs() < f64::EPSILON);
}

#[test]
fn ocr_to_scene_elements_builds_normalized_coordinates() {
    let results = vec![OcrResult {
        text: "Save".to_string(),
        x: 192,
        y: 108,
        width: 96,
        height: 54,
        confidence: 0.88,
    }];

    let scene_elements = OcrElementFinder::ocr_to_scene_elements(
        &results,
        1920,
        1080,
        Some("VSCode"),
        Some("m1"),
        PiiFilterLevel::Standard,
    );
    assert_eq!(scene_elements.len(), 1);
    let first = &scene_elements[0];
    assert_eq!(first.label, "Save");
    assert!((first.bbox_norm.x - 0.1).abs() < 1e-6);
    assert!((first.bbox_norm.y - 0.1).abs() < 1e-6);
    assert!(first.text_masked.is_some());
}

#[test]
fn ocr_to_elements_text_filter() {
    let results = vec![
        OcrResult {
            text: "file".to_string(),
            x: 0,
            y: 0,
            width: 40,
            height: 20,
            confidence: 0.9,
        },
        OcrResult {
            text: "save".to_string(),
            x: 50,
            y: 0,
            width: 40,
            height: 20,
            confidence: 0.85,
        },
        OcrResult {
            text: "edit".to_string(),
            x: 100,
            y: 0,
            width: 40,
            height: 20,
            confidence: 0.88,
        },
    ];

    let elements = OcrElementFinder::ocr_to_elements(&results, Some("save"), None, None);
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].text, "save");
}

#[test]
fn ocr_to_elements_region_filter() {
    let results = vec![
        OcrResult {
            text: "file".to_string(),
            x: 0,
            y: 0,
            width: 40,
            height: 20,
            confidence: 0.9,
        },
        OcrResult {
            text: "save".to_string(),
            x: 200,
            y: 200,
            width: 40,
            height: 20,
            confidence: 0.85,
        },
    ];

    let region = ElementBounds {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };
    let elements = OcrElementFinder::ocr_to_elements(&results, None, None, Some(&region));
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].text, "file");
}

#[test]
fn ocr_to_elements_all_filtered() {
    let results = vec![OcrResult {
        text: "file".to_string(),
        x: 0,
        y: 0,
        width: 40,
        height: 20,
        confidence: 0.9,
    }];

    let elements = OcrElementFinder::ocr_to_elements(&results, Some("no-text"), None, None);
    assert!(elements.is_empty());
}

#[test]
fn ocr_to_elements_no_query_returns_all() {
    let results = vec![
        OcrResult {
            text: "A".to_string(),
            x: 0,
            y: 0,
            width: 20,
            height: 20,
            confidence: 0.8,
        },
        OcrResult {
            text: "B".to_string(),
            x: 30,
            y: 0,
            width: 20,
            height: 20,
            confidence: 0.9,
        },
    ];

    let elements = OcrElementFinder::ocr_to_elements(&results, None, None, None);
    assert_eq!(elements.len(), 2);
}

struct MockFinder {
    name: String,
    results: Vec<UiElement>,
}

#[async_trait]
impl ElementFinder for MockFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        Ok(self.results.clone())
    }
    fn name(&self) -> &str {
        &self.name
    }
}

struct FailingFinder;

#[async_trait]
impl ElementFinder for FailingFinder {
    async fn find_element(
        &self,
        _text: Option<&str>,
        _role: Option<&str>,
        _region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        Err(CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "Search failed".to_string(),
        })
    }
    fn name(&self) -> &str {
        "failing"
    }
}

/// #12515: the OCR search log names the query only by its length.
#[tokio::test]
async fn ocr_search_log_carries_query_length_not_query_text() {
    /// Records each event's fields as ` name=value` text, one event per line.
    #[derive(Clone, Default)]
    struct FieldCapture(Arc<std::sync::Mutex<String>>);

    struct FieldLine<'a>(&'a mut String);

    impl tracing::field::Visit for FieldLine<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write as _;
            let _ = write!(self.0, " {}={:?}", field.name(), value);
        }
    }

    impl tracing::Subscriber for FieldCapture {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut line = String::new();
            event.record(&mut FieldLine(&mut line));
            line.push('\n');
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_str(&line);
        }
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    const QUERY: &str = "Jane Roe account 4471";
    let capture = FieldCapture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    // tracing-core caches each callsite's interest when the callsite is first
    // hit. While only one dispatcher is registered, it computes that cache from
    // the hitting thread's default, so a parallel test reaching `find_element`'s
    // callsite first caches "never" and this thread loses the event. A second
    // registered dispatcher makes every later computation consult all live
    // dispatchers, and registering it rebuilds any value cached before.
    let _all_dispatchers = tracing::Dispatch::new(FieldCapture::default());

    let provider = maekon_core::ports::ocr_provider::FakeOcrProvider::new(vec![OcrResult {
        text: QUERY.to_string(),
        x: 10,
        y: 10,
        width: 120,
        height: 20,
        confidence: 0.9,
    }]);
    let finder = OcrElementFinder::new(Arc::new(provider));
    finder.set_image(vec![0_u8; 4], "png".to_string()).await;
    finder
        .find_element(Some(QUERY), Some("button"), None)
        .await
        .expect("the fake provider answers");

    let logs = capture.0.lock().unwrap().clone();
    assert!(logs.contains("OCR element search started"), "{logs}");
    assert!(
        !logs.contains("Jane Roe"),
        "query text reached the log: {logs}"
    );
    assert!(
        logs.contains(&format!("text_len=Some({})", QUERY.len())),
        "{logs}"
    );
}

fn word(text: &str, x: i32) -> OcrResult {
    OcrResult {
        text: text.to_string(),
        x,
        y: 0,
        width: 40,
        height: 20,
        confidence: 0.9,
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(width, height)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

/// Lets a test hold an OCR call open while the screen changes.
struct OcrGate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

/// Counts provider calls and answers `results` for every image.
struct CountingOcr {
    results: Vec<OcrResult>,
    external: bool,
    calls: std::sync::atomic::AtomicUsize,
    gate: Option<OcrGate>,
}

impl CountingOcr {
    fn build(results: Vec<OcrResult>, external: bool, gate: Option<OcrGate>) -> Arc<Self> {
        Arc::new(Self {
            results,
            external,
            calls: std::sync::atomic::AtomicUsize::new(0),
            gate,
        })
    }

    fn local(results: Vec<OcrResult>) -> Arc<Self> {
        Self::build(results, false, None)
    }

    fn external(results: Vec<OcrResult>) -> Arc<Self> {
        Self::build(results, true, None)
    }

    fn gated(results: Vec<OcrResult>) -> Arc<Self> {
        let gate = OcrGate {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        };
        Self::build(results, false, Some(gate))
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn gate(&self) -> &OcrGate {
        self.gate.as_ref().unwrap()
    }
}

#[async_trait]
impl OcrProvider for CountingOcr {
    async fn extract_elements(
        &self,
        _image: &[u8],
        _image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.entered.notify_one();
            let _permit = gate.release.acquire().await;
        }
        Ok(self.results.clone())
    }

    fn provider_name(&self) -> &str {
        "counting"
    }

    fn is_external(&self) -> bool {
        self.external
    }
}

async fn finder_holding(ocr: Arc<CountingOcr>, image: Vec<u8>) -> OcrElementFinder {
    let finder = OcrElementFinder::new(ocr);
    finder.set_image(image, "webp".to_string()).await;
    finder
}

#[tokio::test]
async fn an_unchanged_image_is_read_once_for_any_query() {
    let words = vec![word("file", 0), word("save", 50), word("edit", 200)];
    let ocr = CountingOcr::local(words.clone());
    let finder = finder_holding(ocr.clone(), vec![1, 2, 3]).await;

    let region = ElementBounds {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };
    let queries = [
        (Some("save"), None, None),
        (Some("edit"), Some("button"), None),
        (None, None, Some(&region)),
        (Some("e"), None, Some(&region)),
        (None, None, None),
    ];
    for (text, role, area) in queries {
        let reused = finder.find_element(text, role, area).await.unwrap();
        let fresh = finder_holding(CountingOcr::local(words.clone()), vec![1, 2, 3])
            .await
            .find_element(text, role, area)
            .await
            .unwrap();
        assert_eq!(format!("{reused:?}"), format!("{fresh:?}"));
    }
    assert_eq!(ocr.calls(), 1, "the same image was read more than once");
}

#[tokio::test]
async fn a_new_image_is_read_again_and_a_repeated_one_is_not() {
    let ocr = CountingOcr::local(vec![word("save", 0)]);
    let finder = finder_holding(ocr.clone(), vec![1]).await;
    finder.find_element(Some("save"), None, None).await.unwrap();
    finder.set_image(vec![1], "webp".to_string()).await;
    finder.find_element(Some("save"), None, None).await.unwrap();
    assert_eq!(ocr.calls(), 1);

    finder.set_image(vec![2], "webp".to_string()).await;
    finder.find_element(Some("save"), None, None).await.unwrap();
    assert_eq!(ocr.calls(), 2);

    // The same bytes in another format are another image.
    finder.set_image(vec![2], "png".to_string()).await;
    finder.find_element(Some("save"), None, None).await.unwrap();
    assert_eq!(ocr.calls(), 3);
}

#[tokio::test]
async fn scene_analysis_and_lookups_share_one_read() {
    let ocr = CountingOcr::local(vec![word("save", 0)]);
    let finder = OcrElementFinder::new(ocr.clone());
    finder.set_image(png(64, 32), "png".to_string()).await;

    let elements = finder.find_element(Some("save"), None, None).await.unwrap();
    let scene = finder.analyze_scene(Some("app"), None).await.unwrap();
    let again = finder.analyze_scene(Some("app"), None).await.unwrap();

    assert_eq!(ocr.calls(), 1);
    assert_eq!(elements.len(), 1);
    assert_eq!((scene.screen_width, scene.screen_height), (64, 32));
    assert_eq!(scene.elements.len(), 1);
    assert_eq!(scene.elements[0].label, "save");
    assert_eq!(again.elements.len(), 1);
}

#[tokio::test]
async fn a_cleared_image_answers_nothing_and_keeps_no_results() {
    let ocr = CountingOcr::local(vec![word("save", 0)]);
    let finder = finder_holding(ocr.clone(), vec![1]).await;
    finder.find_element(Some("save"), None, None).await.unwrap();

    finder.clear_image().await;
    let err = finder
        .find_element(Some("save"), None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CoreError::Internal { message, .. } if message.contains("image is missing")),
        "a cleared image must not answer a lookup, got: {err:?}"
    );
    let err = finder.analyze_scene(None, None).await.unwrap_err();
    assert!(
        matches!(&err, VisionError::Internal(message) if message.contains("image is missing")),
        "a cleared image must not answer a scene, got: {err:?}"
    );
    assert_eq!(ocr.calls(), 1);

    // The same frame held again is read afresh: nothing survived the clear.
    finder.set_image(vec![1], "webp".to_string()).await;
    finder.find_element(Some("save"), None, None).await.unwrap();
    assert_eq!(ocr.calls(), 2);
}

#[tokio::test]
async fn an_external_provider_is_called_for_every_lookup() {
    let ocr = CountingOcr::external(vec![word("save", 0)]);
    let finder = finder_holding(ocr.clone(), vec![1]).await;
    for _ in 0..3 {
        finder.find_element(Some("save"), None, None).await.unwrap();
    }
    assert_eq!(ocr.calls(), 3);
}

#[tokio::test]
async fn results_read_from_a_replaced_image_are_not_kept() {
    let ocr = CountingOcr::gated(vec![word("old", 0)]);
    let finder = Arc::new(finder_holding(ocr.clone(), vec![1]).await);

    let mut lookup = tokio::spawn({
        let finder = finder.clone();
        async move { finder.find_element(None, None, None).await }
    });
    // A lookup that never reaches OCR must fail the test, not leave it waiting.
    tokio::select! {
        () = ocr.gate().entered.notified() => {}
        early = &mut lookup => panic!("the lookup ended before it reached OCR: {early:?}"),
    }
    finder.set_image(vec![2], "webp".to_string()).await;
    ocr.gate().release.add_permits(1);
    let answered = lookup.await.unwrap().unwrap();
    assert_eq!(answered[0].text, "old");

    finder.find_element(None, None, None).await.unwrap();
    assert_eq!(
        ocr.calls(),
        2,
        "results read from the replaced image were kept for the new one"
    );
}

#[test]
fn only_a_different_image_moves_the_screen_number() {
    let mut held = HeldScreen::default();
    held.hold(Some((vec![1], "webp".to_string())));
    let first = held.screen_seq;
    held.results = Some(Zeroizing::new(HeldResults(vec![word("save", 0)])));

    held.hold(Some((vec![1], "webp".to_string())));
    assert_eq!(held.screen_seq, first);
    assert!(held.results.is_some());

    held.hold(Some((vec![2], "webp".to_string())));
    assert_eq!(held.screen_seq, first + 1);
    assert!(held.results.is_none());

    held.hold(None);
    held.hold(None);
    assert_eq!(held.screen_seq, first + 2);
}

#[test]
fn wiping_overwrites_the_recognised_text() {
    let mut held = HeldResults(vec![word("Jane Roe 4471", 0), word("save", 50)]);
    held.zeroize();
    assert!(held.0.iter().all(|result| result.text.is_empty()));
}

#[tokio::test]
async fn chained_finder_returns_first_success() {
    let empty_finder = MockFinder {
        name: "empty".to_string(),
        results: vec![],
    };
    let success_finder = MockFinder {
        name: "success".to_string(),
        results: vec![UiElement {
            text: "save".to_string(),
            bounds: ElementBounds {
                x: 0,
                y: 0,
                width: 40,
                height: 20,
            },
            role: None,
            confidence: 0.9,
            source: FinderSource::Ocr,
        }],
    };

    let chained = ChainedElementFinder::new(vec![Box::new(empty_finder), Box::new(success_finder)]);

    let result = chained
        .find_element(Some("save"), None, None)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].text, "save");
}

#[tokio::test]
async fn chained_finder_skips_failing() {
    let success_finder = MockFinder {
        name: "success".to_string(),
        results: vec![UiElement {
            text: "confirm".to_string(),
            bounds: ElementBounds {
                x: 10,
                y: 10,
                width: 50,
                height: 25,
            },
            role: Some("button".to_string()),
            confidence: 0.85,
            source: FinderSource::Accessibility,
        }],
    };

    let chained =
        ChainedElementFinder::new(vec![Box::new(FailingFinder), Box::new(success_finder)]);

    let result = chained
        .find_element(Some("confirm"), None, None)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
}

#[tokio::test]
async fn chained_finder_all_fail_returns_error() {
    let chained = ChainedElementFinder::new(vec![Box::new(FailingFinder)]);

    let err = chained
        .find_element(Some("none"), None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::ElementNotFound { .. }),
        "all-finders-fail must produce ElementNotFound, got: {err:?}"
    );
}

#[cfg(feature = "native-vision")]
#[tokio::test]
async fn pixel_geometry_is_independent_of_external_text_and_links_same_frame_words() {
    use crate::native_detect::PixelRegionDetector;
    use std::sync::atomic::Ordering;
    let image = image::RgbImage::from_fn(160, 90, |x, y| {
        if (20..140).contains(&x) && (18..74).contains(&y) {
            image::Rgb([25, 72, 92])
        } else {
            image::Rgb([255, 255, 255])
        }
    });
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let provider = CountingOcr::external(vec![OcrResult {
        text: "Save".to_owned(),
        x: 40,
        y: 35,
        width: 55,
        height: 18,
        confidence: 0.95,
    }]);
    let finder = OcrElementFinder::new(provider.clone())
        .with_rectangle_detector(Arc::new(PixelRegionDetector));
    let local = finder
        .analyze_geometry_from_image(bytes.clone(), None, None)
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(local.elements.len(), 1);
    assert!(local.elements[0].label.is_empty());
    assert_eq!(
        (
            local.elements[0].bbox_abs.x,
            local.elements[0].bbox_abs.width
        ),
        (20, 120)
    );
    let full = finder
        .analyze_scene_from_image_data(bytes, "png".to_owned(), None, None)
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let word = full.elements.iter().find(|e| e.label == "Save").unwrap();
    let button = full
        .elements
        .iter()
        .find(|e| e.role.as_deref() == Some("region"))
        .unwrap();
    assert_eq!(word.parent_id.as_deref(), Some(button.element_id.as_str()));
}

#[test]
fn ambiguous_or_unlocalized_text_gets_no_region_owner() {
    let bounds = |x, y, width, height| ElementBounds {
        x,
        y,
        width,
        height,
    };
    let region = |bounds| DetectedRectangle {
        bounds,
        bounds_normalized: NormalizedBounds::new(0.0, 0.0, 1.0, 1.0),
        confidence: 0.8,
        classification: None,
    };
    let regions = merge_rectangles(
        &[region(bounds(5, 10, 90, 40)), region(bounds(35, 0, 55, 65))],
        &[],
        150,
        100,
    );
    let mut words = OcrElementFinder::ocr_to_scene_elements(
        &[OcrResult {
            text: "Save".into(),
            x: 41,
            y: 22,
            width: 25,
            height: 20,
            confidence: 0.9,
        }],
        150,
        100,
        None,
        None,
        PiiFilterLevel::Standard,
    );
    attach_text_to_regions(&mut words, &regions);
    assert!(words[0].parent_id.is_none());
    words[0].bbox_abs = bounds(41, 22, 0, 0);
    attach_text_to_regions(&mut words, &regions);
    assert!(words[0].parent_id.is_none());
}

#[cfg(feature = "native-vision")]
#[tokio::test]
async fn denied_subscription_cannot_produce_a_successful_combined_scene() {
    struct Denied;
    #[async_trait]
    impl OcrProvider for Denied {
        async fn extract_elements(&self, _: &[u8], _: &str) -> Result<Vec<OcrResult>, CoreError> {
            Err(CoreError::PolicyDenied {
                code: maekon_core::error_codes::PolicyCode::Denied,
                message: "fixture".into(),
            })
        }
        fn provider_name(&self) -> &str {
            "denied-fixture"
        }
        fn is_external(&self) -> bool {
            true
        }
    }
    let finder = OcrElementFinder::new(Arc::new(Denied))
        .with_rectangle_detector(Arc::new(crate::native_detect::PixelRegionDetector));
    let image = png(50, 50);
    let local = finder
        .analyze_geometry_from_image(image.clone(), None, None)
        .await
        .expect("geometry must remain local when the external provider is denied");
    assert_eq!((local.screen_width, local.screen_height), (50, 50));
    assert!(local.elements.is_empty());
    assert!(matches!(
        finder
            .analyze_scene_from_image_data(image, "png".into(), None, None)
            .await,
        Err(VisionError::Core(CoreError::PolicyDenied { .. }))
    ));
}

#[tokio::test]
async fn a_late_subscription_answer_cannot_supply_a_live_scene_or_click_target() {
    for scene in [false, true] {
        let ocr = CountingOcr::build(
            vec![word("old", 0)],
            true,
            Some(OcrGate {
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Semaphore::new(0),
            }),
        );
        let finder = Arc::new(OcrElementFinder::new(ocr.clone()));
        finder.set_image(png(80, 42), "png".into()).await;
        let mut reply = tokio::spawn({
            let finder = finder.clone();
            async move {
                if scene {
                    finder
                        .analyze_scene(None, None)
                        .await
                        .map(|_| ())
                        .map_err(CoreError::from)
                } else {
                    finder.find_element(None, None, None).await.map(|_| ())
                }
            }
        });
        tokio::select! {
            () = ocr.gate().entered.notified() => {}
            early = &mut reply => panic!("the lookup ended before it reached the provider: {early:?}"),
        }
        finder.set_image(png(82, 43), "png".into()).await;
        ocr.gate().release.add_permits(1);
        assert!(matches!(
            reply.await.unwrap(),
            Err(CoreError::ServiceUnavailable { .. })
        ));
    }
}

fn bounds(x: i32, y: i32, width: u32, height: u32) -> ElementBounds {
    ElementBounds {
        x,
        y,
        width,
        height,
    }
}

fn detected(bounds: ElementBounds) -> DetectedRectangle {
    DetectedRectangle {
        bounds,
        bounds_normalized: NormalizedBounds::new(0.0, 0.0, 1.0, 1.0),
        confidence: 0.8,
        classification: None,
    }
}

#[test]
fn merge_thresholds_distinguish_contained_text_from_partial_overlap() {
    // Two width-60 boxes offset by 40 overlap over 20 of their 100 combined pixels.
    for (rectangle, text, retain) in [
        (bounds(10, 20, 60, 20), bounds(51, 20, 60, 20), true),
        (bounds(10, 20, 60, 20), bounds(50, 20, 60, 20), false),
        (bounds(10, 20, 60, 20), bounds(49, 20, 60, 20), false),
        (bounds(10, 20, 100, 20), bounds(10, 20, 79, 20), true),
        (bounds(10, 20, 100, 20), bounds(10, 20, 80, 20), false),
    ] {
        let words = merge_rectangles(&[detected(text)], &[], 200, 100);
        let output = merge_rectangles(&[detected(rectangle)], &words, 200, 100);
        assert_eq!(output.len(), usize::from(retain));
    }
}

#[test]
fn containment_requires_nonempty_text_and_observes_each_far_edge() {
    let outer = bounds(13, 17, 21, 23);
    assert!(contains_box(&outer, &outer));
    assert!(contains_box(&outer, &bounds(31, 38, 3, 2)));
    for outside in [
        bounds(31, 38, 4, 2),
        bounds(31, 38, 3, 3),
        bounds(12, 17, 4, 2),
        bounds(13, 16, 3, 3),
        bounds(14, 18, 0, 2),
        bounds(14, 18, 3, 0),
    ] {
        assert!(!contains_box(&outer, &outside));
    }
}

#[test]
fn text_belongs_to_innermost_enclosing_region_in_either_order() {
    let outer = bounds(10, 10, 40, 35);
    for inner in [
        bounds(11, 14, 35, 10),
        bounds(10, 14, 40, 10),
        bounds(11, 10, 35, 35),
    ] {
        for (order, inner_index) in [
            ([outer.clone(), inner.clone()], 1),
            ([inner.clone(), outer.clone()], 0),
        ] {
            let regions = merge_rectangles(&order.map(detected), &[], 100, 100);
            let mut words = merge_rectangles(&[detected(bounds(15, 16, 12, 6))], &[], 100, 100);
            attach_text_to_regions(&mut words, &regions);
            assert_eq!(
                words[0].parent_id.as_ref(),
                Some(&regions[inner_index].element_id)
            );
        }
    }
}
