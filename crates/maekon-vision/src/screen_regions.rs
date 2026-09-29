//! Validated native-resolution crops and bounded screen overviews (#12204).
//!
//! Callers supply bounds in this frame's pixel space and a remaining pixel
//! budget. Apply privacy transformations before use. This module does not
//! detect layouts, capture screens, save images, or grant egress permission.

use image::{imageops, RgbaImage};
use maekon_core::error::CoreError;
use maekon_core::error_codes::ValidationCode;
use maekon_core::models::frame::BoundingBox;

/// Pixels and coordinates are kept together and exposed immutably.
/// Intentionally does not implement Debug or serialization for raw images.
pub struct ScreenRegionCrop {
    candidate_index: usize,
    source_bounds: BoundingBox,
    image: RgbaImage,
}

impl ScreenRegionCrop {
    #[must_use]
    pub fn candidate_index(&self) -> usize {
        self.candidate_index
    }

    #[must_use]
    pub fn source_bounds(&self) -> &BoundingBox {
        &self.source_bounds
    }

    #[must_use]
    pub fn image(&self) -> &RgbaImage {
        &self.image
    }

    /// Map a crop pixel to the source image, never to OS logical coordinates.
    #[must_use]
    pub fn source_pixel(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        if x >= self.image.width() || y >= self.image.height() {
            return None;
        }
        Some((
            self.source_bounds.x.checked_add(x)?,
            self.source_bounds.y.checked_add(y)?,
        ))
    }
}

fn invalid_arguments(message: &str) -> CoreError {
    CoreError::InvalidArguments {
        code: ValidationCode::InvalidArguments,
        message: message.to_owned(),
    }
}

/// Copy one valid region without resizing or silently clipping its bounds.
///
/// The caller owns frame/policy identity and the total request budget. This
/// primitive limits the returned crop to `max_output_pixels`, not peak memory.
/// A zero budget rejects every nonempty crop.
///
/// # Errors
///
/// Returns `CoreError::InvalidArguments` for zero, overflowing, or out-of-frame
/// bounds, or when the native-resolution crop exceeds the supplied budget.
pub fn prepare_screen_region(
    frame: &RgbaImage,
    bounds: &BoundingBox,
    candidate_index: usize,
    max_output_pixels: u64,
) -> Result<ScreenRegionCrop, CoreError> {
    let right = bounds
        .x
        .checked_add(bounds.width)
        .ok_or_else(|| invalid_arguments("screen region bounds overflow"))?;
    let bottom = bounds
        .y
        .checked_add(bounds.height)
        .ok_or_else(|| invalid_arguments("screen region bounds overflow"))?;
    if bounds.width == 0 || bounds.height == 0 || right > frame.width() || bottom > frame.height() {
        return Err(invalid_arguments("screen region bounds exceed frame"));
    }
    if bounds.area() > max_output_pixels {
        return Err(invalid_arguments("screen region crop exceeds pixel budget"));
    }
    let image =
        imageops::crop_imm(frame, bounds.x, bounds.y, bounds.width, bounds.height).to_image();
    Ok(ScreenRegionCrop {
        candidate_index,
        source_bounds: bounds.clone(),
        image,
    })
}

const MAX_OVERVIEW_SOURCE_PIXELS: u64 = 64_000_000;
const MAX_OVERVIEW_EDGE: u32 = 4096;

/// Global context and the source dimensions used to create it.
/// Intentionally does not implement Debug or serialization for raw images.
pub struct ScreenOverview {
    source_dimensions: (u32, u32),
    image: RgbaImage,
}

impl ScreenOverview {
    #[must_use]
    pub fn source_dimensions(&self) -> (u32, u32) {
        self.source_dimensions
    }

    #[must_use]
    pub fn image(&self) -> &RgbaImage {
        &self.image
    }
}

