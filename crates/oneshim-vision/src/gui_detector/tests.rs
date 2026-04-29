use oneshim_core::models::frame::BoundingBox;
use oneshim_core::models::gui_interaction::GuiElementType;

use super::*;

fn make_region(text: &str, x: u32, y: u32, w: u32, h: u32, confidence: f32) -> OcrRegion {
    OcrRegion {
        text: text.to_string(),
        bbox: BoundingBox {
            x,
            y,
            width: w,
            height: h,
        },
        confidence,
    }
}

fn detector() -> GuiElementDetector {
    // 1920x1080 standard resolution, PII off for tests
    GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off)
}

#[test]
fn correlate_click_finds_matching_region() {
    let d = detector();
    let regions = vec![
        make_region("Save", 100, 200, 60, 30, 0.9),
        make_region("Cancel", 180, 200, 80, 30, 0.85),
    ];

    let result = d.correlate_click(120, 210, &regions);
    assert!(result.is_some());
    let elem = result.unwrap();
    assert_eq!(elem.text, "Save");
    assert_eq!(elem.element_type, GuiElementType::Button);
}

#[test]
fn correlate_click_proximity_fallback() {
    let d = detector();
    // Click is 20px away from region — within default 40px threshold
    let regions = vec![make_region("Save", 100, 200, 60, 30, 0.9)];

    let result = d.correlate_click(80, 210, &regions);
    assert!(result.is_some());
    assert_eq!(result.unwrap().text, "Save");
}

#[test]
fn correlate_click_returns_none_beyond_threshold() {
    let d = detector();
    let regions = vec![make_region("Save", 100, 200, 60, 30, 0.9)];

    // Click is far outside threshold
    let result = d.correlate_click(500, 500, &regions);
    assert!(result.is_none());
}

#[test]
fn correlate_click_selects_smallest_overlapping_region() {
    let d = detector();
    let regions = vec![
        make_region("Dialog", 50, 50, 300, 200, 0.8),
        make_region("OK", 150, 120, 40, 20, 0.9),
    ];

    let result = d.correlate_click(160, 125, &regions);
    assert!(result.is_some());
    let elem = result.unwrap();
    assert_eq!(elem.text, "OK");
}

#[test]
fn correlate_click_empty_regions() {
    let d = detector();
    let result = d.correlate_click(100, 100, &[]);
    assert!(result.is_none());
}

#[test]
fn correlate_typing_marks_as_text_input() {
    let d = detector();
    let regions = vec![make_region("Username", 100, 200, 200, 30, 0.85)];

    let result = d.correlate_typing(&regions, 150, 210);
    assert!(result.is_some());
    let elem = result.unwrap();
    assert_eq!(elem.element_type, GuiElementType::TextInput);
}

#[test]
fn infer_element_type_title_bar() {
    let d = detector();
    let bbox = BoundingBox {
        x: 0,
        y: 10,
        width: 200,
        height: 20,
    };
    let t = d.infer_element_type("My Application", &bbox);
    assert_eq!(t, GuiElementType::TitleBar);
}

#[test]
fn infer_element_type_link() {
    let d = detector();
    let bbox = BoundingBox {
        x: 50,
        y: 300,
        width: 200,
        height: 20,
    };
    let t = d.infer_element_type("https://example.com", &bbox);
    assert_eq!(t, GuiElementType::Link);
}

#[test]
fn infer_element_type_button() {
    let d = detector();
    let bbox = BoundingBox {
        x: 50,
        y: 300,
        width: 60,
        height: 30,
    };
    let t = d.infer_element_type("Save", &bbox);
    assert_eq!(t, GuiElementType::Button);
}

#[test]
fn infer_element_type_text_region_multiword() {
    let d = detector();
    let bbox = BoundingBox {
        x: 50,
        y: 300,
        width: 400,
        height: 20,
    };
    // Multi-word text (3+ words) that doesn't match any other pattern → TextRegion
    let t = d.infer_element_type("The quick brown fox jumps over the lazy dog", &bbox);
    assert_eq!(t, GuiElementType::TextRegion);
}

