//! #12204: pixel and coordinate controls for native-resolution screen crops.

use image::{Rgba, RgbaImage};
use maekon_core::error::CoreError;
use maekon_core::error_codes::ValidationCode;
use maekon_core::models::frame::BoundingBox;
use maekon_vision::screen_regions::{
    padded_region_bounds, prepare_screen_overview, prepare_screen_region, prepare_screen_regions,
    RegionDecision, ScreenRegionOptions,
};
use std::error::Error;

fn bounds(x: u32, y: u32, width: u32, height: u32) -> BoundingBox {
    BoundingBox {
        x,
        y,
        width,
        height,
    }
}

fn patterned_frame() -> RgbaImage {
    RgbaImage::from_fn(20, 10, |x, y| {
        Rgba([
            u8::try_from(x).unwrap_or(0),
            u8::try_from(y).unwrap_or(0),
            71,
            255,
        ])
    })
}

fn assert_invalid_arguments<T>(result: Result<T, CoreError>, expected: &str) {
    match result {
        Err(CoreError::InvalidArguments { code, message }) => {
            assert_eq!(code, ValidationCode::InvalidArguments);
            assert_eq!(message, expected);
        }
        Err(other) => panic!("expected invalid arguments, got {other}"),
        Ok(_) => panic!("expected invalid arguments, preparation succeeded"),
    }
}

#[test]
fn crop_preserves_every_pixel_and_source_coordinate() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let requested = bounds(7, 3, 5, 4);
    let crop = prepare_screen_region(&frame, &requested, 7, 20)?;
    assert_eq!(crop.candidate_index(), 7);
    assert_eq!(crop.source_bounds(), &requested);
    assert_eq!(crop.image().dimensions(), (5, 4));
    for (x, y, pixel) in crop.image().enumerate_pixels() {
        assert_eq!(crop.source_pixel(x, y), Some((7 + x, 3 + y)));
        assert_eq!(pixel, frame.get_pixel(7 + x, 3 + y));
    }
    Ok(())
}

#[test]
fn source_pixels_are_exclusive_at_each_crop_edge() -> Result<(), Box<dyn Error>> {
    let crop = prepare_screen_region(&patterned_frame(), &bounds(7, 3, 5, 4), 0, 20)?;
    assert_eq!(crop.source_pixel(0, 0), Some((7, 3)));
    assert_eq!(crop.source_pixel(4, 3), Some((11, 6)));
    for (x, y) in [(5, 0), (0, 4), (5, 4), (u32::MAX, 0), (0, u32::MAX)] {
        assert_eq!(crop.source_pixel(x, y), None);
    }
    Ok(())
}

#[test]
fn exact_frame_edges_and_one_pixel_crops_are_valid() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    for requested in [
        bounds(0, 0, 20, 10),
        bounds(0, 0, 1, 1),
        bounds(19, 9, 1, 1),
        bounds(19, 0, 1, 10),
        bounds(0, 9, 20, 1),
    ] {
        let crop = prepare_screen_region(&frame, &requested, 3, requested.area())?;
        assert_eq!(crop.source_bounds(), &requested);
        assert_eq!(
            crop.image().dimensions(),
            (requested.width, requested.height)
        );
        assert_eq!(
            crop.image()
                .get_pixel(requested.width - 1, requested.height - 1),
            frame.get_pixel(
                requested.x + requested.width - 1,
                requested.y + requested.height - 1
            ),
        );
    }
    Ok(())
}

#[test]
fn zero_and_out_of_frame_bounds_are_rejected_without_clipping() {
    let frame = patterned_frame();
    for requested in [
        bounds(0, 0, 0, 1),
        bounds(0, 0, 1, 0),
        bounds(0, 0, 0, 0),
        bounds(19, 0, 2, 1),
        bounds(0, 9, 1, 2),
        bounds(20, 0, 1, 1),
        bounds(0, 10, 1, 1),
    ] {
        assert_invalid_arguments(
            prepare_screen_region(&frame, &requested, 0, 200),
            "screen region bounds exceed frame",
        );
    }
}

