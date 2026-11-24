use super::*;
use image::{ImageFormat, Rgba};
fn png(image: RgbaImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

fn geometry(left: usize, top: usize, right: usize, bottom: usize) -> Box2d {
    Box2d {
        left,
        top,
        right,
        bottom,
    }
}

#[test]
fn overlap_is_intersection_over_union_in_both_orders() {
    let a = geometry(2, 3, 7, 7);
    let b = geometry(5, 1, 9, 5);
    assert_eq!(a.overlap(b), 0.125);
    assert_eq!(b.overlap(a), 0.125);
    assert_eq!(a.overlap(a), 1.0);
    assert_eq!(a.overlap(geometry(7, 3, 8, 7)), 0.0);
}

#[test]
fn frame_area_limit_is_checked_before_decoding_the_capture() {
    let one_pixel = png(RgbaImage::new(1, 1));
    assert_eq!(PixelRegionDetector.provider_name(), "pixel-contours");
    for min_size in [-0.1, 1.1, f32::NEG_INFINITY, f32::INFINITY, f32::NAN] {
        assert!(matches!(
            PixelRegionDetector.detect_rectangles(&one_pixel, 1, 1, min_size, 0),
            Err(CoreError::Internal { code: InternalCode::Generic, message })
                if message == "Invalid dimensions or minimum region size"
        ));
    }
    for min_size in [0.0, 1.0] {
        assert!(PixelRegionDetector
            .detect_rectangles(&one_pixel, 1, 1, min_size, 0)
            .unwrap()
            .is_empty());
    }
    let error = |w, h| match PixelRegionDetector
        .detect_rectangles(&one_pixel, w, h, 0.0, 0)
        .unwrap_err()
    {
        CoreError::Internal {
            code: InternalCode::Generic,
            message,
        } => message,
        error => panic!("unexpected validation error: {error:?}"),
    };
    assert_eq!(
        error(8192, 4096),
        "Image dimensions differ from capture metadata"
    );
    assert_eq!(
        error(8192, 4097),
        "Invalid dimensions or minimum region size"
    );
    assert_eq!(error(0, 1), "Invalid dimensions or minimum region size");
    assert_eq!(error(1, 0), "Invalid dimensions or minimum region size");
}

#[test]
fn the_flood_fill_respects_labels_and_visits_each_four_neighbor_once() {
    let plane = Plane {
        width: 7,
        height: 5,
        step: 1,
        colors: vec![[0; 3]; 35],
    };
    let mut labels = vec![0; 35];
    labels[15] = 90;
    let mut hits = vec![999];
    // Four arms around index 16. Index 15 already belongs to another
    // component and index 25 touches this one only at a corner.
    let b = plane.component(23, &mut labels, &mut hits, |i| {
        [8, 9, 15, 16, 17, 23, 25].contains(&i)
    });
    hits.sort_unstable();
    assert_eq!(hits, [8, 9, 16, 17, 23]);
    assert_eq!((b.left, b.top, b.right, b.bottom), (1, 1, 4, 4));
    let id = labels[23];
    assert_ne!(id, 0);
    assert_ne!(id, 90);
    for (i, label) in labels.into_iter().enumerate() {
        let expected = if hits.contains(&i) {
            id
        } else if i == 15 {
            90
        } else {
            0
        };
        assert_eq!(label, expected);
    }
}

#[test]
fn raster_outline_counts_each_side_and_the_boundaries_around_holes() {
    let fixtures: &[(&[&str], Box2d, [usize; 4], usize)] = &[
        (&["#"], geometry(0, 0, 1, 1), [1, 1, 1, 1], 4),
        (
            &["#.##", "####", "##.#"],
            geometry(0, 0, 4, 3),
            [3, 3, 3, 3],
            18,
        ),
        (
            &["......", ".###..", ".###..", ".#.#..", ".##...", "......"],
            geometry(1, 1, 4, 5),
            [3, 2, 4, 3],
            18,
        ),
        (
            &["#####", "#...#", "#####"],
            geometry(0, 0, 5, 3),
            [5, 5, 3, 3],
            24,
        ),
    ];
    for &(rows, bounds, sides, perimeter) in fixtures {
        let plane = Plane {
            width: rows[0].len(),
            height: rows.len(),
            step: 1,
            colors: Vec::new(),
        };
        let labels: Vec<_> = rows
            .iter()
            .flat_map(|row| row.bytes().map(|b| u32::from(b == b'#')))
            .collect();
        let pixels: Vec<_> = (0..labels.len()).filter(|&i| labels[i] == 1).collect();
        assert_eq!(plane.outline(&pixels, &labels, bounds), (sides, perimeter));
    }
}

fn painted_proposal(
    size: (usize, usize),
    frame: (usize, usize),
    step: u32,
    minimum: usize,
    ink: impl Fn(usize, usize) -> bool,
) -> Option<Proposal> {
    let (w, h) = size;
    let (width, height) = frame;
    let plane = Plane {
        width,
        height,
        step,
        colors: Vec::new(),
    };
    let mut labels = vec![0; width * height];
    let mut pixels = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if ink(x, y) {
                labels[y * width + x] = 1;
                pixels.push(y * width + x);
            }
        }
    }
    plane.proposal(&pixels, &labels, geometry(0, 0, w, h), minimum)
}