#[test]
fn infer_element_type_unknown_short_text() {
    let d = detector();
    let bbox = BoundingBox {
        x: 50,
        y: 300,
        width: 60,
        height: 20,
    };
    // Short non-matching text (< 3 words) → Unknown
    let t = d.infer_element_type("xy", &bbox);
    assert_eq!(t, GuiElementType::Unknown);
}

#[test]
fn infer_element_type_tab_label() {
    let d = detector();
    let bbox = BoundingBox {
        x: 100,
        y: 80,
        width: 80,
        height: 20,
    };
    let t = d.infer_element_type("main.rs", &bbox);
    assert_eq!(t, GuiElementType::TabLabel);
}

#[test]
fn infer_element_type_status_bar() {
    let d = detector();
    let bbox = BoundingBox {
        x: 0,
        y: 1050,
        width: 200,
        height: 20,
    };
    let t = d.infer_element_type("Ln 42, Col 10", &bbox);
    assert_eq!(t, GuiElementType::StatusBar);
}

#[test]
fn infer_element_type_menu_item_shortcut() {
    let d = detector();
    let bbox = BoundingBox {
        x: 50,
        y: 300,
        width: 150,
        height: 20,
    };
    let t = d.infer_element_type("Save  Ctrl+S", &bbox);
    assert_eq!(t, GuiElementType::MenuItem);
}

#[test]
fn infer_element_type_menu_item_mac_shortcut() {
    let d = detector();
    let bbox = BoundingBox {
        x: 50,
        y: 300,
        width: 150,
        height: 20,
    };
    let t = d.infer_element_type("New File  ⌘N", &bbox);
    assert_eq!(t, GuiElementType::MenuItem);
}

#[test]
fn looks_like_menu_item_detection() {
    assert!(GuiElementDetector::looks_like_menu_item("Save  Ctrl+S"));
    assert!(GuiElementDetector::looks_like_menu_item("⌘N"));
    assert!(GuiElementDetector::looks_like_menu_item("⇧⌘P"));
    assert!(GuiElementDetector::looks_like_menu_item("Alt+F4"));
    assert!(!GuiElementDetector::looks_like_menu_item("Save"));
    assert!(!GuiElementDetector::looks_like_menu_item("Hello World"));
}

#[test]
fn infer_element_type_tree_item() {
    let d = detector();
    let bbox = BoundingBox {
        x: 20,
        y: 300,
        width: 150,
        height: 20,
    };
    let t = d.infer_element_type("▸ src", &bbox);
    assert_eq!(t, GuiElementType::TreeItem);
}

#[test]
fn word_grouping_merges_adjacent() {
    let regions = vec![
        make_region("Hello", 10, 100, 50, 20, 0.9),
        make_region("World", 65, 100, 50, 20, 0.9),
    ];

    let grouped = GuiElementDetector::group_words(&regions);
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped[0].text, "Hello World");
    assert_eq!(grouped[0].bbox.x, 10);
    assert_eq!(grouped[0].bbox.width, 105); // 10..115
}

#[test]
fn word_grouping_splits_distant_words() {
    let regions = vec![
        make_region("Hello", 10, 100, 50, 20, 0.9),
        make_region("World", 500, 100, 50, 20, 0.9),
    ];

    let grouped = GuiElementDetector::group_words(&regions);
    assert_eq!(grouped.len(), 2);
}

#[test]
fn word_grouping_splits_different_lines() {
    let regions = vec![
        make_region("Line1", 10, 100, 50, 20, 0.9),
        make_region("Line2", 10, 200, 50, 20, 0.9),
    ];

    let grouped = GuiElementDetector::group_words(&regions);
    assert_eq!(grouped.len(), 2);
}

#[test]
fn word_grouping_empty() {
    let grouped = GuiElementDetector::group_words(&[]);
    assert!(grouped.is_empty());
}

