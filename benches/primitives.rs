//! 固定功能图元基准：线、三角形、矩形、圆、圆弧、掩码混合。
//!
//! 覆盖的公开面（`src/core.rs`）：
//!
//! * `draw_line`（Bresenham 受限内核 + Cohen–Sutherland 裁剪）
//! * `draw_triangle` / `fill_triangle`（扫描线，半开规则）
//! * `draw_rect` / `fill_rect`（跨度填充，含 SWAR 不透明路径）
//! * `draw_circle` / `fill_circle`（精确圆盘语义）
//! * `draw_arc` / `fill_arc`（楔形判定，整圈时退化到圆路径）
//! * `blit_mask`（8bpp 掩码混合，字体/精灵的底层）
//!
//! 每组都配了不透明与半透明两档：前者走 `fill_span_opaque` 的批量快速路径，
//! 后者逐像素做读-改-写。两者的差值就是「混合成本」，也是 SWAR 是否生效的判据。

mod shared;

use core::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use rasteron::color::{Color, PixelFormat};
use rasteron::{Angle, Pixel};

use shared::{
    CORE_FORMATS, OPAQUE, SIZE_HD, SIZE_SD, TRANSLUCENT, antialiased_mask, criterion_config,
    gradient_texture, hard_mask, PixFb,
};

/// 采样几何：一组覆盖「水平/垂直/对角/陡峭」四种斜率的线段。
const LINE_SPAN: usize = 400;

/// 线段绘制：斜率与长度的扫描。
///
/// 斜率决定 Bresenham 主轴上每次迭代写多少像素，是内核分支行为的
/// 主要来源；长度决定迭代量。四档斜率 × 两档颜色足以刻画出内核形态。
fn bench_draw_line(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_line");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);

    for (label, from, to) in [
        ("horizontal", Pixel::new(40, 200), Pixel::new(40 + LINE_SPAN, 200)),
        ("vertical", Pixel::new(200, 40), Pixel::new(200, 40 + LINE_SPAN)),
        ("diagonal", Pixel::new(40, 40), Pixel::new(40 + LINE_SPAN, 40 + LINE_SPAN)),
        ("shallow", Pixel::new(40, 100), Pixel::new(40 + LINE_SPAN, 140)),
        ("steep", Pixel::new(100, 40), Pixel::new(140, 40 + LINE_SPAN)),
    ] {
        for (alpha_label, color) in [("opaque", OPAQUE), ("alpha", TRANSLUCENT)] {
            let id = format!("{label}_{alpha_label}_{LINE_SPAN}px");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer.draw_line(black_box(from), black_box(to), &color);
                });
            });
        }
    }

    group.finish();
}

/// 线段裁剪路径：完全在外、部分相交、完全在内。
///
/// 本库的 `Pixel` 字段是 `usize`，负坐标无法表达，因此「完全在外」只能通过
/// 超出右/下边界构造。全部在外时应被 `clip_line` 直接拒绝，成本恒定。
fn bench_draw_line_clipped(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_line_clipped");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);

    let cases = [
        ("fully_outside_right", Pixel::new(10_000, 10), Pixel::new(20_000, 400)),
        ("fully_outside_bottom", Pixel::new(10, 10_000), Pixel::new(400, 20_000)),
        ("straddling_right", Pixel::new(600, 10), Pixel::new(5_000, 400)),
        ("straddling_corner", Pixel::new(600, 400), Pixel::new(5_000, 9_000)),
        ("fully_inside", Pixel::new(10, 10), Pixel::new(600, 400)),
    ];

    for (label, from, to) in cases {
        group.bench_with_input(BenchmarkId::from_parameter(label), &label, |b, _| {
            b.iter(|| {
                fb.renderer.draw_line(black_box(from), black_box(to), &OPAQUE);
            });
        });
    }

    group.finish();
}