#[test]
fn rectangular_proposals_use_measured_edge_coverage_and_the_shorter_side() {
    let aligned = |x: usize, y: usize| (1..4).contains(&y) || (3..7).contains(&x);
    for result in [
        painted_proposal((10, 5), (10, 6), 1, 5, aligned),
        painted_proposal((5, 10), (6, 10), 1, 5, |x, y| aligned(y, x)),
    ] {
        let p = result.unwrap();
        assert_eq!(p.kind, "rectangular-surface");
        // The limiting edge contains four of ten pixels.
        assert!((p.score - 0.848).abs() < 1e-12);
    }
    for (size, frame, minimum) in [
        ((10, 5), (11, 6), 6),
        ((10, 5), (10, 5), 1),
        ((191, 5), (192, 6), 1),
    ] {
        assert!(painted_proposal(size, frame, 1, minimum, |_, _| true).is_none());
    }
    let long = painted_proposal((190, 5), (191, 6), 1, 5, |_, _| true).unwrap();
    assert_eq!(long.kind, "rectangular-surface");
    assert!((long.score - 0.98).abs() < 1e-12);
}

#[test]
fn compact_shapes_use_capture_scale_and_orientation_independent_confidence() {
    let diamond = |w: usize, h: usize, step| {
        painted_proposal((w, h), (w + 1, h + 1), step, 1, |x, y| {
            (2 * x).abs_diff(w - 1) * (h - 1) + (2 * y).abs_diff(h - 1) * (w - 1)
                <= (w - 1) * (h - 1)
        })
    };
    for (w, h, step, aspect) in [
        (25, 25, 1, 1.0),
        (31, 21, 1, 21.0 / 31.0),
        (21, 31, 1, 21.0 / 31.0),
        (11, 11, 2, 1.0),
        (63, 63, 2, 1.0),
    ] {
        let p = diamond(w, h, step).unwrap();
        assert_eq!(p.kind, "compact-shape");
        assert!((p.score - (0.50 + 0.18 * aspect)).abs() < 1e-12);
    }
    for (w, h, step) in [(19, 19, 1), (65, 65, 2), (41, 21, 1)] {
        assert!(diamond(w, h, step).is_none());
    }
    let thin_cross = painted_proposal((41, 41), (42, 42), 1, 1, |x, y| x == 20 || y == 20);
    assert!(thin_cross.is_none());
}

#[test]
fn rail_and_handle_spans_are_measured_in_both_orientations() {
    // The rail accepts a centered stroke up to four pixels wide. Three
    // handle columns span at least eight pixels; blank columns split ink runs.
    let strokes = [
        (1, 4, 8),
        (2, 7, 9),
        (3, 8, 10),
        (4, 0, 4),
        (5, 1, 5),
        (7, 3, 8),
        (8, 2, 10),
        (9, 2, 9),
        (10, 0, 12),
        (12, 4, 12),
    ];
    for horizontal in [true, false] {
        let (w, h) = if horizontal { (18, 12) } else { (12, 18) };
        let plane = Plane {
            width: w + 7,
            height: h + 4,
            step: 1,
            colors: Vec::new(),
        };
        let b = geometry(4, 2, 4 + w, 2 + h);
        let mut labels = vec![20; plane.width * plane.height];
        for (along, lo, hi) in strokes {
            for across in lo..hi {
                let (x, y) = if horizontal {
                    (along, across)
                } else {
                    (across, along)
                };
                labels[(b.top + y) * plane.width + b.left + x] = 3;
            }
        }
        let profile = plane.ink_profile(&labels, 20, b);
        assert_eq!(
            profile,
            InkProfile {
                rail_columns: 3,
                tall_start: Some(8),
                tall_end: 13,
                tall_count: 3,
                ink_runs: 3,
            }
        );
    }
}