#[test]
fn coordinate_overflow_is_rejected_on_both_axes() {
    let frame = patterned_frame();
    for requested in [
        bounds(u32::MAX, 0, 2, 1),
        bounds(0, u32::MAX, 1, 2),
        bounds(1, 1, u32::MAX, u32::MAX),
    ] {
        assert_invalid_arguments(
            prepare_screen_region(&frame, &requested, 0, 200),
            "screen region bounds overflow",
        );
    }
}

#[test]
fn empty_source_frames_cannot_produce_nonempty_crops() {
    for (width, height) in [(0, 10), (20, 0), (0, 0)] {
        assert_invalid_arguments(
            prepare_screen_region(&RgbaImage::new(width, height), &bounds(0, 0, 1, 1), 0, 1),
            "screen region bounds exceed frame",
        );
    }
}

#[test]
fn exact_budget_succeeds_and_insufficient_budgets_do_not_resize() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let requested = bounds(7, 3, 5, 4);
    for budget in [0, 19] {
        assert_invalid_arguments(
            prepare_screen_region(&frame, &requested, 0, budget),
            "screen region crop exceeds pixel budget",
        );
    }
    for budget in [20, 21, u64::MAX] {
        let crop = prepare_screen_region(&frame, &requested, 0, budget)?;
        assert_eq!(crop.image().dimensions(), (5, 4));
    }
    Ok(())
}

#[test]
fn crop_is_an_immutable_snapshot_of_the_supplied_frame() -> Result<(), Box<dyn Error>> {
    let mut frame = patterned_frame();
    let original = frame.clone();
    let requested = bounds(7, 3, 5, 4);
    let crop = prepare_screen_region(&frame, &requested, 0, 20)?;
    assert_eq!(frame, original);
    frame.put_pixel(7, 3, Rgba([255, 0, 0, 255]));
    assert_eq!(crop.image().get_pixel(0, 0), original.get_pixel(7, 3));
    assert_ne!(crop.image().get_pixel(0, 0), frame.get_pixel(7, 3));
    Ok(())
}

#[test]
fn replacing_the_image_changes_the_crop() -> Result<(), Box<dyn Error>> {
    let red = RgbaImage::from_pixel(20, 10, Rgba([255, 0, 0, 255]));
    let blue = RgbaImage::from_pixel(20, 10, Rgba([0, 0, 255, 255]));
    let requested = bounds(7, 3, 5, 4);
    let first = prepare_screen_region(&red, &requested, 0, 20)?;
    let second = prepare_screen_region(&blue, &requested, 0, 20)?;
    assert_ne!(first.image(), second.image());
    assert_eq!(first.image().get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    assert_eq!(second.image().get_pixel(0, 0), &Rgba([0, 0, 255, 255]));
    Ok(())
}

#[test]
fn changing_the_original_bounds_does_not_change_crop_provenance() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let mut requested = bounds(7, 3, 5, 4);
    let crop = prepare_screen_region(&frame, &requested, 9, 20)?;
    requested.x = 0;
    requested.y = 0;
    assert_eq!(crop.source_bounds(), &bounds(7, 3, 5, 4));
    assert_eq!(crop.candidate_index(), 9);
    assert_eq!(crop.source_pixel(0, 0), Some((7, 3)));
    assert_ne!(crop.source_bounds(), &requested);
    Ok(())
}

#[test]
fn overview_retains_the_whole_small_frame_without_upscaling() -> Result<(), Box<dyn Error>> {
    let mut frame = patterned_frame();
    let original = frame.clone();
    let overview = prepare_screen_overview(&frame, 4096, 200, 200)?;
    assert_eq!(overview.source_dimensions(), (20, 10));
    assert_eq!(overview.image(), &original);
    assert_eq!(frame, original);
    frame.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    assert_eq!(overview.image(), &original);
    assert_ne!(overview.image(), &frame);
    Ok(())
}

