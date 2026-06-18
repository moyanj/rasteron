use core::hint::black_box;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rasteron::Renderer;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;

fn make_renderer(
    width: usize,
    height: usize,
    format: PixelFormat,
) -> (Renderer<'static>, Box<[u8]>) {
    let pixel_size = format.bytes_per_pixel();
    let pitch = width * pixel_size;
    let info = FramebufferInfo::new(width, height, pitch, (pixel_size * 8) as u16, format);
    let mut front = vec![0u8; info.buffer_size()].into_boxed_slice();
    let ptr = front.as_mut_ptr();
    let len = front.len();
    let front_slice = unsafe { core::slice::from_raw_parts_mut(ptr, len) };
    let renderer = Renderer::new(info, front_slice);
    (renderer, front)
}

fn bench_set_pixel_opaque(c: &mut Criterion) {
    let mut group = c.benchmark_group("set_pixel_opaque");

    for format in [
        PixelFormat::Bgr24,
        PixelFormat::Xrgb8888,
        PixelFormat::Rgba8888,
    ] {
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
                        black_box(Color::with_alpha(0x12, 0x34, 0x56, 0xFF)),
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

    for format in [
        PixelFormat::Bgr24,
        PixelFormat::Xrgb8888,
        PixelFormat::Rgba8888,
    ] {
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
                        black_box(Color::with_alpha(0x90, 0x70, 0x50, 0x80)),
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

    for format in [
        PixelFormat::Bgr24,
        PixelFormat::Xrgb8888,
        PixelFormat::Rgba8888,
    ] {
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
                            renderer.set_pixel(x, y, black_box(Color::with_alpha(r, g, b, 0xFF)));
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_set_pixel_opaque,
    bench_set_pixel_alpha,
    bench_frame_gradient
);
criterion_main!(benches);