/// 三角形描边：三条线段串联。
fn bench_draw_triangle(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_triangle");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);

    for side in [32usize, 128, 400] {
        let p1 = Pixel::new(40, 40);
        let p2 = Pixel::new(40 + side, 60);
        let p3 = Pixel::new(80, 40 + side);
        for (alpha_label, color) in [("opaque", OPAQUE), ("alpha", TRANSLUCENT)] {
            let id = format!("side{side}_{alpha_label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer.draw_triangle(black_box(p1), black_box(p2), black_box(p3), &color);
                });
            });
        }
    }

    group.finish();
}

/// 三角形填充：扫描线内核，面积决定写入量。
///
/// 半透明档是重点：它逐像素读-改-写，而 `fill_triangle` 的半开扫描线规则
/// （`y2` 行只属于下半段）保证每行只发射一次，否则半透明行会明显更亮。
/// 这组基准的吞吐异常放大通常意味着该规则被破坏。
fn bench_fill_triangle(c: &mut Criterion) {
    let mut group = c.benchmark_group("fill_triangle");
    let (fb_w, fb_h) = SIZE_HD;
    let mut fb = PixFb::new(fb_w, fb_h, PixelFormat::Rgba8888);

    for side in [16usize, 64, 256] {
        let p1 = Pixel::new(10, 10);
        let p2 = Pixel::new(10 + side, 20);
        let p3 = Pixel::new(30, 10 + side);
        // 面积按半宽估算，用于吞吐归一化。
        let pixels = ((side * side) / 2) as u64;
        group.throughput(Throughput::Elements(pixels));

        for (alpha_label, color) in [("opaque", OPAQUE), ("alpha", TRANSLUCENT)] {
            let id = format!("side{side}_{alpha_label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer.fill_triangle(black_box(p1), black_box(p2), black_box(p3), &color);
                });
            });
        }
    }

    // 水平底边：走 `min(x2, x3)..max(x2, x3)` 的专门分支，
    // 与顶点顺序无关（历史缺陷的回归点）。
    group.throughput(Throughput::Elements((256 * 128) as u64));
    group.bench_function("horizontal_bottom_edge", |b| {
        b.iter(|| {
            fb.renderer.fill_triangle(
                black_box(Pixel::new(100, 100)),
                black_box(Pixel::new(356, 100)),
                black_box(Pixel::new(100, 228)),
                &TRANSLUCENT,
            );
        });
    });

    group.finish();
}

/// 三角形填充：越界顶点的钳位路径。
///
/// 契约是「钳到边界」而非几何裁剪，故巨大顶点仍会扫过钳位后的整个跨度。
/// 这条基准的价值在于钉住「越权防护是功能」——迭代量不随原始坐标爆炸。
fn bench_fill_triangle_clamped(c: &mut Criterion) {
    let mut group = c.benchmark_group("fill_triangle_clamped");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);

    group.bench_function("huge_vertices", |b| {
        b.iter(|| {
            fb.renderer.fill_triangle(
                black_box(Pixel::new(usize::MAX / 2, usize::MAX / 2)),
                black_box(Pixel::new(0, 0)),
                black_box(Pixel::new(639, 479)),
                &OPAQUE,
            );
        });
    });

    group.bench_function("one_corner_only", |b| {
        b.iter(|| {
            fb.renderer.fill_triangle(
                black_box(Pixel::new(0, 0)),
                black_box(Pixel::new(1, 1)),
                black_box(Pixel::new(0, 1)),
                &OPAQUE,
            );
        });
    });

    group.finish();
}

