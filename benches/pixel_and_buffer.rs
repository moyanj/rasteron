//! 像素读写、清屏、双缓冲呈现、跨度填充与子视口基准。
//!
//! 覆盖的公开面（`src/core.rs`）：
//!
//! * `set_pixel` / `set_pixel_at` / `get_pixel` / `get_pixel_at` / `pixel_offset`
//! * `clear`（含带行填充的布局）
//! * `swap_buffers`（双缓冲呈现）
//! * `blit`（同格式块拷贝）
//! * `utils::sub_view` / `sub_view_with_back_buffer`
//!
//! 这些是所有图元的共同底座：`set_pixel` 走的是 `write_color_at_offset`
//! 这条被 `Renderer` 用 `fmt_*` 缓存加速过的路径，它的成本决定了
//! 「逐像素写入」类图元的下限。

mod shared;

use core::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use rasteron::Pixel;
use rasteron::color::PixelFormat;
use rasteron::framebuffer::FramebufferInfo;
use rasteron::utils::{sub_view, sub_view_with_back_buffer};

use shared::{
    ALL_FORMATS, CORE_FORMATS, OPAQUE, SIZE_HD, SIZE_SD, TRANSLUCENT, PixFb, criterion_config,
    gradient_texture,
};

/// 逐像素写入：不透明色走「直接写」快速路径。
///
/// 坐标用递增计数器扫过整个帧缓冲，避免同一像素反复命中缓存造成
/// 对真实工作负载的乐观估计。
fn bench_set_pixel_opaque(c: &mut Criterion) {
    let mut group = c.benchmark_group("set_pixel_opaque");

    for format in CORE_FORMATS {
        let (width, height) = SIZE_SD;
        let mut fb = PixFb::new(width, height, format);
        group.bench_with_input(BenchmarkId::from_parameter(format!("{format:?}")), &format, |b, _| {
            let mut x = 0usize;
            let mut y = 0usize;
            b.iter(|| {
                fb.renderer.set_pixel(black_box(x), black_box(y), &OPAQUE);
                x += 1;
                if x >= width {
                    x = 0;
                    y += 1;
                    if y >= height {
                        y = 0;
                    }
                }
            });
        });
    }

    group.finish();
}

/// 逐像素写入：半透明色强制走混合路径（读-改-写）。
///
/// 与 [`bench_set_pixel_opaque`] 的差值就是混合本身的成本，
/// 也是 SWAR 内核（4bpp 对齐时）与逐通道回退（3bpp）的分水岭。
fn bench_set_pixel_alpha(c: &mut Criterion) {
    let mut group = c.benchmark_group("set_pixel_alpha");

    for format in CORE_FORMATS {
        let (width, height) = SIZE_SD;
        let mut fb = PixFb::new(width, height, format);
        group.bench_with_input(BenchmarkId::from_parameter(format!("{format:?}")), &format, |b, _| {
            let mut x = 0usize;
            let mut y = 0usize;
            b.iter(|| {
                fb.renderer.set_pixel(black_box(x), black_box(y), &TRANSLUCENT);
                x += 1;
                if x >= width {
                    x = 0;
                    y += 1;
                    if y >= height {
                        y = 0;
                    }
                }
            });
        });
    }

    group.finish();
}

/// 像素读回：`get_pixel` 的边界检查 + 格式解码。
fn bench_get_pixel(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_pixel");

    for format in CORE_FORMATS {
        let (width, height) = SIZE_SD;
        // `get_pixel` 只读，故不需要可变绑定。
        let fb = PixFb::new(width, height, format);
        group.bench_with_input(BenchmarkId::from_parameter(format!("{format:?}")), &format, |b, _| {
            let mut x = 0usize;
            let mut y = 0usize;
            b.iter(|| {
                let pixel = fb.renderer.get_pixel(black_box(x), black_box(y));
                black_box(pixel);
                x += 7;
                if x >= width {
                    x = 0;
                    y += 3;
                    if y >= height {
                        y = 0;
                    }
                }
            });
        });
    }

    group.finish();
}

/// 坐标到字节偏移的换算（`pixel_offset`）。
///
/// 它是每次写入的前置算术，本身含溢出检查。
fn bench_pixel_offset(c: &mut Criterion) {
    let mut group = c.benchmark_group("pixel_offset");
    let (width, height) = SIZE_SD;
    let fb = PixFb::new(width, height, PixelFormat::Rgba8888);

    group.bench_function("rgba8888_640x480", |b| {
        let mut x = 0usize;
        let mut y = 0usize;
        b.iter(|| {
            let offset = fb.renderer.pixel_offset(black_box(x), black_box(y));
            black_box(offset);
            x += 1;
            if x >= width {
                x = 0;
                y = (y + 1) % height;
            }
        });
    });

    group.finish();
}