/// Retain the whole frame within an edge limit and a supplied pixel budget.
///
/// Neither axis is upscaled. Dimensions are rounded down and clamped to one
/// pixel, so an extreme aspect ratio can have a one-pixel rounding difference.
/// Triangle filtering applies only to this overview; native crops stay intact.
///
/// The source limit can be reduced by the caller but never exceeds 64 million
/// pixels. The longest returned edge cannot exceed 4096 pixels. These limits
/// cover source dimensions and returned pixels, not peak memory or model cost.
/// The caller owns frame/policy identity and the aggregate overview/crop budget.
///
/// # Errors
///
/// Returns `CoreError::InvalidArguments` for an empty or oversized source, an
/// edge limit outside 1..=4096, or a budget smaller than the complete overview.
pub fn prepare_screen_overview(
    frame: &RgbaImage,
    max_edge: u32,
    max_source_pixels: u64,
    max_output_pixels: u64,
) -> Result<ScreenOverview, CoreError> {
    let (width, height) = frame.dimensions();
    let source_pixels = u64::from(width) * u64::from(height);
    if source_pixels == 0 || source_pixels > max_source_pixels.min(MAX_OVERVIEW_SOURCE_PIXELS) {
        return Err(invalid_arguments(
            "screen overview source exceeds pixel limit",
        ));
    }
    if max_edge == 0 || max_edge > MAX_OVERVIEW_EDGE {
        return Err(invalid_arguments("screen overview edge exceeds limits"));
    }
    let denominator = u64::from(width.max(height).max(max_edge));
    let output_width = ((u64::from(width) * u64::from(max_edge)) / denominator).max(1) as u32;
    let output_height = ((u64::from(height) * u64::from(max_edge)) / denominator).max(1) as u32;
    if u64::from(output_width) * u64::from(output_height) > max_output_pixels {
        return Err(invalid_arguments("screen overview exceeds pixel budget"));
    }
    let image = imageops::resize(
        frame,
        output_width,
        output_height,
        imageops::FilterType::Triangle,
    );
    Ok(ScreenOverview {
        source_dimensions: (width, height),
        image,
    })
}

/// Expand valid source-pixel bounds while clamping added context to the frame.
///
/// The original rectangle must fit entirely before padding. Returns `None`
/// for zero-area, overflowing or out-of-frame bounds, including empty frames.
/// Even maximum padding cannot turn an invalid source rectangle into a crop.
///
/// This geometry-only helper does not allocate or inspect an image. Callers
/// must supply dimensions from the same frame and retain privacy/policy checks.
/// Pass the result to [`prepare_screen_region`] with an explicit pixel budget.
#[must_use]
pub fn padded_region_bounds(
    bounds: &BoundingBox,
    width: u32,
    height: u32,
    padding: u32,
) -> Option<BoundingBox> {
    let right = bounds.x.checked_add(bounds.width)?;
    let bottom = bounds.y.checked_add(bounds.height)?;
    if bounds.width == 0 || bounds.height == 0 || right > width || bottom > height {
        return None;
    }
    let x = bounds.x.saturating_sub(padding);
    let y = bounds.y.saturating_sub(padding);
    Some(BoundingBox {
        x,
        y,
        width: right.saturating_add(padding).min(width) - x,
        height: bottom.saturating_add(padding).min(height) - y,
    })
}

const MAX_REGION_CANDIDATES: usize = 4096;
const MAX_REGION_CROPS: usize = 16;
const MAX_REGION_OUTPUT_PIXELS: u64 = 16_000_000;

/// Limits for returned images, not peak memory or model tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRegionOptions {
    pub overview_max_edge: u32,
    pub max_crops: usize,
    pub max_output_pixels: u64,
    pub padding: u32,
}

impl Default for ScreenRegionOptions {
    fn default() -> Self {
        Self {
            overview_max_edge: 512,
            max_crops: 4,
            max_output_pixels: 4_000_000,
            padding: 16,
        }
    }
}

/// One outcome per input candidate, in the original priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionDecision {
    Included { crop_index: usize },
    InvalidBounds,
    DuplicateOf { crop_index: usize },
    CropLimit,
    PixelBudget,
}

