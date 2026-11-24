//! Screen-space proposals from color-connected components and their outer edges.
//! A proposal is geometry. Its score is a shape heuristic, and conveys no actionability.
use std::cmp::Reverse;
use std::io::Cursor;

use image::{ImageReader, RgbaImage};
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::NormalizedBounds;
use maekon_core::ports::rectangle_detector::{DetectedRectangle, RectangleDetector};

pub struct PixelRegionDetector;

const PIXEL_LIMIT: u64 = 32 * 1024 * 1024;
const ANALYSIS_LONG_EDGE: u32 = 1600;
const MAX_COLOR_DELTA: u8 = 8;

fn invalid(message: impl ToString) -> CoreError {
    CoreError::Internal {
        code: InternalCode::Generic,
        message: message.to_string(),
    }
}

#[derive(Clone, Copy)]
struct Box2d {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

impl Box2d {
    fn width(self) -> usize {
        self.right - self.left
    }
    fn height(self) -> usize {
        self.bottom - self.top
    }
    fn area(self) -> usize {
        self.width() * self.height()
    }
    fn can_contain_compound(self, minimum: usize) -> bool {
        let short = self.height().min(self.width());
        let long = self.height().max(self.width());
        short >= minimum.max(6) && short <= 64 && long >= 3 * short
    }

    fn with_interaction_margin(self, width: usize, height: usize) -> Self {
        let short = self.height().min(self.width());
        let (dx, dy) = if self.width() >= self.height() {
            (short / 4, short / 2)
        } else {
            (short / 2, short / 4)
        };
        Self {
            left: self.left.saturating_sub(dx),
            right: (self.right + dx).min(width),
            top: self.top.saturating_sub(dy),
            bottom: (self.bottom + dy).min(height),
        }
    }

    fn distinct_from(self, proposals: &[Proposal], threshold: f64) -> bool {
        proposals.iter().all(|p| p.bounds.overlap(self) < threshold)
    }

    fn has_end_indicator(self, icon: Self, short: usize) -> bool {
        let margin = (short / 4).max(2);
        icon.width().abs_diff(icon.height()) <= margin
            && icon.height().abs_diff(short) <= margin
            && icon.top.abs_diff(self.top) <= margin
            && (icon.left.abs_diff(self.left) <= margin
                || icon.right.abs_diff(self.right) <= margin)
    }

    fn overlap(self, b: Self) -> f64 {
        let w = self
            .right
            .min(b.right)
            .saturating_sub(self.left.max(b.left));
        let h = self
            .bottom
            .min(b.bottom)
            .saturating_sub(self.top.max(b.top));
        let intersection = w * h;
        intersection as f64 / (self.area() + b.area() - intersection) as f64
    }
}

// Join internal gaps between foreground pixels on a single text row.
fn bridge_ink_gaps(row: &mut [u32], max_gap: usize) {
    let mut last_ink = None;
    for x in 0..row.len() {
        if row[x] == 0 {
            if let Some(last) = last_ink.filter(|last| x - last <= max_gap + 1) {
                // Both endpoints are already ink. Include the first in the fill.
                row[last..x].fill(0);
            }
            last_ink = Some(x);
        }
    }
}

struct Proposal {
    bounds: Box2d,
    kind: &'static str,
    score: f64,
}

#[derive(Debug, Default, PartialEq)]
struct InkProfile {
    rail_columns: usize,
    tall_start: Option<usize>,
    tall_end: usize,
    tall_count: usize,
    ink_runs: usize,
}

impl InkProfile {
    fn describes_track(&self, short: usize, long: usize) -> bool {
        self.tall_start.is_some_and(|start| {
            let thumb = self.tall_end - start;
            self.rail_columns * 3 >= long * 2
                && thumb >= short / 3
                && thumb <= 2 * short
                && self.tall_count * 4 >= thumb * 3
        })
    }
}

struct Plane {
    width: usize,
    height: usize,
    step: u32,
    colors: Vec<[u8; 3]>,
}

impl Plane {
    fn from_frame(image: &RgbaImage) -> Self {
        let step = image
            .width()
            .max(image.height())
            .div_ceil(ANALYSIS_LONG_EDGE);
        let width = image.width().div_ceil(step) as usize;
        let height = image.height().div_ceil(step) as usize;
        let mut colors = Vec::with_capacity(width * height);
        for y in (0..image.height()).step_by(step as usize) {
            for x in (0..image.width()).step_by(step as usize) {
                let [r, g, b, a] = image.get_pixel(x, y).0;
                // Composite on white. Hidden RGB in a transparent PNG cannot generate a target.
                let visible = |v: u8| 255 - ((255 - u16::from(v)) * u16::from(a) / 255) as u8;
                colors.push([visible(r), visible(g), visible(b)]);
            }
        }
        Self {
            width,
            height,
            step,
            colors,
        }
    }