/// 矩形描边与填充。
///
/// 填充走 `fill_visible_horizontal_span`：先排序、再求交、最后隐式夹取。
/// 尺寸扫描用于区分固定开销主导（小矩形）与带宽主导（大矩形）。
fn bench_rect(c: &mut Criterion) {
    let mut group = c.benchmark_group("rect");
    let (fb_w, fb_h) = SIZE_HD;
    let mut fb = PixFb::new(fb_w, fb_h, PixelFormat::Rgba8888);

    for side in [8usize, 32, 128, 512] {
        for (kind, alpha_label, color) in [
            ("draw", "opaque", OPAQUE),
            ("fill", "opaque", OPAQUE),
            ("fill", "alpha", TRANSLUCENT),
        ] {
            // 吞吐量按该变体**实际写入的像素数**标注：描边只覆盖周长
            // （约 4×side），填充覆盖整个面积。两者共用 side² 会让描边的
            // 吞吐虚高 side/4 倍，跨变体比较就失去意义。
            let elements = if kind == "draw" {
                (4 * side) as u64
            } else {
                (side * side) as u64
            };
            group.throughput(Throughput::Elements(elements));

            let id = format!("{kind}_side{side}_{alpha_label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    let top_left = black_box(Pixel::new(10, 10));
                    if kind == "draw" {
                        fb.renderer.draw_rect(top_left, black_box(side), black_box(side), &color);
                    } else {
                        fb.renderer.fill_rect(top_left, black_box(side), black_box(side), color);
                    }
                });
            });
        }
    }

    group.finish();
}

/// 矩形填充：超出帧缓冲的裁剪。
///
/// 半透明色下的正确性依赖「每像素只写一次」；纯越界请求应被求交直接拒绝。
fn bench_fill_rect_clipped(c: &mut Criterion) {
    let mut group = c.benchmark_group("fill_rect_clipped");
    let (fb_w, fb_h) = SIZE_HD;
    let mut fb = PixFb::new(fb_w, fb_h, PixelFormat::Rgba8888);

    group.bench_function("fully_outside", |b| {
        b.iter(|| {
            fb.renderer
                .fill_rect(black_box(Pixel::new(5_000, 5_000)), black_box(100), black_box(100), OPAQUE);
        });
    });

    group.bench_function("oversized_alpha", |b| {
        b.iter(|| {
            fb.renderer.fill_rect(
                black_box(Pixel::new(0, 0)),
                black_box(fb_w * 2),
                black_box(fb_h * 2),
                TRANSLUCENT,
            );
        });
    });

    group.finish();
}

/// 圆与圆弧：描边/填充 × 半径扫描。
///
/// `draw_circle` 逐行求边界点，`fill_circle` 逐行取精确跨度（每半行一次开方），
/// 两者的成本随半径的增长率不同，因此半径扫描比单点更能刻画内核。
fn bench_circle(c: &mut Criterion) {
    let mut group = c.benchmark_group("circle");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);

    for radius in [8usize, 64, 200] {
        for (kind, alpha_label, color) in [
            ("draw", "opaque", OPAQUE),
            ("fill", "alpha", TRANSLUCENT),
        ] {
            // 描边覆盖约一周像素（2πr），填充覆盖圆面积（πr²）。
            // 分别标注才能让两者的吞吐数字互相可比。
            let elements = if kind == "draw" {
                (radius as f64 * 6.28) as u64
            } else {
                (radius as f64 * radius as f64 * 3.14159) as u64
            };
            group.throughput(Throughput::Elements(elements.max(1)));

            let id = format!("{kind}_r{radius}_{alpha_label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    let center = black_box(Pixel::new(240, 240));
                    if kind == "draw" {
                        fb.renderer.draw_circle(center, black_box(radius), &color);
                    } else {
                        fb.renderer.fill_circle(center, black_box(radius), color);
                    }
                });
            });
        }
    }

    // 半径钳制：半径远大于帧缓冲时迭代量应被钳为 O(height)。
    group.bench_function("fill_radius_clamped", |b| {
        b.iter(|| {
            fb.renderer
                .fill_circle(black_box(Pixel::new(320, 240)), black_box(usize::MAX / 4), OPAQUE);
        });
    });

    group.finish();
}