#[test]
fn overview_dimensions_preserve_both_orientations_and_rounding() -> Result<(), Box<dyn Error>> {
    for (width, height, edge, expected) in [
        (20, 10, 8, (8, 4)),
        (10, 20, 8, (4, 8)),
        (19, 7, 8, (8, 2)),
        (7, 19, 8, (2, 8)),
        (1, 101, 8, (1, 8)),
        (101, 1, 8, (8, 1)),
        (9, 9, 3, (3, 3)),
        (1, 1, 4096, (1, 1)),
        (20, 10, 20, (20, 10)),
    ] {
        let frame = RgbaImage::from_pixel(width, height, Rgba([23, 67, 89, 255]));
        let overview = prepare_screen_overview(&frame, edge, 1000, 1000)?;
        assert_eq!(overview.source_dimensions(), (width, height));
        assert_eq!(overview.image().dimensions(), expected);
        assert!(overview
            .image()
            .pixels()
            .all(|pixel| pixel == &Rgba([23, 67, 89, 255])));
    }
    Ok(())
}

#[test]
fn overview_keeps_opposite_screen_edges_in_global_context() -> Result<(), Box<dyn Error>> {
    let frame = RgbaImage::from_fn(20, 10, |x, _| {
        if x < 10 {
            Rgba([255, 0, 0, 255])
        } else {
            Rgba([0, 0, 255, 255])
        }
    });
    let overview = prepare_screen_overview(&frame, 8, 200, 32)?;
    assert_eq!(overview.image().dimensions(), (8, 4));
    assert_eq!(overview.image().get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    assert_eq!(overview.image().get_pixel(7, 3), &Rgba([0, 0, 255, 255]));
    Ok(())
}

#[test]
fn overview_checks_exact_source_and_output_budgets() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    for source_budget in [0, 199] {
        assert_invalid_arguments(
            prepare_screen_overview(&frame, 8, source_budget, 32),
            "screen overview source exceeds pixel limit",
        );
    }
    for output_budget in [0, 31] {
        assert_invalid_arguments(
            prepare_screen_overview(&frame, 8, 200, output_budget),
            "screen overview exceeds pixel budget",
        );
    }
    for source_budget in [200, 201, u64::MAX] {
        for output_budget in [32, 33, u64::MAX] {
            let overview = prepare_screen_overview(&frame, 8, source_budget, output_budget)?;
            assert_eq!(overview.image().dimensions(), (8, 4));
        }
    }
    Ok(())
}

#[test]
fn overview_rejects_empty_frames_and_invalid_edge_limits() {
    for (width, height) in [(0, 10), (20, 0), (0, 0)] {
        assert_invalid_arguments(
            prepare_screen_overview(&RgbaImage::new(width, height), 8, 200, 32),
            "screen overview source exceeds pixel limit",
        );
    }
    for edge in [0, 4097, u32::MAX] {
        assert_invalid_arguments(
            prepare_screen_overview(&patterned_frame(), edge, 200, 200),
            "screen overview edge exceeds limits",
        );
    }
}

#[test]
fn overview_and_native_crop_use_the_same_supplied_frame() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let original = frame.clone();
    let overview = prepare_screen_overview(&frame, 8, 200, 32)?;
    let crop = prepare_screen_region(&frame, &bounds(7, 3, 5, 4), 6, 20)?;
    assert_eq!(overview.source_dimensions(), (20, 10));
    assert_eq!(overview.image().dimensions(), (8, 4));
    assert_eq!(crop.image().dimensions(), (5, 4));
    assert_eq!(crop.source_pixel(0, 0), Some((7, 3)));
    assert_eq!(crop.image().get_pixel(0, 0), frame.get_pixel(7, 3));
    assert_eq!(frame, original);
    Ok(())
}