#[test]
fn tracks_need_enough_rail_and_a_dense_handle_within_the_width_limits() {
    let accepts = |rail_columns, thumb: usize, tall_count| {
        InkProfile {
            rail_columns,
            tall_start: Some(5),
            tall_end: 5 + thumb,
            tall_count,
            ink_runs: 1,
        }
        .describes_track(12, 36)
    };
    assert!(accepts(24, 4, 3));
    assert!(accepts(24, 12, 9));
    assert!(!accepts(23, 4, 3));
    assert!(!accepts(24, 3, 3));
    assert!(!accepts(24, 12, 8));
    assert!(!InkProfile::default().describes_track(12, 36));
    // A longer rail leaves room for a handle up to twice the short side.
    let long = |span: usize| {
        InkProfile {
            rail_columns: 60,
            tall_start: Some(3),
            tall_end: 3 + span,
            tall_count: span,
            ink_runs: 1,
        }
        .describes_track(12, 90)
    };
    assert!(long(24));
    assert!(!long(25));
}

#[test]
fn labelled_controls_accept_only_square_indicators_aligned_at_either_end() {
    let b = geometry(20, 10, 80, 22);
    for icon in [geometry(17, 13, 29, 25), geometry(68, 7, 83, 19)] {
        assert!(b.has_end_indicator(icon, 12));
    }
    for icon in [
        geometry(16, 10, 28, 22),
        geometry(40, 10, 52, 22),
        geometry(20, 14, 32, 26),
        geometry(20, 10, 36, 22),
        geometry(20, 10, 36, 26),
    ] {
        assert!(!b.has_end_indicator(icon, 12));
    }
}