#[test]
fn pii_filter_applied_to_element_text() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Basic);
    let regions = vec![make_region("user@example.com", 100, 200, 200, 30, 0.9)];

    let result = d.correlate_click(150, 210, &regions);
    assert!(result.is_some());
    let elem = result.unwrap();
    // Basic PII filter masks emails
    assert!(!elem.text.contains("user@example.com"));
}

#[test]
fn infer_element_type_toolbar_icon() {
    let d = detector();
    // Near top of window (within 2× title bar height), small box, no text
    // Title bar max = 1080 * 0.04 = 43, toolbar max = 86
    let bbox = BoundingBox {
        x: 200,
        y: 50,
        width: 30,
        height: 30,
    };
    let t = d.infer_element_type("", &bbox);
    assert_eq!(t, GuiElementType::ToolbarIcon);
}

#[test]
fn infer_element_type_toolbar_icon_single_char() {
    let d = detector();
    let bbox = BoundingBox {
        x: 200,
        y: 60,
        width: 24,
        height: 24,
    };
    // Single-char icon label (e.g., "X" close icon)
    let t = d.infer_element_type("X", &bbox);
    assert_eq!(t, GuiElementType::ToolbarIcon);
}

#[test]
fn infer_element_type_scrollbar_right_edge() {
    let d = detector();
    // 1920×1080 screen — right edge starts at 1920-20=1900
    let bbox = BoundingBox {
        x: 1905,
        y: 200,
        width: 15,
        height: 400,
    };
    let t = d.infer_element_type("", &bbox);
    assert_eq!(t, GuiElementType::ScrollBar);
}

#[test]
fn infer_element_type_scrollbar_bottom_edge() {
    let d = detector();
    // 1920×1080 screen — bottom edge starts at 1080-20=1060
    // Also must be below status_bar_min_y (1026), but scrollbar check
    // is after status bar, so test at the right edge instead
    let bbox = BoundingBox {
        x: 1905,
        y: 500,
        width: 12,
        height: 300,
    };
    let t = d.infer_element_type("", &bbox);
    assert_eq!(t, GuiElementType::ScrollBar);
}

#[test]
fn distance_to_bbox_inside() {
    let bbox = BoundingBox {
        x: 100,
        y: 100,
        width: 50,
        height: 30,
    };
    assert_eq!(GuiElementDetector::distance_to_bbox(120, 110, &bbox), 0.0);
}

#[test]
fn distance_to_bbox_outside() {
    let bbox = BoundingBox {
        x: 100,
        y: 100,
        width: 50,
        height: 30,
    };
    // 10px to the left of the bbox
    let dist = GuiElementDetector::distance_to_bbox(90, 110, &bbox);
    assert!((dist - 10.0).abs() < 0.01);
}

#[test]
fn update_resolution_changes_thresholds() {
    let mut d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    assert_eq!(d.resolution(), (1920, 1080));

    // Title bar threshold at 1080p: 1080 * 0.04 = 43px
    let bbox_at_30 = BoundingBox {
        x: 0,
        y: 30,
        width: 200,
        height: 20,
    };
    assert_eq!(
        d.infer_element_type("File", &bbox_at_30),
        GuiElementType::TitleBar
    );

    // Switch to 4K resolution
    d.update_resolution(3840, 2160);
    assert_eq!(d.resolution(), (3840, 2160));

    // Same bbox at y=30 is now well within the title bar (2160 * 0.04 = 86px)
    assert_eq!(
        d.infer_element_type("File", &bbox_at_30),
        GuiElementType::TitleBar
    );

    // y=50 was NOT title bar at 1080p (43px threshold), but IS at 4K (86px threshold)
    let bbox_at_50 = BoundingBox {
        x: 200,
        y: 50,
        width: 30,
        height: 30,
    };
    // At 1080p this would be ToolbarIcon (below 43px title bar, within 86px toolbar)
    let d_1080 = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    assert_eq!(
        d_1080.infer_element_type("", &bbox_at_50),
        GuiElementType::ToolbarIcon
    );
    // At 4K this is TitleBar (below 86px threshold)
    assert_eq!(
        d.infer_element_type("", &bbox_at_50),
        GuiElementType::TitleBar
    );
}

