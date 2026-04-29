use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use image::{DynamicImage, Rgba, RgbaImage};
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::frame::BoundingBox;
use oneshim_vision::gui_detector::GuiElementDetector;
use oneshim_vision::{delta, encoder, encoder::WebPQuality, thumbnail};

fn create_test_image(width: u32, height: u32, seed: u8) -> DynamicImage {
    let mut img = RgbaImage::new(width, height);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = (x as u8).wrapping_add(seed).wrapping_mul(17);
        let g = (y as u8).wrapping_add(seed).wrapping_mul(31);
        let b = (x as u8).wrapping_add(y as u8).wrapping_add(seed);
        *pixel = Rgba([r, g, b, 255]);
    }
    DynamicImage::ImageRgba8(img)
}

fn create_modified_image(base: &DynamicImage, change_ratio: f32) -> DynamicImage {
    let mut img = base.to_rgba8();
    let (w, h) = img.dimensions();
    let change_width = (w as f32 * change_ratio.sqrt()) as u32;
    let change_height = (h as f32 * change_ratio.sqrt()) as u32;

    for y in 0..change_height.min(h) {
        for x in 0..change_width.min(w) {
            let pixel = img.get_pixel_mut(x, y);
            pixel[0] = pixel[0].wrapping_add(50);
            pixel[1] = pixel[1].wrapping_add(30);
        }
    }
    DynamicImage::ImageRgba8(img)
}

fn bench_delta(c: &mut Criterion) {
    let mut group = c.benchmark_group("delta_encoding");

    let resolutions = [(640, 480), (1280, 720), (1920, 1080)];

    for (width, height) in resolutions {
        let pixels = width * height;
        group.throughput(Throughput::Elements(pixels as u64));

        let prev = create_test_image(width, height, 42);
        let curr_10 = create_modified_image(&prev, 0.10); // 10% change
        let curr_50 = create_modified_image(&prev, 0.50); // 50% change
        group.bench_with_input(
            BenchmarkId::new("10%_change", format!("{}x{}", width, height)),
            &(&prev, &curr_10),
            |b, (prev, curr)| {
                b.iter(|| black_box(delta::compute_delta(prev, curr)));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("50%_change", format!("{}x{}", width, height)),
            &(&prev, &curr_50),
            |b, (prev, curr)| {
                b.iter(|| black_box(delta::compute_delta(prev, curr)));
            },
        );
    }

    group.finish();
}

fn bench_thumbnail(c: &mut Criterion) {
    let mut group = c.benchmark_group("thumbnail_resize");

    let sources = [(1920, 1080), (2560, 1440), (3840, 2160)];
    let target = (480, 270); // default size
    for (src_w, src_h) in sources {
        let pixels = src_w * src_h;
        group.throughput(Throughput::Elements(pixels as u64));

        let img = create_test_image(src_w, src_h, 123);

        group.bench_with_input(
            BenchmarkId::new("resize", format!("{}x{}", src_w, src_h)),
            &img,
            |b, img| {
                b.iter(|| black_box(thumbnail::fast_resize(img, target.0, target.1)));
            },
        );

        let _ = thumbnail::fast_resize(&img, target.0, target.1);
        group.bench_with_input(
            BenchmarkId::new("cached", format!("{}x{}", src_w, src_h)),
            &img,
            |b, img| {
                b.iter(|| black_box(thumbnail::fast_resize(img, target.0, target.1)));
            },
        );
    }

    group.finish();
}

fn bench_webp_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("webp_encoding");

    let resolutions = [(640, 480), (1280, 720), (1920, 1080)];
    let qualities = [
        ("low", WebPQuality::Low),
        ("medium", WebPQuality::Medium),
        ("high", WebPQuality::High),
    ];

    for (width, height) in resolutions {
        let pixels = width * height;
        let img = create_test_image(width, height, 77);

        for (quality_name, quality) in &qualities {
            group.throughput(Throughput::Elements(pixels as u64));

            group.bench_with_input(
                BenchmarkId::new(*quality_name, format!("{}x{}", width, height)),
                &(&img, *quality),
                |b, (img, quality)| {
                    b.iter(|| black_box(encoder::encode_webp(img, *quality)));
                },
            );
        }
    }

    group.finish();
}

fn bench_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_pipeline");

    let (width, height) = (1920, 1080);
    let prev = create_test_image(width, height, 1);
    let curr = create_modified_image(&prev, 0.20);

    group.throughput(Throughput::Elements((width * height) as u64));

    group.bench_function("delta_thumbnail_webp", |b| {
        b.iter(|| {
            let _delta = delta::compute_delta(&prev, &curr);

            let thumb = thumbnail::fast_resize(&curr, 480, 270).unwrap();

            let _encoded = encoder::encode_webp(&thumb, WebPQuality::Medium);
        });
    });

    group.finish();
}

fn bench_gui_inference(c: &mut Criterion) {
    let mut group = c.benchmark_group("gui_inference");
    let detector = GuiElementDetector::new((1920, 1080), PiiFilterLevel::Off);

    let cases = vec![
        (
            "title_bar",
            "My App Window",
            BoundingBox {
                x: 100,
                y: 10,
                width: 200,
                height: 20,
            },
        ),
        (
            "button",
            "Save",
            BoundingBox {
                x: 500,
                y: 500,
                width: 60,
                height: 30,
            },
        ),
        (
            "link",
            "https://example.com/path",
            BoundingBox {
                x: 200,
                y: 400,
                width: 300,
                height: 20,
            },
        ),
        (
            "menu_item",
            "File  Ctrl+S",
            BoundingBox {
                x: 50,
                y: 200,
                width: 150,
                height: 25,
            },
        ),
        (
            "text_region",
            "This is a longer text block with multiple words",
            BoundingBox {
                x: 100,
                y: 300,
                width: 400,
                height: 100,
            },
        ),
        (
            "unknown",
            "X",
            BoundingBox {
                x: 960,
                y: 540,
                width: 20,
                height: 20,
            },
        ),
    ];

    for (name, text, bbox) in &cases {
        group.bench_with_input(
            BenchmarkId::new("scored", *name),
            &(text, bbox),
            |b, (text, bbox)| {
                b.iter(|| black_box(detector.infer_element_type_scored(text, bbox)));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_delta,
    bench_thumbnail,
    bench_webp_encode,
    bench_pipeline,
    bench_gui_inference
);
criterion_main!(benches);