/// 清屏：`clear` 对整块帧缓冲的写入吞吐。
///
/// 两种尺寸用于确认成本随面积线性扩展；带行填充的布局单列一组，
/// 因为 `clear` 需要跳过行填充字节而不能整段 memcpy。
fn bench_clear(c: &mut Criterion) {
    let mut group = c.benchmark_group("clear");

    for (label, (width, height)) in [("640x480", SIZE_SD), ("1920x1080", SIZE_HD)] {
        let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
        let pixels = (width * height) as u64;
        group.throughput(Throughput::Elements(pixels));
        group.bench_with_input(BenchmarkId::new("rgba8888", label), &label, |b, _| {
            b.iter(|| fb.renderer.clear(black_box(OPAQUE)));
        });
    }

    // 带 64 字节行填充：pitch > row_len，clear 必须逐行拷贝可见部分。
    let (width, height) = SIZE_SD;
    let mut padded = PixFb::padded(width, height, PixelFormat::Rgba8888, 64);
    group.throughput(Throughput::Elements((width * height) as u64));
    group.bench_function("rgba8888_640x480_padded64", |b| {
        b.iter(|| padded.renderer.clear(black_box(OPAQUE)));
    });

    // 3bpp 对照组：行宽非 2 的幂，且无 SWAR 内核。
    let mut bgr = PixFb::new(width, height, PixelFormat::Bgr24);
    group.bench_function("bgr24_640x480", |b| {
        b.iter(|| bgr.renderer.clear(black_box(OPAQUE)));
    });

    group.finish();
}

/// 双缓冲呈现：`swap_buffers` 把后台可见像素拷到前台。
///
/// 测的是「逐行只拷 `width × bpp`」这条口径下的拷贝吞吐，
/// 与 `clear` 的差异反映额外的前后台两段缓冲的访存行为。
fn bench_swap_buffers(c: &mut Criterion) {
    let mut group = c.benchmark_group("swap_buffers");

    for (label, (width, height)) in [("640x480", SIZE_SD), ("1920x1080", SIZE_HD)] {
        let mut fb = PixFb::double(width, height, PixelFormat::Rgba8888);
        assert!(fb.renderer.is_double_buffered());

        // 先往后台写一个明确的非零值。否则后台全是 0，拷贝全 0 之后
        // 无法区分「真的呈现了」与「什么都没干」——一个测空操作的基准
        // 和一个正常基准的耗时看不出差别。
        fb.renderer.clear(OPAQUE);
        group.throughput(Throughput::Elements((width * height) as u64));
        group.bench_with_input(BenchmarkId::new("rgba8888", label), &label, |b, _| {
            b.iter(|| fb.renderer.swap_buffers());
        });

        // 呈现后前台应带上后台的内容：首像素的 alpha 为 Rgba8888 的
        // 不透明值 255。这既证明拷贝范围正确，也证明基准没测空操作。
        assert_eq!(
            fb.front_bytes()[3], 255,
            "swap_buffers 应把后台内容呈现到前台首像素"
        );
        // 末像素同样被覆盖，排除「只拷了第一行」这类残缺实现。
        assert_eq!(
            fb.front_bytes()[fb.front.len() - 1], 255,
            "swap_buffers 应覆盖到最后一行的最后一个像素"
        );
    }

    group.finish();
}