#[test]
fn replacing_the_frame_changes_the_overview() -> Result<(), Box<dyn Error>> {
    let red = RgbaImage::from_pixel(20, 10, Rgba([255, 0, 0, 255]));
    let blue = RgbaImage::from_pixel(20, 10, Rgba([0, 0, 255, 255]));
    let first = prepare_screen_overview(&red, 8, 200, 32)?;
    let second = prepare_screen_overview(&blue, 8, 200, 32)?;
    assert_ne!(first.image(), second.image());
    assert_eq!(first.image().get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
    assert_eq!(second.image().get_pixel(0, 0), &Rgba([0, 0, 255, 255]));
    Ok(())
}

#[test]
fn zero_padding_preserves_valid_source_bounds() {
    for candidate in [
        bounds(7, 3, 5, 4),
        bounds(0, 0, 20, 10),
        bounds(19, 9, 1, 1),
    ] {
        assert_eq!(padded_region_bounds(&candidate, 20, 10, 0), Some(candidate));
    }
}

#[test]
fn padding_expands_the_native_crop_in_source_pixel_space() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let original = frame.clone();
    let candidate = bounds(7, 3, 5, 4);
    let padded =
        padded_region_bounds(&candidate, 20, 10, 2).ok_or("valid source bounds were rejected")?;
    assert_eq!(padded, bounds(5, 1, 9, 8));
    let crop = prepare_screen_region(&frame, &padded, 6, 72)?;
    assert_eq!(crop.source_bounds(), &padded);
    assert_eq!(crop.source_pixel(0, 0), Some((5, 1)));
    assert_eq!(crop.source_pixel(8, 7), Some((13, 8)));
    for (x, y, pixel) in crop.image().enumerate_pixels() {
        assert_eq!(pixel, frame.get_pixel(x + 5, y + 1));
    }
    assert_eq!(candidate, bounds(7, 3, 5, 4));
    assert_eq!(frame, original);
    Ok(())
}

#[test]
fn padding_clamps_only_added_context_at_each_frame_edge() {
    for (candidate, expected) in [
        (bounds(0, 4, 2, 2), bounds(0, 1, 5, 8)),
        (bounds(18, 4, 2, 2), bounds(15, 1, 5, 8)),
        (bounds(7, 0, 5, 2), bounds(4, 0, 11, 5)),
        (bounds(7, 8, 5, 2), bounds(4, 5, 11, 5)),
        (bounds(0, 0, 1, 1), bounds(0, 0, 4, 4)),
        (bounds(19, 9, 1, 1), bounds(16, 6, 4, 4)),
        (bounds(0, 0, 20, 10), bounds(0, 0, 20, 10)),
    ] {
        assert_eq!(padded_region_bounds(&candidate, 20, 10, 3), Some(expected));
    }
}

#[test]
fn maximum_padding_saturates_to_the_complete_frame() {
    for candidate in [bounds(7, 3, 5, 4), bounds(0, 0, 1, 1), bounds(19, 9, 1, 1)] {
        assert_eq!(
            padded_region_bounds(&candidate, 20, 10, u32::MAX),
            Some(bounds(0, 0, 20, 10))
        );
    }
}

#[test]
fn invalid_source_bounds_are_rejected_before_padding() {
    for candidate in [
        bounds(0, 0, 0, 1),
        bounds(0, 0, 1, 0),
        bounds(20, 0, 1, 1),
        bounds(0, 10, 1, 1),
        bounds(21, 0, 1, 1),
        bounds(0, 11, 1, 1),
        bounds(18, 0, 3, 1),
        bounds(0, 8, 1, 3),
        bounds(u32::MAX - 2, 0, 3, 1),
        bounds(0, u32::MAX - 2, 1, 3),
        bounds(u32::MAX, 0, 1, 1),
        bounds(0, u32::MAX, 1, 1),
    ] {
        for padding in [0, 2, u32::MAX] {
            assert_eq!(padded_region_bounds(&candidate, 20, 10, padding), None);
        }
    }
}

#[test]
fn padding_rejects_empty_frame_dimensions() {
    for (width, height) in [(0, 10), (20, 0), (0, 0)] {
        for padding in [0, u32::MAX] {
            assert_eq!(
                padded_region_bounds(&bounds(0, 0, 1, 1), width, height, padding),
                None
            );
        }
    }
}

#[test]
fn padding_handles_u32_boundaries_without_image_allocation() {
    let maximum = u32::MAX;
    assert_eq!(
        padded_region_bounds(&bounds(maximum - 4, maximum - 6, 4, 6), maximum, maximum, 3),
        Some(bounds(maximum - 7, maximum - 9, 7, 9))
    );
    assert_eq!(
        padded_region_bounds(&bounds(0, 0, 1, 1), maximum, maximum, maximum),
        Some(bounds(0, 0, maximum, maximum))
    );
    assert_eq!(
        padded_region_bounds(&bounds(maximum - 4, 0, 5, 1), maximum, maximum, maximum),
        None
    );
    assert_eq!(
        padded_region_bounds(&bounds(0, maximum - 6, 1, 7), maximum, maximum, maximum),
        None
    );
}