/// 圆弧与扇形：角度跨度的扫描。
///
/// 跨度从 1° 到整圈。整圈会被 `wedge.is_full()` 拦下并退化到圆路径，
/// 因此「整圈」这一档应显著快于「350°」——这正是分派生效的证据。
fn bench_arc(c: &mut Criterion) {
    let mut group = c.benchmark_group("arc");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);
    let radius = 180usize;
    let center = Pixel::new(320, 240);

    for (label, start, end) in [
        ("span_45deg", Angle::ZERO, Angle::from_degrees(45)),
        ("span_180deg", Angle::ZERO, Angle::from_degrees(180)),
        ("span_350deg", Angle::ZERO, Angle::from_degrees(350)),
        ("full_circle", Angle::ZERO, Angle::ZERO),
    ] {
        for (kind, color) in [("draw", OPAQUE), ("fill", OPAQUE)] {
            let id = format!("{kind}_{label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    let center = black_box(center);
                    if kind == "draw" {
                        fb.renderer.draw_arc(center, black_box(radius), start, end, &color);
                    } else {
                        fb.renderer.fill_arc(center, black_box(radius), start, end, color);
                    }
                });
            });
        }
    }

    // 扇形的半透明档：每行只遍历一次，故不该出现重复混合导致的额外成本。
    group.bench_function("fill_span_90deg_alpha", |b| {
        b.iter(|| {
            fb.renderer.fill_arc(
                black_box(center),
                black_box(radius),
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                TRANSLUCENT,
            );
        });
    });

    group.finish();
}

/// 掩码混合：`blit_mask` 的尺寸与掩码形态扫描。
///
/// 掩码形态分两档：
///
/// * `hard` —— 只有 0/255，SWAR 掩码内核走「全 0 跳行 / 全 255 快写」的分组；
/// * `antialiased` —— 边缘有中间值，强制走逐像素混合的回退分支。
///
/// 两者的差距就是抗锯齿掩码在 SWAR 路径上的实际代价。
///
/// 格式固定为 `Rgba8888`（4bpp、行首对齐、走 SWAR）：跨格式的差异由
/// [`bench_blit_mask_formats`] 专门覆盖，这里再排一遍格式维度只是重复。
fn bench_blit_mask(c: &mut Criterion) {
    let mut group = c.benchmark_group("blit_mask");
    let (fb_w, fb_h) = SIZE_HD;
    let mut fb = PixFb::new(fb_w, fb_h, PixelFormat::Rgba8888);

    for side in [16usize, 64, 128] {
        let aa = antialiased_mask(side, side);
        let hard = hard_mask(side, side);
        group.throughput(Throughput::Elements((side * side) as u64));

        for (kind, mask) in [("hard", &hard), ("antialiased", &aa)] {
            let id = format!("side{side}_{kind}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer.blit_mask(
                        black_box(Pixel::new(0, 0)),
                        black_box(side),
                        black_box(side),
                        black_box(mask),
                        black_box(side),
                        OPAQUE,
                    );
                });
            });
        }
    }

    group.finish();
}