/// 同格式块拷贝：`blit` 的尺寸扫描。
///
/// 小尺寸落在 `simd::memcpy` 的 `len <= 128` 标量分支内，大尺寸进入 ISA 分支
/// （本机默认编译为 SSE2，`-C target-cpu=native` 下为 AVX2），
/// 因此这条扫描同时是 SIMD memcpy 阈值的行为探针。
fn bench_blit(c: &mut Criterion) {
    let mut group = c.benchmark_group("blit");
    let (fb_w, fb_h) = SIZE_HD;
    let mut fb = PixFb::new(fb_w, fb_h, PixelFormat::Rgba8888);

    for side in [8usize, 16, 32, 64, 128, 256] {
        let src = gradient_texture(side, side);
        let pitch = side * 4;
        group.throughput(Throughput::Elements((side * side) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(side), &side, |b, _| {
            b.iter(|| {
                fb.renderer.blit(
                    black_box(Pixel::new(0, 0)),
                    black_box(side),
                    black_box(side),
                    PixelFormat::Rgba8888,
                    black_box(&src),
                    black_box(pitch),
                );
            });
        });
    }

    // 带源行填充的拷贝：源 pitch 是 width × bpp 的两倍，逐行只读可见字节。
    //
    // 源缓冲必须真的容纳 `src_pitch × height`——`blit` 对该乘积做
    // 长度校验且**静默返回**，给少了会得到一个 7 ns 的「空操作」基准
    // （本基准初版就踩过这个坑）。
    let side = 64usize;
    let src_pitch = side * 2 * 4;
    let mut padded_src = vec![0u8; src_pitch * side];
    for (index, byte) in padded_src.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    group.throughput(Throughput::Elements((side * side) as u64));
    group.bench_function("64_padded_src", |b| {
        b.iter(|| {
            fb.renderer.blit(
                black_box(Pixel::new(0, 0)),
                black_box(side),
                black_box(side),
                PixelFormat::Rgba8888,
                black_box(&padded_src),
                black_box(src_pitch),
            );
        });
    });

    group.finish();
}

/// `blit` 的三类静默失败路径。
///
/// 它们返回得极快，但前提是「校验在前、写入在后」——把这条契约
/// 固定成基准，任何把校验挪到循环内部的重构都会让这里的数字暴涨。
fn bench_blit_rejected(c: &mut Criterion) {
    let mut group = c.benchmark_group("blit_rejected");
    let (fb_w, fb_h) = SIZE_SD;
    let mut fb = PixFb::new(fb_w, fb_h, PixelFormat::Rgba8888);
    let src = gradient_texture(64, 64);

    group.bench_function("format_mismatch", |b| {
        b.iter(|| {
            fb.renderer.blit(
                black_box(Pixel::new(0, 0)),
                64,
                64,
                PixelFormat::Bgra8888,
                black_box(&src),
                64 * 4,
            );
        });
    });

    group.bench_function("src_too_short", |b| {
        b.iter(|| {
            fb.renderer.blit(
                black_box(Pixel::new(0, 0)),
                64,
                64,
                PixelFormat::Rgba8888,
                black_box(&src[..64]),
                64 * 4,
            );
        });
    });

    group.bench_function("pitch_below_row_len", |b| {
        b.iter(|| {
            fb.renderer.blit(
                black_box(Pixel::new(0, 0)),
                64,
                64,
                PixelFormat::Rgba8888,
                black_box(&src),
                4,
            );
        });
    });

    group.finish();
}

/// 格式解码与编码的完整性覆盖：全部 8 种格式各测一次 `set_pixel`。
///
/// 前四组 [`bench_set_pixel_opaque`] 只是代表性取样；这里是穷举，
/// 确保新增格式时基准会自动带上它。
fn bench_all_formats_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("set_pixel_all_formats");
    let (width, height) = (256usize, 256usize);

    for format in ALL_FORMATS {
        let mut fb = PixFb::new(width, height, format);
        group.bench_with_input(BenchmarkId::from_parameter(format!("{format:?}")), &format, |b, _| {
            let mut x = 0usize;
            let mut y = 0usize;
            b.iter(|| {
                fb.renderer.set_pixel(black_box(x), black_box(y), &TRANSLUCENT);
                x = (x + 1) % width;
                y = (y + 7) % height;
            });
        });
    }

    group.finish();
}

/// 子视口构造：`sub_view` 的几何计算 + 子渲染器构造。
///
/// 每次调用都要做一遍溢出检查与收缩计算，是分屏渲染的每帧固定开销。
fn bench_sub_view(c: &mut Criterion) {
    let mut group = c.benchmark_group("sub_view");
    let (width, height) = SIZE_HD;
    let info = FramebufferInfo::new(width, height, width * 4, PixelFormat::Rgba8888);
    let mut fb = vec![0u8; info.buffer_size()];

    for (x, y, w, h) in [(0usize, 0usize, 640usize, 480usize), (8, 8, 640, 480), (960, 540, 960, 540)] {
        let label = format!("at_{x}_{y}_{w}x{h}");
        group.bench_with_input(BenchmarkId::new("rgba8888", &label), &label, |b, _| {
            b.iter(|| {
                let view = sub_view(black_box(&mut fb), &info, x, y, w, h);
                black_box(view.is_some());
            });
        });
    }

    group.finish();
}

/// 双缓冲子视口：`sub_view_with_back_buffer` 的构造 + 一次呈现。
///
/// 呈现只写回窗口内的可见字节，是分屏双缓冲的典型每帧成本。
fn bench_sub_view_swap(c: &mut Criterion) {
    let mut group = c.benchmark_group("sub_view_swap");
    let (width, height) = SIZE_HD;
    let info = FramebufferInfo::new(width, height, width * 4, PixelFormat::Rgba8888);
    let size = info.buffer_size();
    let mut fb = vec![0u8; size];
    let mut back = vec![0u8; size];

    group.bench_function("960x540_present", |b| {
        b.iter(|| {
            let mut view =
                sub_view_with_back_buffer(black_box(&mut fb), black_box(&mut back), &info, 0, 0, 960, 540)
                    .expect("窗口应可用");
            view.swap_buffers();
        });
    });

    group.finish();
}

criterion_group!(
    name = pixel_and_buffer;
    config = criterion_config();
    targets =
    bench_set_pixel_opaque,
    bench_set_pixel_alpha,
    bench_get_pixel,
    bench_pixel_offset,
    bench_clear,
    bench_swap_buffers,
    bench_blit,
    bench_blit_rejected,
    bench_all_formats_write,
    bench_sub_view,
    bench_sub_view_swap
);
fn main() {
    // 先量一次本机抖动：它决定本次数字是否值得采信。
    shared::report_jitter();
    // `criterion_config()` 给的是本项目统一的参数，`configure_from_args()`
    // 再叠加命令行覆盖（`--quick`、`--sample-size` 等），后者优先。
    pixel_and_buffer();
}