fn region_options() -> ScreenRegionOptions {
    ScreenRegionOptions {
        overview_max_edge: 8,
        max_crops: 4,
        max_output_pixels: 200,
        padding: 0,
    }
}

#[test]
fn bundle_preserves_pixels_priority_coordinates_and_source() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let original = frame.clone();
    let candidates = [bounds(14, 6, 4, 3), bounds(2, 1, 5, 4)];
    let prepared = prepare_screen_regions(
        &frame,
        &candidates,
        ScreenRegionOptions {
            max_output_pixels: 64,
            ..region_options()
        },
    )?;
    assert_eq!(prepared.source_dimensions(), (20, 10));
    assert_eq!(prepared.overview().dimensions(), (8, 4));
    assert_eq!(prepared.output_pixels(), 64);
    assert_eq!(
        prepared.decisions(),
        &[
            RegionDecision::Included { crop_index: 0 },
            RegionDecision::Included { crop_index: 1 },
        ],
    );
    assert_eq!(prepared.crops().len(), 2);
    for (index, crop) in prepared.crops().iter().enumerate() {
        assert_eq!(crop.candidate_index(), index);
        assert_eq!(crop.source_bounds(), &candidates[index]);
        for (x, y, pixel) in crop.image().enumerate_pixels() {
            let source_x = candidates[index].x + x;
            let source_y = candidates[index].y + y;
            assert_eq!(crop.source_pixel(x, y), Some((source_x, source_y)));
            assert_eq!(pixel, frame.get_pixel(source_x, source_y));
        }
    }
    assert_eq!(frame, original);
    Ok(())
}

#[test]
fn bundle_uses_caller_priority_before_crop_count_limit() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let first = bounds(2, 1, 5, 4);
    let second = bounds(14, 6, 4, 3);
    for candidates in [[first.clone(), second.clone()], [second, first]] {
        let prepared = prepare_screen_regions(
            &frame,
            &candidates,
            ScreenRegionOptions {
                max_crops: 1,
                ..region_options()
            },
        )?;
        assert_eq!(prepared.crops().len(), 1);
        assert_eq!(prepared.crops()[0].source_bounds(), &candidates[0]);
        assert_eq!(prepared.crops()[0].candidate_index(), 0);
        assert_eq!(
            prepared.decisions(),
            &[
                RegionDecision::Included { crop_index: 0 },
                RegionDecision::CropLimit
            ],
        );
    }
    Ok(())
}

#[test]
fn bundle_retains_overview_for_empty_rejected_and_zero_crop_inputs() -> Result<(), Box<dyn Error>> {
    let frame = RgbaImage::from_pixel(20, 10, Rgba([23, 67, 89, 255]));
    for (candidates, expected) in [
        (vec![], vec![]),
        (
            vec![bounds(0, 0, 0, 1)],
            vec![RegionDecision::InvalidBounds],
        ),
        (vec![bounds(0, 0, 1, 1)], vec![RegionDecision::CropLimit]),
    ] {
        let prepared = prepare_screen_regions(
            &frame,
            &candidates,
            ScreenRegionOptions {
                max_crops: 0,
                max_output_pixels: 32,
                ..region_options()
            },
        )?;
        assert_eq!(prepared.overview().dimensions(), (8, 4));
        assert!(prepared
            .overview()
            .pixels()
            .all(|pixel| pixel == &Rgba([23, 67, 89, 255])));
        assert_eq!(prepared.output_pixels(), 32);
        assert!(prepared.crops().is_empty());
        assert_eq!(prepared.decisions(), expected);
    }
    Ok(())
}

