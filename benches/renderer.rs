use core::hint::black_box;
use core::time::Duration;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rasteron::Renderer;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;

const BENCH_FORMATS: [PixelFormat; 1] = [PixelFormat::Bgr24];

fn criterion_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(4))
        .measurement_time(Duration::from_secs(10))
        .sample_size(500)
        .noise_threshold(0.025)
        .significance_level(0.01)
}

fn make_renderer(
    width: usize,
    height: usize,
    format: PixelFormat,
) -> (Renderer<'static>, Box<[u8]>) {
    let pixel_size = format.bytes_per_pixel();
    let pitch = width * pixel_size;
    let info = FramebufferInfo::new(width, height, pitch, format);
    let mut front = vec![0u8; info.buffer_size()].into_boxed_slice();
    let ptr = front.as_mut_ptr();
    let len = front.len();
    let front_slice = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    let renderer = Renderer::new(info, front_slice);
    (renderer, front)
}

fn bench_set_pixel_opaque(c: &mut Criterion) {
    let mut group = c.benchmark_group("set_pixel_opaque");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(640, 480, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut x = 0usize;
                let mut y = 0usize;
                b.iter(|| {
                    renderer.set_pixel(
                        black_box(x),
                        black_box(y),
                        black_box(&Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    x += 1;
                    if x == 640 {
                        x = 0;
                        y = (y + 1) % 480;
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_set_pixel_alpha(c: &mut Criterion) {
    let mut group = c.benchmark_group("set_pixel_alpha");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(640, 480, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut x = 0usize;
                let mut y = 0usize;
                b.iter(|| {
                    renderer.set_pixel(
                        black_box(x),
                        black_box(y),
                        black_box(&Color::with_alpha(0x90, 0x70, 0x50, 0x80)),
                    );
                    x += 1;
                    if x == 640 {
                        x = 0;
                        y = (y + 1) % 480;
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_frame_gradient(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let pixels = (width * height) as u64;
    let mut group = c.benchmark_group("frame_gradient");
    group.throughput(Throughput::Elements(pixels));

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                b.iter(|| {
                    for y in 0..height {
                        for x in 0..width {
                            let r = ((x * 255) / (width - 1)) as u8;
                            let g = ((y * 255) / (height - 1)) as u8;
                            let b = 255u8.saturating_sub(
                                ((x + y) * 255 / ((width - 1) + (height - 1))) as u8,
                            );
                            renderer.set_pixel(x, y, black_box(&Color::with_alpha(r, g, b, 0xFF)));
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_clear(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let pixels = (width * height) as u64;
    let mut group = c.benchmark_group("clear");
    group.throughput(Throughput::Elements(pixels));

    let (mut renderer, _front) = make_renderer(width, height, PixelFormat::Xrgb8888);
    group.bench_with_input(
        BenchmarkId::from_parameter("Xrgb8888"),
        &PixelFormat::Xrgb8888,
        |b, _| {
            b.iter(|| {
                renderer.clear(black_box(Color::with_alpha(0x12, 0x34, 0x56, 0xFF)));
            });
        },
    );

    group.finish();
}

fn bench_draw_line(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("draw_line");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut y = 0usize;
                b.iter(|| {
                    let start = Pixel::new(0, y);
                    let end = Pixel::new(width - 1, y + 16);
                    unsafe {
                        renderer.draw_line_unchecked(
                            black_box(start),
                            black_box(end),
                            black_box(&Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                        );
                    }
                    y = (y + 1) % (height - 16);
                });
            },
        );
    }

    group.finish();
}

fn bench_draw_triangle(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("draw_triangle");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut y = 0usize;
                b.iter(|| {
                    let p1 = Pixel::new(width / 2, y);
                    let p2 = Pixel::new(0, (y + 64).min(height - 1));
                    let p3 = Pixel::new(width - 1, (y + 64).min(height - 1));
                    renderer.draw_triangle(
                        black_box(p1),
                        black_box(p2),
                        black_box(p3),
                        black_box(&Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    y = (y + 16) % (height - 64);
                });
            },
        );
    }

    group.finish();
}

fn bench_fill_triangle(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("fill_triangle");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut y = 0usize;
                b.iter(|| {
                    let p1 = Pixel::new(width / 2, y);
                    let p2 = Pixel::new(0, (y + 64).min(height - 1));
                    let p3 = Pixel::new(width - 1, (y + 64).min(height - 1));
                    renderer.fill_triangle(
                        black_box(p1),
                        black_box(p2),
                        black_box(p3),
                        black_box(&Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    y = (y + 16) % (height - 64);
                });
            },
        );
    }

    group.finish();
}

fn bench_draw_rect(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("draw_rect");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut y = 0usize;
                b.iter(|| {
                    renderer.draw_rect(
                        black_box(Pixel::new(16, y)),
                        black_box(width - 32),
                        black_box(64usize),
                        black_box(&Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    y = (y + 16) % (height - 64);
                });
            },
        );
    }

    group.finish();
}

fn bench_fill_rect(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("fill_rect");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut y = 0usize;
                b.iter(|| {
                    renderer.fill_rect(
                        black_box(Pixel::new(16, y)),
                        black_box(width - 32),
                        black_box(64usize),
                        black_box(Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    y = (y + 16) % (height - 64);
                });
            },
        );
    }

    group.finish();
}

fn bench_draw_circle(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("draw_circle");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut radius = 8usize;
                b.iter(|| {
                    renderer.draw_circle(
                        black_box(Pixel::new(width / 2, height / 2)),
                        black_box(radius),
                        black_box(&Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    radius = (radius + 4) % 128 + 8;
                });
            },
        );
    }

    group.finish();
}

fn bench_fill_circle(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("fill_circle");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut radius = 8usize;
                b.iter(|| {
                    renderer.fill_circle(
                        black_box(Pixel::new(width / 2, height / 2)),
                        black_box(radius),
                        black_box(Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    radius = (radius + 4) % 128 + 8;
                });
            },
        );
    }

    group.finish();
}

fn bench_fill_rounded_rect(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("fill_rounded_rect");

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut x = 16usize;
                b.iter(|| {
                    renderer.fill_rounded_rect(
                        black_box(Pixel::new(x, 32)),
                        black_box(width - 48),
                        black_box(96usize),
                        black_box(16usize),
                        black_box(Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    x = (x + 32) % 128 + 16;
                });
            },
        );
    }

    group.finish();
}

fn bench_blit(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("blit");
    group.throughput(Throughput::Elements(64 * 64));

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        let bpp = format.bytes_per_pixel();
        let src_w = 64usize;
        let src_h = 64usize;
        let src_pitch = src_w * bpp;
        let src = vec![0xABu8; src_pitch * src_h];
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut x = 0usize;
                b.iter(|| {
                    renderer.blit(
                        black_box(Pixel::new(x, 32)),
                        black_box(src_w),
                        black_box(src_h),
                        black_box(format),
                        black_box(&src),
                        black_box(src_pitch),
                    );
                    x = (x + 32) % (width - src_w - 16) + 8;
                });
            },
        );
    }

    group.finish();
}

fn bench_blit_mask(c: &mut Criterion) {
    let width = 640usize;
    let height = 360usize;
    let mut group = c.benchmark_group("blit_mask");
    group.throughput(Throughput::Elements(64 * 64));

    for format in BENCH_FORMATS {
        let (mut renderer, _front) = make_renderer(width, height, format);
        let src_w = 64usize;
        let src_h = 64usize;
        let mask_pitch = src_w;
        let mask = vec![128u8; mask_pitch * src_h];
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{format:?}")),
            &format,
            |b, _| {
                let mut x = 0usize;
                b.iter(|| {
                    renderer.blit_mask(
                        black_box(Pixel::new(x, 32)),
                        black_box(src_w),
                        black_box(src_h),
                        black_box(&mask),
                        black_box(mask_pitch),
                        black_box(Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
                    );
                    x = (x + 32) % (width - src_w - 16) + 8;
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    name = benches;
    config = criterion_config();
    targets =
    bench_set_pixel_opaque,
    bench_set_pixel_alpha,
    bench_frame_gradient,
    bench_clear,
    bench_draw_line,
    bench_draw_triangle,
    bench_fill_triangle,
    bench_draw_rect,
    bench_fill_rect,
    bench_draw_circle,
    bench_fill_circle,
    bench_fill_rounded_rect,
    bench_blit,
    bench_blit_mask
);
criterion_main!(benches);