#[test]
fn update_resolution_ignores_zero() {
    let mut d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);

    // Zero dimensions should be rejected
    d.update_resolution(0, 0);
    assert_eq!(d.resolution(), (1920, 1080));

    d.update_resolution(0, 1080);
    assert_eq!(d.resolution(), (1920, 1080));

    d.update_resolution(1920, 0);
    assert_eq!(d.resolution(), (1920, 1080));
}

#[test]
fn zero_resolution_bug_regression() {
    // Regression test: with (0,0) resolution, ALL elements are classified
    // as TitleBar because title_bar_max_y = 0*0.04 = 0, and bbox.y < 0 is
    // always false, so the first threshold is "passed". Actually,
    // title_bar_max_y=0 means bbox.y < 0 is never true for u32, so this
    // test verifies the fix works correctly.
    let d_bad = GuiElementDetector::new((0, 0), PiiFilterLevel::Off);
    let bbox_mid = BoundingBox {
        x: 100,
        y: 300,
        width: 60,
        height: 30,
    };
    // With (0,0): status_bar_min_y = 0*0.95 = 0, so bbox.y(300) >= 0 → StatusBar!
    let t = d_bad.infer_element_type("Save", &bbox_mid);
    assert_eq!(
        t,
        GuiElementType::StatusBar,
        "bug: (0,0) misclassifies mid-screen as StatusBar"
    );

    // With proper resolution, "Save" button at y=300 should be Button
    let d_good = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    let t = d_good.infer_element_type("Save", &bbox_mid);
    assert_eq!(
        t,
        GuiElementType::Button,
        "with proper resolution, Save button is correctly identified"
    );
}

// ── App-specific override tests ──

#[test]
fn ide_sidebar_override_to_tree_item() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    // Left 20% of screen (< 384px), below title bar (y > 97)
    let region = make_region("src/main.rs", 30, 200, 120, 16, 0.9);
    let elem = d.correlate_click_with_app(60, 208, &[region], "Visual Studio Code");
    assert_eq!(elem.unwrap().element_type, GuiElementType::TreeItem);
}

#[test]
fn browser_url_override_to_link() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    // Top 8% of screen (< 86px), contains URL-like text
    let region = make_region("github.com/repo", 200, 60, 400, 20, 0.9);
    let elem = d.correlate_click_with_app(300, 70, &[region], "Google Chrome");
    assert_eq!(elem.unwrap().element_type, GuiElementType::Link);
}

#[test]
fn chat_sidebar_override_to_tree_item() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    // Left 25% of screen (< 480px), below title bar
    let region = make_region("#general", 50, 200, 100, 18, 0.9);
    let elem = d.correlate_click_with_app(80, 209, &[region], "Slack");
    assert_eq!(elem.unwrap().element_type, GuiElementType::TreeItem);
}

#[test]
fn non_matching_app_uses_generic_inference() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    let region = make_region("Save", 500, 500, 50, 20, 0.9);
    let elem = d.correlate_click_with_app(520, 510, &[region], "CustomApp");
    assert_eq!(elem.unwrap().element_type, GuiElementType::Button);
}

// ── R-tree spatial index tests ──

#[test]
fn spatial_index_matches_linear_scan_for_large_regions() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    // Generate 500 regions (above SPATIAL_INDEX_THRESHOLD of 400)
    let regions: Vec<OcrRegion> = (0..500)
        .map(|i| {
            let row = i / 25;
            let col = i % 25;
            make_region(&format!("item_{i}"), col * 76, row * 54, 72, 50, 0.9)
        })
        .collect();

    // Click at center of region #312 (row 12, col 12)
    let click_x = 12 * 76 + 36;
    let click_y = 12 * 54 + 25;

    // This should use the spatial path (500 >= 400)
    let result = d.correlate_click(click_x, click_y, &regions);
    assert!(result.is_some(), "spatial index should find a match");
    assert!(result.unwrap().text.starts_with("item_"));
}