#[test]
fn fully_transparent_rgb_and_flat_frames_have_no_targets() {
    for alpha in [0, 255] {
        let bytes = png(RgbaImage::from_fn(96, 64, |x, y| {
            let value = if alpha == 0 && (12..69).contains(&x) && (10..39).contains(&y) {
                32
            } else {
                200
            };
            Rgba([value, value, value, alpha])
        }));
        assert!(PixelRegionDetector
            .detect_rectangles(&bytes, 96, 64, 0.01, 20)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn validates_frame_size_limits_and_coordinates_after_downsampling() {
    let bytes = png(RgbaImage::from_fn(1801, 101, |x, y| {
        if (1640..1801).contains(&x) && (20..80).contains(&y) {
            Rgba([24, 63, 74, 255])
        } else {
            Rgba([255; 4])
        }
    }));
    let mismatch = PixelRegionDetector
        .detect_rectangles(&bytes, 1800, 101, 0.01, 4)
        .expect_err("capture metadata with the wrong dimensions must be rejected");
    assert!(
        matches!(mismatch, CoreError::Internal { code: InternalCode::Generic, message }
        if message == "Image dimensions differ from capture metadata")
    );
    let invalid_size = PixelRegionDetector
        .detect_rectangles(&bytes, 1801, 101, f32::NAN, 4)
        .expect_err("a non-finite minimum size must be rejected");
    assert!(
        matches!(invalid_size, CoreError::Internal { code: InternalCode::Generic, message }
        if message == "Invalid dimensions or minimum region size")
    );
    assert!(PixelRegionDetector
        .detect_rectangles(&bytes, 1801, 101, 0.01, 0)
        .unwrap()
        .is_empty());
    let boxes = PixelRegionDetector
        .detect_rectangles(&bytes, 1801, 101, 0.01, 1)
        .unwrap();
    assert_eq!(boxes.len(), 1);
    let b = &boxes[0].bounds;
    assert_eq!((b.x, b.y, b.width, b.height), (1640, 20, 161, 60));
    let n = &boxes[0].bounds_normalized;
    for (actual, expected) in [
        (n.x, 1640.0 / 1801.0),
        (n.y, 20.0 / 101.0),
        (n.width, 161.0 / 1801.0),
        (n.height, 60.0 / 101.0),
    ] {
        assert!((actual - expected).abs() < 1e-6);
    }
}

#[test]
fn joins_a_handle_to_its_rail_and_an_indicator_to_adjacent_ink() {
    let frame = png(RgbaImage::from_fn(240, 108, |x, y| {
        if ((24..150).contains(&x) && (75..78).contains(&y))
            || ((74..88).contains(&x) && (70..84).contains(&y))
        {
            Rgba([40, 80, 100, 255])
        } else if ((30..44).contains(&x) && (23..37).contains(&y))
            || ([51, 57, 64, 73, 80]
                .iter()
                .any(|&start| (start..start + 3).contains(&x))
                && (25..35).contains(&y))
        {
            Rgba([25, 25, 25, 255])
        } else {
            Rgba([248, 248, 248, 255])
        }
    }));
    let boxes = PixelRegionDetector
        .detect_rectangles(&frame, 240, 108, 0.02, 50)
        .unwrap();
    let find = |kind| {
        &boxes
            .iter()
            .find(|r| r.classification.as_deref() == Some(kind))
            .unwrap()
            .bounds
    };
    let indicator = find("indicator-and-label");
    assert!(indicator.x <= 30 && indicator.x + indicator.width as i32 >= 83);
    let track = find("track-and-handle");
    assert!(track.x <= 24 && track.x + track.width as i32 >= 150);
    assert!(track.y <= 70 && track.y + track.height as i32 >= 84);
}

#[test]
fn text_and_a_plain_divider_do_not_propose_a_compound_control() {
    let frame = png(RgbaImage::from_fn(180, 90, |x, y| {
        let ink = ((20..135).contains(&x) && (65..67).contains(&y))
            || ((20..90).contains(&x) && (23..35).contains(&y) && x % 8 < 3);
        if ink {
            Rgba([20, 20, 20, 255])
        } else {
            Rgba([242, 242, 242, 255])
        }
    }));
    let regions = PixelRegionDetector
        .detect_rectangles(&frame, 180, 90, 0.02, 50)
        .unwrap();
    assert!(!regions.iter().any(|r| matches!(
        r.classification.as_deref(),
        Some("track-and-handle" | "indicator-and-label")
    )));
}

#[test]
fn only_internal_ink_gaps_within_the_budget_are_joined() {
    let mut row = [1, 0, 1, 1, 0, 1, 1, 1, 0, 1];
    bridge_ink_gaps(&mut row, 2);
    assert_eq!(row, [1, 0, 0, 0, 0, 1, 1, 1, 0, 1]);
    let mut adjacent = [0, 0, 1, 0];
    bridge_ink_gaps(&mut adjacent, 0);
    assert_eq!(adjacent, [0, 0, 1, 0]);
}

#[test]
fn compound_size_limits_apply_to_both_axes_and_the_capture_minimum() {
    for (w, h, minimum, valid) in [
        (18, 6, 1, true),
        (6, 18, 1, true),
        (20, 5, 1, false),
        (17, 6, 1, false),
        (18, 6, 7, false),
        (27, 9, 9, true),
        (192, 64, 10, true),
        (195, 65, 10, false),
    ] {
        assert_eq!(geometry(0, 0, w, h).can_contain_compound(minimum), valid);
    }
}

#[test]
fn interaction_margins_follow_widget_orientation_and_stop_at_frame_edges() {
    let edges = |b: Box2d| (b.left, b.top, b.right, b.bottom);
    assert_eq!(
        edges(geometry(10, 10, 50, 22).with_interaction_margin(100, 100)),
        (7, 4, 53, 28)
    );
    assert_eq!(
        edges(geometry(10, 10, 22, 50).with_interaction_margin(100, 100)),
        (4, 7, 28, 53)
    );
    assert_eq!(
        edges(geometry(1, 1, 41, 13).with_interaction_margin(42, 14)),
        (0, 0, 42, 14)
    );
}

#[test]
fn duplicate_regions_prefer_rectangles_and_observe_the_exact_overlap() {
    let proposal = |kind, bounds| Proposal {
        kind,
        bounds,
        score: 0.8,
    };
    let full = geometry(0, 0, 100, 10);
    let partial = geometry(0, 0, 78, 10);
    let compact = proposal("compact-shape", full);
    let rectangle = proposal("rectangular-surface", partial);
    let far = proposal("rectangular-surface", geometry(20, 40, 25, 45));
    let result = distinct_proposals(vec![compact, far, rectangle], 2);
    assert_eq!(result.len(), 2);
    assert_eq!(
        (result[0].kind, result[0].bounds.width()),
        ("rectangular-surface", 78)
    );
    assert_eq!(result[1].bounds.top, 40);
    let reference = [proposal("rectangular-surface", full)];
    for threshold in [0.65, 0.78] {
        let w = (threshold * 100.0_f64).round() as usize;
        assert!(!geometry(0, 0, w, 10).distinct_from(&reference, threshold));
        assert!(geometry(0, 0, w - 1, 10).distinct_from(&reference, threshold));
    }
    assert_eq!(
        distinct_proposals(
            vec![
                proposal("rectangular-surface", full),
                proposal("rectangular-surface", geometry(20, 30, 28, 38)),
            ],
            1
        )
        .len(),
        1
    );
}

#[test]
fn a_compound_scan_needs_a_partial_surface_of_at_least_one_eighth() {
    assert!(!usable_surface(1, 9));
    assert!(usable_surface(2, 9));
    assert!(usable_surface(8, 9));
    assert!(!usable_surface(9, 9));
    assert!(!usable_surface(0, 0));
}

#[test]
fn minimum_region_size_scales_with_the_short_frame_edge_and_rounds_up() {
    let frame = png(RgbaImage::from_fn(150, 100, |x, y| {
        let ink = ((10..19).contains(&x) && (10..22).contains(&y))
            || ((35..45).contains(&x) && (10..25).contains(&y))
            || ((65..95).contains(&x) && (30..49).contains(&y));
        Rgba(if ink { [20, 30, 60, 255] } else { [255; 4] })
    }));
    let boxes = |minimum| {
        PixelRegionDetector
            .detect_rectangles(&frame, 150, 100, minimum, 5)
            .unwrap()
            .into_iter()
            .map(|p| (p.bounds.x, p.bounds.width, p.bounds.height))
            .collect::<Vec<_>>()
    };
    assert_eq!(boxes(0.0), [(65, 30, 19), (35, 10, 15)]);
    assert_eq!(boxes(0.101), [(65, 30, 19)]);
    assert!(boxes(0.191).is_empty());
}

#[test]
fn equally_large_background_surfaces_use_the_first_one_for_compound_controls() {
    let image = RgbaImage::from_fn(192, 56, |x, y| {
        let local = x % 96;
        let rail = (12..82).contains(&local) && (27..30).contains(&y);
        let handle = (42..52).contains(&local) && (22..35).contains(&y);
        Rgba(if rail || handle {
            [20, 40, 80, 255]
        } else if x < 96 {
            [240; 4]
        } else {
            [180; 4]
        })
    });
    let frame = png(image);
    let boxes = PixelRegionDetector
        .detect_rectangles(&frame, 192, 56, 0.0, 20)
        .unwrap();
    let controls: Vec<_> = boxes
        .iter()
        .filter(|p| p.classification.as_deref() == Some("track-and-handle"))
        .collect();
    assert_eq!(controls.len(), 1);
    // The left rail spans x=12..82 and its handle y=22..35.
    // Only foreground pixels determine the three- and six-pixel margins.
    let bounds = &controls[0].bounds;
    assert_eq!(
        (bounds.x, bounds.y, bounds.width, bounds.height),
        (9, 16, 76, 25)
    );
    let capped = PixelRegionDetector
        .detect_rectangles(&frame, 192, 56, 0.0, 1)
        .unwrap();
    assert_eq!(capped.len(), 1);
    assert_eq!(capped[0].classification, boxes[0].classification);
}