    fn similar(&self, index: usize, color: [u8; 3]) -> bool {
        let [r, g, b] = self.colors[index];
        r.abs_diff(color[0])
            .max(g.abs_diff(color[1]))
            .max(b.abs_diff(color[2]))
            <= MAX_COLOR_DELTA
    }

    fn component(
        &self,
        seed: usize,
        labels: &mut [u32],
        pixels: &mut Vec<usize>,
        matches: impl Fn(usize) -> bool,
    ) -> Box2d {
        pixels.clear();
        pixels.push(seed);
        let id = seed as u32 + 1;
        labels[seed] = id;
        let mut bounds = Box2d {
            left: self.width,
            right: 0,
            top: self.height,
            bottom: 0,
        };
        // A labelled pixel enters the queue once, so the frame area bounds the traversal.
        for head in 0..labels.len() {
            let Some(&index) = pixels.get(head) else {
                break;
            };
            let x = index % self.width;
            let y = index / self.width;
            bounds.left = bounds.left.min(x);
            bounds.right = bounds.right.max(x + 1);
            bounds.top = bounds.top.min(y);
            bounds.bottom = bounds.bottom.max(y + 1);
            let mut visit = |other: usize| {
                if labels[other] == 0 && matches(other) {
                    labels[other] = id;
                    pixels.push(other);
                }
            };
            if x > 0 {
                visit(index - 1);
            }
            if x + 1 < self.width {
                visit(index + 1);
            }
            if y > 0 {
                visit(index - self.width);
            }
            if y + 1 < self.height {
                visit(index + self.width);
            }
            debug_assert!(
                pixels.len() <= labels.len(),
                "a component cannot contain more pixels than its frame"
            );
        }
        bounds
    }

    fn outline(&self, pixels: &[usize], labels: &[u32], b: Box2d) -> ([usize; 4], usize) {
        let id = labels[pixels[0]];
        let mut sides = [0usize; 4];
        let mut perimeter = 0usize;
        for &i in pixels {
            let x = i % self.width;
            let y = i / self.width;
            sides[0] += usize::from(y == b.top);
            sides[1] += usize::from(y + 1 == b.bottom);
            sides[2] += usize::from(x == b.left);
            sides[3] += usize::from(x + 1 == b.right);
            // Four-neighbor raster boundary, including edges of holes.
            perimeter += usize::from(x == 0 || labels[i - 1] != id);
            perimeter += usize::from(x + 1 == self.width || labels[i + 1] != id);
            perimeter += usize::from(y == 0 || labels[i - self.width] != id);
            perimeter += usize::from(y + 1 == self.height || labels[i + self.width] != id);
        }
        (sides, perimeter)
    }