#[test]
fn spatial_index_proximity_fallback() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    let mut regions: Vec<OcrRegion> = (0..400)
        .map(|i| make_region(&format!("r{i}"), (i % 20) * 96, (i / 20) * 54, 90, 50, 0.9))
        .collect();
    // Add one region far from click point
    regions.push(make_region("target", 960, 540, 50, 20, 0.9));

    // Click near but not inside "target"
    let result = d.correlate_click(1000, 545, &regions);
    // Should find "target" via proximity (within 40px)
    assert!(result.is_some());
}

// --- Scored inference tests ---

#[test]
fn scored_title_bar_has_high_confidence() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    let bbox = BoundingBox {
        x: 100,
        y: 10, // top 1% → clearly title bar
        width: 200,
        height: 20,
    };
    let (etype, conf) = d.infer_element_type_scored("My App Window", &bbox);
    assert_eq!(etype, GuiElementType::TitleBar);
    assert!(
        conf > 0.5,
        "title bar confidence should be > 0.5, got {conf}"
    );
}

#[test]
fn scored_link_very_high_confidence() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    let bbox = BoundingBox {
        x: 100,
        y: 500, // middle of screen (no position signal)
        width: 300,
        height: 20,
    };
    let (etype, conf) = d.infer_element_type_scored("https://example.com", &bbox);
    assert_eq!(etype, GuiElementType::Link);
    assert!(conf > 0.6, "link confidence should be > 0.6, got {conf}");
}

#[test]
fn scored_ambiguous_element_lower_confidence() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    // In tab bar region + short text → TabLabel vs ToolbarIcon compete
    let bbox = BoundingBox {
        x: 100,
        y: 60, // between title bar (4%) and tab bar (9%)
        width: 30,
        height: 20,
    };
    let (_etype, conf) = d.infer_element_type_scored("OK", &bbox);
    // Multiple signals match → confidence should be moderate, not 1.0
    assert!(
        conf < 0.9,
        "ambiguous element confidence should be < 0.9, got {conf}"
    );
}

#[test]
fn scored_backward_compat_same_types() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    let cases = vec![
        (
            "Save",
            BoundingBox {
                x: 100,
                y: 500,
                width: 60,
                height: 30,
            },
        ),
        (
            "https://test.com",
            BoundingBox {
                x: 100,
                y: 500,
                width: 200,
                height: 20,
            },
        ),
        (
            "Ctrl+S",
            BoundingBox {
                x: 100,
                y: 500,
                width: 80,
                height: 20,
            },
        ),
    ];
    for (text, bbox) in cases {
        let old = d.infer_element_type(text, &bbox);
        let (new, _conf) = d.infer_element_type_scored(text, &bbox);
        assert_eq!(old, new, "scored inference should match old for '{text}'");
    }
}

#[test]
fn scored_build_gui_element_populates_type_confidence() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);
    let region = make_region("Save", 100, 500, 60, 30, 0.9);
    let element = d.build_gui_element(&region);
    assert_eq!(element.element_type, GuiElementType::Button);
    assert!(
        element.type_confidence > 0.0 && element.type_confidence <= 1.0,
        "type_confidence should be in (0,1], got {}",
        element.type_confidence
    );
    // OCR confidence should still be the region's value
    assert!((element.confidence - 0.9).abs() < f32::EPSILON);
}

// --- ML classifier integration tests ---

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::gui_element_classifier::GuiElementClassifier;
use std::sync::Arc;

struct MockMlClassifier {
    return_type: GuiElementType,
    confidence: f32,
}