/// 掩码混合：由真实纹理派生掩码，模拟精灵绘制。
///
/// 与 [`bench_blit_mask`] 的区别是掩码内容来自解码后的纹理（含 alpha 渐变），
/// 更接近「带透明通道的精灵贴图」这一实际用法。
fn bench_blit_mask_sprite(c: &mut Criterion) {
    let mut group = c.benchmark_group("blit_mask_sprite");
    let (fb_w, fb_h) = SIZE_HD;
    let mut fb = PixFb::new(fb_w, fb_h, PixelFormat::Rgba8888);

    for side in [16usize, 64, 256] {
        let texels = gradient_texture(side, side);
        // 取每像素的 alpha 通道当掩码。
        let mask: Vec<u8> = texels.chunks_exact(4).map(|texel| texel[3]).collect();
        group.throughput(Throughput::Elements((side * side) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(side), &side, |b, _| {
            b.iter(|| {
                fb.renderer.blit_mask(
                    black_box(Pixel::new(0, 0)),
                    black_box(side),
                    black_box(side),
                    black_box(&mask),
                    black_box(side),
                    TRANSLUCENT,
                );
            });
        });
    }

    group.finish();
}

/// 全格式覆盖：半透明掩码混合在核心格式集合上的表现。
///
/// `blit_mask` 对 565 走「解码 → mix_alpha → blend_565」，对 4bpp 对齐走 SWAR，
/// 对 3bpp 逐通道回退，三条路径都在这组里。
fn bench_blit_mask_formats(c: &mut Criterion) {
    let mut group = c.benchmark_group("blit_mask_formats");
    let side = 64usize;
    let mask = antialiased_mask(side, side);
    group.throughput(Throughput::Elements((side * side) as u64));

    for format in CORE_FORMATS {
        let mut fb = PixFb::new(SIZE_HD.0, SIZE_HD.1, format);
        group.bench_with_input(BenchmarkId::from_parameter(format!("{format:?}")), &format, |b, _| {
            b.iter(|| {
                fb.renderer.blit_mask(
                    black_box(Pixel::new(0, 0)),
                    black_box(side),
                    black_box(side),
                    black_box(&mask),
                    black_box(side),
                    OPAQUE,
                );
            });
        });
    }

    group.finish();
}

/// 混合对写满一屏的成本：格式 × 透明度的矩阵。
///
/// 这组是「SWAR 是否生效」最直接的判据：4bpp 的半透明档应明显快于
/// `Bgr24`，因为后者因跨字节错位**始终**逐通道回退。
///
/// 格式只取三档代表，不穷举四种：`Xrgb8888` 与 `Rgba8888` 走**同一条**
/// SWAR 内核，差别仅在第 4 字节写 0 还是 255，重复测量得不到新信息。
/// 透明度保留五档——它们命中不同的短路分支（`a == 0` 空操作、
/// `a == 255` 纯覆盖、其余混合），是有区分度的维度。
fn bench_fill_blend_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("fill_blend_matrix");
    let (fb_w, fb_h) = SIZE_SD;
    group.throughput(Throughput::Elements((fb_w * fb_h) as u64));

    for format in [PixelFormat::Rgba8888, PixelFormat::Bgr24, PixelFormat::Rgb565] {
        let mut fb = PixFb::new(fb_w, fb_h, format);
        for (alpha_label, color) in [
            ("a0", Color::with_alpha(200, 100, 50, 0)),
            ("a1", Color::with_alpha(200, 100, 50, 1)),
            ("a128", Color::with_alpha(200, 100, 50, 128)),
            ("a254", Color::with_alpha(200, 100, 50, 254)),
            ("a255", Color::with_alpha(200, 100, 50, 255)),
        ] {
            let id = format!("{format:?}_{alpha_label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer
                        .fill_rect(black_box(Pixel::new(0, 0)), fb_w, fb_h, color);
                });
            });
        }
    }

    group.finish();
}

criterion_group!(
    name = primitives;
    config = criterion_config();
    targets =
    bench_draw_line,
    bench_draw_line_clipped,
    bench_draw_triangle,
    bench_fill_triangle,
    bench_fill_triangle_clamped,
    bench_rect,
    bench_fill_rect_clipped,
    bench_circle,
    bench_arc,
    bench_blit_mask,
    bench_blit_mask_sprite,
    bench_blit_mask_formats,
    bench_fill_blend_matrix
);
fn main() {
    // 先量一次本机抖动：它决定本次数字是否值得采信。
    shared::report_jitter();
    // `criterion_config()` 给的是本项目统一的参数，`configure_from_args()`
    // 再叠加命令行覆盖（`--quick`、`--sample-size` 等），后者优先。
    primitives();
}