    fn proposal(
        &self,
        pixels: &[usize],
        labels: &[u32],
        b: Box2d,
        minimum: usize,
    ) -> Option<Proposal> {
        let w = b.width();
        let h = b.height();
        if w.min(h) < minimum || (w == self.width && h == self.height) {
            return None;
        }
        let (sides, perimeter) = self.outline(pixels, labels, b);
        let edge = (sides[0].min(sides[1]) as f64 / w as f64)
            .min(sides[2].min(sides[3]) as f64 / h as f64);
        let fill = pixels.len() as f64 / b.area() as f64;
        if edge >= 0.40 && w.max(h) <= 38 * w.min(h) {
            return Some(Proposal {
                bounds: b,
                kind: "rectangular-surface",
                score: 0.76 + 0.22 * edge,
            });
        }
        let aspect = w.min(h) as f64 / w.max(h) as f64;
        let compactness = perimeter as f64 * perimeter as f64 / pixels.len() as f64;
        if w.min(h) * self.step as usize >= 20
            && w.max(h) * self.step as usize <= 128
            && aspect >= 0.55
            && (0.23..=0.9).contains(&fill)
            && (10.0..=65.0).contains(&compactness)
        {
            return Some(Proposal {
                bounds: b,
                kind: "compact-shape",
                score: 0.50 + 0.18 * aspect,
            });
        }
        None
    }

    fn ink_profile(&self, labels: &[u32], surface: u32, b: Box2d) -> InkProfile {
        let short = b.width().min(b.height());
        let long = b.width().max(b.height());
        let horizontal = b.width() >= b.height();
        let mut profile = InkProfile::default();
        let mut previous_ink = false;
        for along in 0..long {
            let mut low = short;
            let mut high = 0;
            for across in 0..short {
                let (x, y) = if horizontal {
                    (along, across)
                } else {
                    (across, along)
                };
                if labels[(b.top + y) * self.width + b.left + x] != surface {
                    low = low.min(across);
                    high = across + 1;
                }
            }
            let ink = high != 0;
            profile.ink_runs += usize::from(ink && !previous_ink);
            previous_ink = ink;
            let span = high.saturating_sub(low);
            if ink && span <= (short / 3).max(3) && low <= short / 2 + 1 && high + 1 >= short / 2 {
                profile.rail_columns += 1;
            }
            if span * 3 >= short * 2 {
                profile.tall_start.get_or_insert(along);
                profile.tall_end = along + 1;
                profile.tall_count += 1;
            }
        }

        profile
    }

    fn composite_kind(
        &self,
        labels: &[u32],
        surface: u32,
        b: Box2d,
        outlines: &[Proposal],
    ) -> Option<&'static str> {
        let short = b.width().min(b.height());
        let long = b.width().max(b.height());
        let horizontal = b.width() >= b.height();
        let profile = self.ink_profile(labels, surface, b);
        if profile.describes_track(short, long) {
            return Some("track-and-handle");
        }
        // The label must be adjacent to an independently outlined, square
        // indicator near one end. A row of text on its own is insufficient.
        (horizontal
            && profile.ink_runs >= 3
            && outlines
                .iter()
                .any(|p| b.has_end_indicator(p.bounds, short)))
        .then_some("indicator-and-label")
    }

    // On a large flat surface, multicolored ink can describe one control even
    // when no single paint color outlines it. Join short gaps on text rows.
    fn composite_proposals(
        &self,
        labels: &[u32],
        surface: u32,
        minimum: usize,
        limit: usize,
        proposals: &mut Vec<Proposal>,
    ) {
        let max_gap = minimum.clamp(4, 12);
        let mut visited: Vec<u32> = labels.iter().map(|&id| u32::from(id == surface)).collect();
        for row in visited.chunks_exact_mut(self.width) {
            bridge_ink_gaps(row, max_gap);
        }

        let mut pixels = Vec::new();
        let initial = proposals.len();
        for i in 0..visited.len() {
            if visited[i] != 0 {
                continue;
            }
            let b = self.component(i, &mut visited, &mut pixels, |_| true);
            if !b.can_contain_compound(minimum) {
                continue;
            }
            let Some(kind) = self.composite_kind(labels, surface, b, &proposals[..initial]) else {
                continue;
            };
            let bounds = b.with_interaction_margin(self.width, self.height);
            if bounds.distinct_from(proposals, 0.65) {
                proposals.push(Proposal {
                    bounds,
                    kind,
                    score: 0.62,
                });
                if proposals.len() == limit {
                    return;
                }
            }
        }
    }