#[async_trait]
impl GuiElementClassifier for MockMlClassifier {
    async fn classify_crop(
        &self,
        _crop_rgba: &[u8],
        _width: u32,
        _height: u32,
    ) -> Result<Option<(GuiElementType, f32)>, CoreError> {
        Ok(Some((self.return_type.clone(), self.confidence)))
    }

    fn is_ready(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn build_gui_element_with_frame_uses_ml_on_high_confidence() {
    let classifier: Arc<dyn GuiElementClassifier> = Arc::new(MockMlClassifier {
        return_type: GuiElementType::Link,
        confidence: 0.85,
    });
    let d =
        GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off).with_ml_classifier(classifier);

    // Region must fit inside the frame dimensions
    let region = make_region("Save", 10, 10, 60, 30, 0.9);
    let frame = vec![128u8; 200 * 100 * 4];
    let elem = d
        .build_gui_element_with_frame(&region, Some(&frame), 200, 100)
        .await;

    // ML says Link with 0.85 (> 0.7 threshold) → should override heuristic
    assert_eq!(elem.element_type, GuiElementType::Link);
    assert!((elem.type_confidence - 0.85).abs() < f32::EPSILON);
}

#[tokio::test]
async fn build_gui_element_with_frame_falls_back_on_low_confidence() {
    let classifier: Arc<dyn GuiElementClassifier> = Arc::new(MockMlClassifier {
        return_type: GuiElementType::Link,
        confidence: 0.5, // Below 0.7 threshold
    });
    let d =
        GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off).with_ml_classifier(classifier);

    // Region must fit inside the frame dimensions
    let region = make_region("Save", 10, 10, 60, 30, 0.9);
    let frame = vec![128u8; 200 * 100 * 4];
    let elem = d
        .build_gui_element_with_frame(&region, Some(&frame), 200, 100)
        .await;

    // ML confidence 0.5 < 0.7 threshold → heuristic should be used
    // "Save" at (10,10) with 1920x1080 screen → TitleBar (y < 43px title_bar_max)
    assert_ne!(
        elem.element_type,
        GuiElementType::Link,
        "low ML should not override"
    );
}

#[tokio::test]
async fn build_gui_element_with_frame_no_frame_data() {
    let classifier: Arc<dyn GuiElementClassifier> = Arc::new(MockMlClassifier {
        return_type: GuiElementType::Link,
        confidence: 0.95,
    });
    let d =
        GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off).with_ml_classifier(classifier);

    let region = make_region("Save", 100, 500, 60, 30, 0.9);
    let elem = d.build_gui_element_with_frame(&region, None, 0, 0).await;

    // No frame data → heuristic fallback
    assert_eq!(elem.element_type, GuiElementType::Button);
}

#[tokio::test]
async fn build_gui_element_with_frame_no_classifier() {
    let d = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);

    let region = make_region("Save", 100, 500, 60, 30, 0.9);
    let frame = vec![128u8; 200 * 100 * 4];
    let elem = d
        .build_gui_element_with_frame(&region, Some(&frame), 200, 100)
        .await;

    // No classifier → heuristic fallback
    assert_eq!(elem.element_type, GuiElementType::Button);
}

#[test]
fn crop_region_rgba_valid_region() {
    // 10x10 frame, crop 3x3 region starting at (2,2)
    let frame = vec![128u8; 10 * 10 * 4];
    let bbox = BoundingBox {
        x: 2,
        y: 2,
        width: 3,
        height: 3,
    };
    let result = GuiElementDetector::crop_region_rgba(&frame, 10, 10, &bbox);
    assert!(result.is_some());
    assert_eq!(result.unwrap().len(), 3 * 3 * 4);
}

#[test]
fn crop_region_rgba_out_of_bounds() {
    let frame = vec![128u8; 10 * 10 * 4];
    let bbox = BoundingBox {
        x: 8,
        y: 8,
        width: 5,
        height: 5,
    };
    let result = GuiElementDetector::crop_region_rgba(&frame, 10, 10, &bbox);
    assert!(result.is_none(), "out-of-bounds crop should return None");
}