#[test]
fn bundle_invalid_bounds_do_not_hide_later_valid_candidates() -> Result<(), Box<dyn Error>> {
    let candidates = [
        bounds(0, 0, 0, 1),
        bounds(0, 0, 1, 0),
        bounds(19, 0, 2, 1),
        bounds(0, 9, 1, 2),
        bounds(u32::MAX, 0, 2, 1),
        bounds(0, u32::MAX, 1, 2),
        bounds(1, 1, u32::MAX, u32::MAX),
        bounds(19, 9, 1, 1),
    ];
    let prepared = prepare_screen_regions(&patterned_frame(), &candidates, region_options())?;
    assert_eq!(prepared.decisions().len(), candidates.len());
    assert_eq!(
        &prepared.decisions()[..7],
        &[RegionDecision::InvalidBounds; 7]
    );
    assert_eq!(
        prepared.decisions()[7],
        RegionDecision::Included { crop_index: 0 }
    );
    assert_eq!(prepared.crops().len(), 1);
    assert_eq!(prepared.crops()[0].candidate_index(), 7);
    assert_eq!(prepared.crops()[0].source_bounds(), &bounds(19, 9, 1, 1));
    Ok(())
}

#[test]
fn bundle_padding_clamps_context_without_rescuing_invalid_bounds() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let candidates = [bounds(0, 0, 1, 1), bounds(19, 9, 1, 1), bounds(20, 0, 1, 1)];
    let prepared = prepare_screen_regions(
        &frame,
        &candidates,
        ScreenRegionOptions {
            padding: 2,
            ..region_options()
        },
    )?;
    assert_eq!(prepared.crops().len(), 2);
    assert_eq!(prepared.crops()[0].source_bounds(), &bounds(0, 0, 3, 3));
    assert_eq!(prepared.crops()[1].source_bounds(), &bounds(17, 7, 3, 3));
    assert_eq!(
        prepared.crops()[1].image().get_pixel(2, 2),
        frame.get_pixel(19, 9)
    );
    assert_eq!(prepared.output_pixels(), 50);
    assert_eq!(prepared.decisions()[2], RegionDecision::InvalidBounds);
    Ok(())
}

#[test]
fn bundle_deduplicates_padded_bounds_before_count_and_pixel_limits() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let prepared = prepare_screen_regions(
        &frame,
        &[bounds(0, 0, 1, 1), bounds(19, 9, 1, 1), bounds(20, 0, 1, 1)],
        ScreenRegionOptions {
            max_crops: 1,
            max_output_pixels: 232,
            padding: u32::MAX,
            ..region_options()
        },
    )?;
    assert_eq!(prepared.crops().len(), 1);
    assert_eq!(prepared.crops()[0].source_bounds(), &bounds(0, 0, 20, 10));
    assert_eq!(prepared.crops()[0].image(), &frame);
    assert_eq!(prepared.output_pixels(), 232);
    assert_eq!(
        prepared.decisions(),
        &[
            RegionDecision::Included { crop_index: 0 },
            RegionDecision::DuplicateOf { crop_index: 0 },
            RegionDecision::InvalidBounds,
        ],
    );
    Ok(())
}

#[test]
fn bundle_retains_nested_and_partially_overlapping_crops() -> Result<(), Box<dyn Error>> {
    let candidates = [bounds(1, 1, 10, 6), bounds(2, 2, 3, 2), bounds(9, 5, 5, 4)];
    let prepared = prepare_screen_regions(
        &patterned_frame(),
        &candidates,
        ScreenRegionOptions {
            max_output_pixels: 118,
            ..region_options()
        },
    )?;
    assert_eq!(prepared.crops().len(), 3);
    assert_eq!(prepared.output_pixels(), 118);
    for (index, candidate) in candidates.iter().enumerate() {
        assert_eq!(prepared.crops()[index].source_bounds(), candidate);
        assert_eq!(
            prepared.decisions()[index],
            RegionDecision::Included { crop_index: index }
        );
    }
    Ok(())
}