    fn detect(&self, min_pixels: u32, limit: usize) -> Vec<Proposal> {
        let minimum = min_pixels.div_ceil(self.step) as usize;
        let mut labels = vec![0; self.colors.len()];
        let mut pixels = Vec::new();
        let mut proposals = Vec::new();
        let mut largest_count = 0;
        let mut surface = 0;
        for index in 0..self.colors.len() {
            if labels[index] != 0 {
                continue;
            }
            let color = self.colors[index];
            let bounds = self.component(index, &mut labels, &mut pixels, |other| {
                self.similar(other, color)
            });
            if pixels.len() > largest_count {
                largest_count = pixels.len();
                surface = labels[index];
            }
            if let Some(proposal) = self.proposal(&pixels, &labels, bounds, minimum) {
                proposals.push(proposal);
            }
        }
        let mut distinct = distinct_proposals(proposals, limit);
        if distinct.len() < limit && usable_surface(largest_count, labels.len()) {
            self.composite_proposals(&labels, surface, minimum, limit, &mut distinct);
        }
        distinct
    }
}

// Ignore a uniform frame and fragmented backgrounds with less than one eighth of the area.
fn usable_surface(largest_count: usize, frame_pixels: usize) -> bool {
    (frame_pixels.div_ceil(8)..frame_pixels).contains(&largest_count)
}

fn distinct_proposals(mut proposals: Vec<Proposal>, limit: usize) -> Vec<Proposal> {
    // Prefer the full outline to its slightly inset fill when they share a boundary.
    // Larger panels and smaller controls both survive when they have distinct edges.
    proposals.sort_by(|a, b| {
        let rank = |p: &Proposal| {
            (
                p.kind == "compact-shape",
                Reverse(p.bounds.area()),
                p.bounds.top,
                p.bounds.left,
            )
        };
        rank(a).cmp(&rank(b))
    });
    let mut distinct: Vec<Proposal> = Vec::new();
    for p in proposals {
        if !p.bounds.distinct_from(&distinct, 0.78) {
            continue;
        }
        distinct.push(p);
        if distinct.len() == limit {
            break;
        }
    }
    distinct
}

impl RectangleDetector for PixelRegionDetector {
    fn detect_rectangles(
        &self,
        bytes: &[u8],
        width: u32,
        height: u32,
        min_size: f32,
        max_results: usize,
    ) -> Result<Vec<DetectedRectangle>, CoreError> {
        if width == 0
            || height == 0
            || u64::from(width) * u64::from(height) > PIXEL_LIMIT
            || !min_size.is_finite()
            || !(0.0..=1.0).contains(&min_size)
        {
            return Err(invalid("Invalid dimensions or minimum region size"));
        }
        let reader = || {
            ImageReader::new(Cursor::new(bytes))
                .with_guessed_format()
                .map_err(invalid)
        };
        if reader()?.into_dimensions().map_err(invalid)? != (width, height) {
            return Err(invalid("Image dimensions differ from capture metadata"));
        }
        if max_results == 0 {
            return Ok(Vec::new());
        }
        let image = reader()?.decode().map_err(invalid)?.into_rgba8();
        let plane = Plane::from_frame(&image);
        drop(image);
        let min_pixels = ((min_size * width.min(height) as f32).ceil() as u32).max(10);
        Ok(plane
            .detect(min_pixels, max_results)
            .into_iter()
            .map(|proposal| {
                let b = proposal.bounds;
                let x = b.left as u32 * plane.step;
                let y = b.top as u32 * plane.step;
                let w = (b.right as u32 * plane.step).min(width) - x;
                let h = (b.bottom as u32 * plane.step).min(height) - y;
                DetectedRectangle {
                    bounds: ElementBounds {
                        x: x as i32,
                        y: y as i32,
                        width: w,
                        height: h,
                    },
                    bounds_normalized: NormalizedBounds::new(
                        x as f32 / width as f32,
                        y as f32 / height as f32,
                        w as f32 / width as f32,
                        h as f32 / height as f32,
                    ),
                    confidence: proposal.score,
                    classification: Some(proposal.kind.to_string()),
                }
            })
            .collect())
    }

    fn provider_name(&self) -> &str {
        "pixel-contours"
    }
}

#[cfg(test)]
mod tests;