/// Global context and native crops prepared together from one borrowed frame.
/// Intentionally does not implement Debug or serialization for raw images.
pub struct PreparedScreenRegions {
    overview: ScreenOverview,
    crops: Vec<ScreenRegionCrop>,
    decisions: Vec<RegionDecision>,
    output_pixels: u64,
}

impl PreparedScreenRegions {
    #[must_use]
    pub fn source_dimensions(&self) -> (u32, u32) {
        self.overview.source_dimensions()
    }

    #[must_use]
    pub fn overview(&self) -> &RgbaImage {
        self.overview.image()
    }

    #[must_use]
    pub fn crops(&self) -> &[ScreenRegionCrop] {
        &self.crops
    }

    #[must_use]
    pub fn decisions(&self) -> &[RegionDecision] {
        &self.decisions
    }

    #[must_use]
    pub fn output_pixels(&self) -> u64 {
        self.output_pixels
    }
}

/// Prepare an overview and prioritized native crops under one output budget.
///
/// Candidates must already be ordered by priority and belong to this frame's
/// pixel space. The caller owns frame/policy identity and privacy checks.
/// The overview is always reserved first, even with no selected crops.
/// Invalid or unaffordable candidates do not hide later affordable candidates.
///
/// Bounds are validated before padding; only the added context is clamped.
/// Exact padded duplicates refer to an included crop, even after a limit is
/// reached. Nested and partially overlapping crops retain their distinct scale.
/// Each candidate receives an outcome, including in overview-only mode.
///
/// # Errors
///
/// Returns `CoreError::InvalidArguments` for more than 4096 candidates, an
/// invalid overview, more than 16 allowed crops, or an output budget outside
/// 1..=16,000,000 pixels. The complete overview must fit the output budget.
pub fn prepare_screen_regions(
    frame: &RgbaImage,
    candidates: &[BoundingBox],
    options: ScreenRegionOptions,
) -> Result<PreparedScreenRegions, CoreError> {
    if candidates.len() > MAX_REGION_CANDIDATES {
        return Err(invalid_arguments(
            "screen region candidate count exceeds limit",
        ));
    }
    if options.max_crops > MAX_REGION_CROPS
        || options.max_output_pixels == 0
        || options.max_output_pixels > MAX_REGION_OUTPUT_PIXELS
    {
        return Err(invalid_arguments("screen region options exceed limits"));
    }
    let overview = prepare_screen_overview(
        frame,
        options.overview_max_edge,
        MAX_OVERVIEW_SOURCE_PIXELS,
        options.max_output_pixels,
    )?;
    let mut output_pixels =
        u64::from(overview.image().width()) * u64::from(overview.image().height());
    let mut crops: Vec<ScreenRegionCrop> = Vec::with_capacity(options.max_crops);
    let mut decisions = Vec::with_capacity(candidates.len());
    for (candidate_index, candidate) in candidates.iter().enumerate() {
        let Some(bounds) =
            padded_region_bounds(candidate, frame.width(), frame.height(), options.padding)
        else {
            decisions.push(RegionDecision::InvalidBounds);
            continue;
        };
        if let Some(crop_index) = crops
            .iter()
            .position(|crop| crop.source_bounds() == &bounds)
        {
            decisions.push(RegionDecision::DuplicateOf { crop_index });
            continue;
        }
        if crops.len() >= options.max_crops {
            decisions.push(RegionDecision::CropLimit);
            continue;
        }
        let remaining_pixels = options.max_output_pixels - output_pixels;
        if bounds.area() > remaining_pixels {
            decisions.push(RegionDecision::PixelBudget);
            continue;
        }
        let crop = prepare_screen_region(frame, &bounds, candidate_index, remaining_pixels)?;
        output_pixels += bounds.area();
        decisions.push(RegionDecision::Included {
            crop_index: crops.len(),
        });
        crops.push(crop);
    }
    Ok(PreparedScreenRegions {
        overview,
        crops,
        decisions,
        output_pixels,
    })
}