#[test]
fn bundle_budget_reserves_overview_and_keeps_later_affordable_candidates(
) -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let candidates = [
        bounds(0, 0, 20, 10),
        bounds(2, 1, 5, 4),
        bounds(14, 6, 4, 3),
        bounds(0, 0, 1, 1),
        bounds(14, 6, 4, 3),
    ];
    let exact = prepare_screen_regions(
        &frame,
        &candidates,
        ScreenRegionOptions {
            max_output_pixels: 64,
            ..region_options()
        },
    )?;
    assert_eq!(exact.output_pixels(), 64);
    assert_eq!(exact.crops().len(), 2);
    assert_eq!(
        exact.decisions(),
        &[
            RegionDecision::PixelBudget,
            RegionDecision::Included { crop_index: 0 },
            RegionDecision::Included { crop_index: 1 },
            RegionDecision::PixelBudget,
            RegionDecision::DuplicateOf { crop_index: 1 },
        ],
    );
    assert_eq!(exact.crops()[0].image().dimensions(), (5, 4));
    assert_eq!(exact.crops()[1].candidate_index(), 2);

    let smaller = prepare_screen_regions(
        &frame,
        &candidates,
        ScreenRegionOptions {
            max_output_pixels: 63,
            ..region_options()
        },
    )?;
    assert_eq!(smaller.output_pixels(), 53);
    assert_eq!(smaller.decisions()[2], RegionDecision::PixelBudget);
    assert_eq!(
        smaller.decisions()[3],
        RegionDecision::Included { crop_index: 1 }
    );
    assert_eq!(smaller.crops()[1].candidate_index(), 3);
    assert_eq!(smaller.decisions()[4], RegionDecision::PixelBudget);
    Ok(())
}

#[test]
fn bundle_budget_counts_padded_pixels_at_the_exact_boundary() -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    for (budget, expected, crop_count) in [
        (47, RegionDecision::PixelBudget, 0),
        (48, RegionDecision::Included { crop_index: 0 }, 1),
    ] {
        let prepared = prepare_screen_regions(
            &frame,
            &[bounds(5, 4, 2, 2)],
            ScreenRegionOptions {
                max_output_pixels: budget,
                padding: 1,
                ..region_options()
            },
        )?;
        assert_eq!(prepared.decisions(), &[expected]);
        assert_eq!(prepared.crops().len(), crop_count);
        assert_eq!(prepared.output_pixels(), 32 + 16 * crop_count as u64);
    }
    Ok(())
}

#[test]
fn bundle_is_a_snapshot_and_image_replacement_changes_all_images() -> Result<(), Box<dyn Error>> {
    let mut red = RgbaImage::from_pixel(20, 10, Rgba([255, 0, 0, 255]));
    let blue = RgbaImage::from_pixel(20, 10, Rgba([0, 0, 255, 255]));
    let original = red.clone();
    let mut candidates = [bounds(7, 3, 5, 4)];
    let first = prepare_screen_regions(&red, &candidates, region_options())?;
    let second = prepare_screen_regions(&blue, &candidates, region_options())?;
    assert_eq!(red, original);
    red.fill(0);
    candidates[0].x = 0;
    assert_eq!(first.crops()[0].source_bounds(), &bounds(7, 3, 5, 4));
    assert_ne!(first.crops()[0].source_bounds(), &candidates[0]);
    assert_ne!(
        first.crops()[0].image().get_pixel(0, 0),
        red.get_pixel(7, 3)
    );
    assert_ne!(first.overview(), second.overview());
    assert_ne!(first.crops()[0].image(), second.crops()[0].image());
    for image in [first.overview(), first.crops()[0].image()] {
        assert!(image.pixels().all(|pixel| pixel == &Rgba([255, 0, 0, 255])));
    }
    for image in [second.overview(), second.crops()[0].image()] {
        assert!(image.pixels().all(|pixel| pixel == &Rgba([0, 0, 255, 255])));
    }
    Ok(())
}

#[test]
fn bundle_accepts_candidate_and_crop_caps_but_rejects_candidate_floods(
) -> Result<(), Box<dyn Error>> {
    let frame = patterned_frame();
    let mut candidates = vec![bounds(0, 0, 1, 1); 4096];
    let prepared = prepare_screen_regions(&frame, &candidates, region_options())?;
    assert_eq!(prepared.decisions().len(), 4096);
    assert_eq!(prepared.crops().len(), 1);
    assert_eq!(prepared.output_pixels(), 33);
    assert_eq!(
        prepared.decisions()[4095],
        RegionDecision::DuplicateOf { crop_index: 0 }
    );
    candidates.push(bounds(1, 0, 1, 1));
    assert_invalid_arguments(
        prepare_screen_regions(&frame, &candidates, region_options()),
        "screen region candidate count exceeds limit",
    );

    let distinct: Vec<_> = (0..17).map(|x| bounds(x, 0, 1, 1)).collect();
    let capped = prepare_screen_regions(
        &frame,
        &distinct,
        ScreenRegionOptions {
            max_crops: 16,
            ..region_options()
        },
    )?;
    assert_eq!(capped.crops().len(), 16);
    assert_eq!(capped.output_pixels(), 48);
    assert_eq!(
        capped.decisions()[15],
        RegionDecision::Included { crop_index: 15 }
    );
    assert_eq!(capped.decisions()[16], RegionDecision::CropLimit);
    Ok(())
}

#[test]
fn bundle_rejects_invalid_options_sources_and_missing_overview_budget() -> Result<(), Box<dyn Error>>
{
    let frame = patterned_frame();
    for max_crops in [17, usize::MAX] {
        assert_invalid_arguments(
            prepare_screen_regions(
                &frame,
                &[],
                ScreenRegionOptions {
                    max_crops,
                    ..region_options()
                },
            ),
            "screen region options exceed limits",
        );
    }
    for max_output_pixels in [0, 16_000_001, u64::MAX] {
        assert_invalid_arguments(
            prepare_screen_regions(
                &frame,
                &[],
                ScreenRegionOptions {
                    max_output_pixels,
                    ..region_options()
                },
            ),
            "screen region options exceed limits",
        );
    }
    let at_limit = prepare_screen_regions(
        &frame,
        &[],
        ScreenRegionOptions {
            max_output_pixels: 16_000_000,
            ..region_options()
        },
    )?;
    assert_eq!(at_limit.output_pixels(), 32);
    for overview_max_edge in [0, 4097, u32::MAX] {
        assert_invalid_arguments(
            prepare_screen_regions(
                &frame,
                &[],
                ScreenRegionOptions {
                    overview_max_edge,
                    ..region_options()
                },
            ),
            "screen overview edge exceeds limits",
        );
    }
    assert_invalid_arguments(
        prepare_screen_regions(
            &frame,
            &[bounds(0, 0, 1, 1)],
            ScreenRegionOptions {
                max_output_pixels: 31,
                ..region_options()
            },
        ),
        "screen overview exceeds pixel budget",
    );
    for (width, height) in [(0, 10), (20, 0), (0, 0)] {
        assert_invalid_arguments(
            prepare_screen_regions(&RgbaImage::new(width, height), &[], region_options()),
            "screen overview source exceeds pixel limit",
        );
    }
    let defaults = prepare_screen_regions(&frame, &[], ScreenRegionOptions::default())?;
    assert_eq!(defaults.overview(), &frame);
    assert_eq!(defaults.output_pixels(), 200);
    Ok(())
}

#[test]
fn bundle_defaults_bound_overview_context_and_crop_count() -> Result<(), Box<dyn Error>> {
    let default_frame = RgbaImage::from_pixel(600, 100, Rgba([9, 17, 31, 255]));
    let default_candidates: Vec<_> = (0..5)
        .map(|index| bounds(50 + 100 * index, 40, 1, 1))
        .collect();
    let default_bundle = prepare_screen_regions(
        &default_frame,
        &default_candidates,
        ScreenRegionOptions::default(),
    )?;
    assert_eq!(default_bundle.overview().dimensions(), (512, 85));
    assert_eq!(default_bundle.crops().len(), 4);
    assert_eq!(
        default_bundle.crops()[0].source_bounds(),
        &bounds(34, 24, 33, 33)
    );
    assert_eq!(default_bundle.output_pixels(), 47_876);
    assert_eq!(default_bundle.decisions()[4], RegionDecision::CropLimit);
    Ok(())
}
