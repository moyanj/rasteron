//! 字体基准：位图（8×16）与 TrueType 两条后端，以及度量/光栅化/绘制三层。
//!
//! 整个文件由 `#[cfg(any(feature = "font_bitmap", feature = "font_truetype"))]`
//! 门控——`rasteron::font` 模块只在两个字体 feature 至少开一个时存在，
//! `Renderer` 上的 `draw_char` / `draw_text` / `draw_glyph` 也随之消失。
//!
//! 覆盖的公开面（`src/font/mod.rs`）：
//!
//! * `Font` trait 的四个方法：`has_glyph` / `layout` / `metrics` / `rasterize`
//! * `GlyphLayout::{mask_len, empty}`、`FontMetrics::{line_height, is_finite}`
//! * `PenX::{from_px, advance, to_px, pixel_x}`、`FontLayout::{new, line_height}`
//! * `Char::{new, size, mask}`、`Renderer::draw_glyph`
//! * `Renderer::{draw_char, draw_text}`（任意构建）与
//!   `{draw_char_owned, draw_text_owned}`（`alloc` feature）
//!
//! # 两条后端的取舍
//!
//! `DefaultFont` 是编译期常量表，`layout` / `rasterize` 都是纯查表 + 整段拷贝，
//! 且**不产生任何浮点运算**。`TtfFont` 走 fontdue：度量按 `px` 缩放、
//! 光栅化是浮点累加，且每次 `rasterize` 都有一次堆分配（fontdue 返回 `Vec<u8>`）。
//! 两者并排测量可以把「点阵快在哪」量化出来。

#![cfg(any(feature = "font_bitmap", feature = "font_truetype"))]

mod shared;

use core::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use rasteron::color::{Color, PixelFormat};
use rasteron::font::{Char, FontMetrics, FontLayout, GlyphLayout, PenX};
use rasteron::pixel::Pixel;

// `Font` trait 必须导入才能调用 `layout` / `rasterize` / `has_glyph`。
// 只有 TrueType 基准会直接调这些方法——位图路径全部经 `Renderer::draw_*`
// 间接驱动，故仅在 `font_truetype` 下导入，避免 `font_bitmap` 单独开启时
// 产生未用导入警告。
#[cfg(feature = "font_truetype")]
use rasteron::font::Font as _;
#[cfg(feature = "font_truetype")]
use rasteron::font::ttf::TtfFont;

use shared::{OPAQUE, SIZE_HD, SIZE_SD, criterion_config, PixFb};

/// 子集化测试字体（DejaVu Sans，7 字形，2048 unitsPerEm）。
///
/// 用 `include_bytes!` 而非 `fs::read`：不依赖基准进程的当前目录，
/// 且与库内测试、示例的加载方式一致。
#[cfg(feature = "font_truetype")]
const MINIMAL_FONT: &[u8] = include_bytes!("../tests/fixtures/minimal_ascii.ttf");

/// 测试字体覆盖的字形是 `A g j i W | space`：`W` 最宽、`j` 有负 bearing
/// （悬挂部，触发 `draw_glyph` 的源掩码左边界裁剪）。基准取 `W` 与 `j`
/// 作为两个代表——其余字形的成本落在两者之间，穷举没有额外信息。

/// 一行有代表性的 ASCII 文本，用于文本绘制基准。
const SAMPLE_LINE: &str = "Rasteron 8x16 !@#$%^&*()_+-=[]{}";

/// 多行文本：内含 8 个换行，覆盖基线重算与行距推进。
const SAMPLE_BLOCK: &str = "Rasteron bitmap font\nABCDEFGHIJKLMNOPQRSTUVWXYZ\nabcdefghijklmnopqrstuvwxyz\n0123456789 !@#$%^&*()\n\
second paragraph line\nthird paragraph line\nfourth paragraph line\nfifth paragraph line";

/// 位图字体：`Font` trait 四方法的直接调用。
///
/// `DefaultFont` 是私有模块内的 static，crate 外只能通过 `None` 隐式使用，
/// 因此这里用 `Renderer::draw_char(.., font: None, ..)` 驱动它，
/// 并把 `layout` / `metrics` 的成本归到 [`bench_text_draw`] 与
/// [`bench_glyph_layout_reference`] 里。
#[cfg(feature = "font_bitmap")]
fn bench_bitmap_draw_char(c: &mut Criterion) {
    let mut group = c.benchmark_group("bitmap_draw_char");
    let (width, height) = SIZE_SD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    // 8×16 字形的掩码是 128 字节，给足余量。
    let mut scratch = [0u8; 8 * 16];

    for c in ['A', 'g', 'W', 'j', ' '] {
        let label = match c {
            ' ' => "space".to_string(),
            other => format!("{other}"),
        };
        group.bench_with_input(BenchmarkId::from_parameter(&label), &label, |b, _| {
            b.iter(|| {
                fb.renderer.draw_char(
                    black_box(Pixel::new(100, 100)),
                    black_box(c),
                    black_box(16.0),
                    OPAQUE,
                    None,
                    black_box(&mut scratch),
                );
            });
        });
    }

    // 缺字回退：非 ASCII 字符落到内置替代字形。
    group.bench_function("missing_glyph_fallback", |b| {
        b.iter(|| {
            fb.renderer.draw_char(
                black_box(Pixel::new(100, 100)),
                black_box('中'),
                black_box(16.0),
                OPAQUE,
                None,
                black_box(&mut scratch),
            );
        });
    });

    group.finish();
}

/// 位图字体：文本绘制，行长与行数扫描。
///
/// 覆盖画笔推进（`PenX` 的 26.16 定点累加）、换行时不逐行累加而是
/// 「首行基线 + 行号 × 行高」重算、以及 `\n` 后画笔回到行首。
#[cfg(feature = "font_bitmap")]
fn bench_bitmap_draw_text(c: &mut Criterion) {
    let mut group = c.benchmark_group("bitmap_draw_text");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let mut scratch = [0u8; 8 * 16];

    // 不同长度的单行文本：字符数是吞吐量的自然度量。
    for chars in [1usize, 16, 64, 256] {
        let line: String = SAMPLE_LINE.chars().cycle().take(chars).collect();
        group.throughput(Throughput::Elements(chars as u64));
        let id = format!("single_line_{chars}chars");
        group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
            b.iter(|| {
                fb.renderer.draw_text(
                    black_box(Pixel::new(10, 10)),
                    black_box(&line),
                    black_box(16.0),
                    OPAQUE,
                    black_box(&rasteron_font_default()),
                    black_box(&mut scratch),
                );
            });
        });
    }

    // 多行文本：8 行，覆盖换行与行距。
    group.throughput(Throughput::Elements(SAMPLE_BLOCK.len() as u64));
    group.bench_function("multi_line_8_rows", |b| {
        b.iter(|| {
            fb.renderer.draw_text(
                black_box(Pixel::new(10, 10)),
                black_box(SAMPLE_BLOCK),
                black_box(16.0),
                OPAQUE,
                black_box(&rasteron_font_default()),
                black_box(&mut scratch),
            );
        });
    });

    group.finish();
}

/// 取位图字体的 `&dyn Font`。
///
/// `DefaultFont` 位于私有模块，crate 外无法直接引用；本库对外的通道是
/// `draw_char` 的 `font: None` 参数。这里用一个自定义的 `Font` 实现
/// 代替它来给 `draw_text` 供字体——`draw_text` 不接受 `None`。
#[cfg(feature = "font_bitmap")]
fn rasteron_font_default() -> BitmapFont {
    BitmapFont
}

/// 最小的自定义 `Font` 实现：8×16 单元掩码、无浮点。
///
/// 它让基准能在不触碰私有 `DefaultFont` 的前提下测 `draw_text` 的排版开销——
/// 掩码内容对排版成本没有影响，只影响 `blit_mask` 的混合分支。
#[cfg(feature = "font_bitmap")]
struct BitmapFont;

#[cfg(feature = "font_bitmap")]
impl rasteron::font::Font for BitmapFont {
    fn has_glyph(&self, c: char) -> bool {
        c.is_ascii()
    }

    fn layout(&self, c: char, px: f32) -> Option<GlyphLayout> {
        if px <= 0.0 || !px.is_finite() || !c.is_ascii() {
            return None;
        }
        Some(GlyphLayout {
            width: 8,
            height: 16,
            bearing_x: 0,
            bearing_y: 12,
            advance: 8.0,
        })
    }

    fn metrics(&self, _px: f32) -> FontMetrics {
        FontMetrics { ascent: 12.0, descent: 4.0, line_gap: 0.0 }
    }

    fn rasterize(
        &self,
        c: char,
        _px: f32,
        layout: &GlyphLayout,
        mask: &mut [u8],
    ) -> Option<usize> {
        let needed = layout.mask_len()?;
        assert!(mask.len() >= needed, "BitmapFont::rasterize: 掩码容量不足");
        // 用字符值生成有纹理的掩码：纯常量填充会让 blit_mask 的分支
        // 被预测得一干二净，掩盖真实的混合成本。
        let seed = c as u8;
        for (index, byte) in mask[..needed].iter_mut().enumerate() {
            *byte = if (index as u8).wrapping_add(seed) % 3 == 0 { 255 } else { 96 };
        }
        Some(needed)
    }
}

/// TrueType：度量与光栅化的分层成本。
///
/// `layout` / `metrics` 是零分配的纯值查询，`rasterize` 每次有一次堆分配
/// （fontdue 返回拥有所有权的 `Vec<u8>`，接口无法规避）。
/// 把两者分开测，可以把「fontdue 的分配」从「排版开销」里剥离出来。
///
/// 维度刻意取小：`layout` 是纯查表，各字形之间**没有**分支差异，穷举 6 个
/// 字形只会把同一个数字重复 6 遍。这里只留两个有代表性的——
/// `W`（最宽）与 `j`（唯一负 bearing，走左悬挂路径）。
#[cfg(feature = "font_truetype")]
fn bench_ttf_metrics_and_rasterize(c: &mut Criterion) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut group = c.benchmark_group("ttf_metrics_and_rasterize");

    // 字形度量：零分配。
    for c in ['W', 'j'] {
        group.bench_with_input(BenchmarkId::new("layout", format!("{c}")), &c, |b, &c| {
            b.iter(|| {
                black_box(font.layout(black_box(c), black_box(16.0)).unwrap());
            });
        });
    }

    // 行度量：只留两个字号——cost 随 px 单调，多取档位只是重复同一条曲线。
    for px in [16.0f32, 32.0] {
        let label = format!("px{px}");
        group.bench_with_input(BenchmarkId::new("metrics", &label), &label, |b, _| {
            b.iter(|| {
                black_box(font.metrics(black_box(px)));
            });
        });
    }

    // 光栅化：含 fontdue 的堆分配。字形 × 字号的笛卡尔积信息量低
    // （成本基本是 px² 的函数），故只留「最大字形 × 两档字号」。
    for c in ['W', 'j'] {
        for px in [16.0f32, 32.0] {
            let layout = font.layout(c, px).expect("字形应存在");
            let needed = layout.mask_len().expect("面积不应溢出");
            let mut mask = vec![0u8; needed];
            let label = format!("{c}_px{px}");
            group.bench_with_input(BenchmarkId::new("rasterize", &label), &label, |b, _| {
                b.iter(|| {
                    black_box(font.rasterize(black_box(c), black_box(px), &layout, &mut mask));
                });
            });
        }
    }

    // 缺字判定：应与 layout 的 Some/None 一致。
    group.bench_function("has_glyph_hit", |b| {
        b.iter(|| black_box(font.has_glyph(black_box('W'))));
    });
    group.bench_function("has_glyph_miss", |b| {
        b.iter(|| black_box(font.has_glyph(black_box('中'))));
    });

    group.finish();
}

/// TrueType：完整绘制链路（`draw_char`：度量 → 光栅化 → 掩码混合）。
///
/// 与上面分层测量的差值就是 `draw_glyph` 的定位与混合成本。字号扫描用于
/// 观察成本随字形面积的增长——掩码面积是 `px²` 量级。
#[cfg(feature = "font_truetype")]
fn bench_ttf_draw_char(c: &mut Criterion) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut group = c.benchmark_group("ttf_draw_char");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);

    // 字号 × 字形的笛卡尔积信息量低：成本基本由字形面积（px²）决定，
    // 字形之间只在 bearing 正负上有分支差异。留两档字号 × 两个代表字形。
    for px in [16.0f32, 32.0] {
        // scratch 取该字号下最大字形的面积上界。
        let cap = ((px as usize) + 9) * ((px as usize) + 9);
        let mut scratch = vec![0u8; cap];

        for c in ['W', 'j'] {
            let label = format!("{c}_px{px}");
            group.bench_with_input(BenchmarkId::from_parameter(&label), &label, |b, _| {
                b.iter(|| {
                    fb.renderer.draw_char(
                        black_box(Pixel::new(100, 200)),
                        black_box(c),
                        black_box(px),
                        OPAQUE,
                        Some(black_box(&font)),
                        black_box(&mut scratch),
                    );
                });
            });
        }
    }

    group.finish();
}

/// TrueType：文本绘制（`draw_text`），字号与行长扫描。
#[cfg(feature = "font_truetype")]
fn bench_ttf_draw_text(c: &mut Criterion) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut group = c.benchmark_group("ttf_draw_text");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);

    // 子集字体只有 7 个字形，用它们循环拼出长串以放大排版成本。
    let long_line: String = "AgjiW|".chars().cycle().take(128).collect();

    for px in [12.0f32, 32.0] {
        let cap = ((px as usize) + 9) * ((px as usize) + 9);
        let mut scratch = vec![0u8; cap];

        for (label, text) in [("short", "WAg"), ("long_128", long_line.as_str())] {
            let id = format!("{label}_px{px}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer.draw_text(
                        black_box(Pixel::new(10, 40)),
                        black_box(text),
                        black_box(px),
                        OPAQUE,
                        black_box(&font),
                        black_box(&mut scratch),
                    );
                });
            });
        }
    }

    group.finish();
}

/// `alloc` 便利变体：`draw_char_owned` / `draw_text_owned` 的每字符堆分配成本。
///
/// 契约是「两条路径逐字节相同」，但 owned 变体每个字符一次堆分配，
/// 因此这组基准是「便利 vs 性能」的取舍数据。
#[cfg(all(feature = "font_truetype", feature = "alloc"))]
fn bench_owned_variants(c: &mut Criterion) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut group = c.benchmark_group("owned_variants");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let text: String = "AgjiW|".chars().cycle().take(64).collect();

    let mut scratch = vec![0u8; 64 * 64];

    group.bench_function("draw_char_scratch_px32", |b| {
        b.iter(|| {
            fb.renderer.draw_char(
                black_box(Pixel::new(100, 200)),
                black_box('W'),
                black_box(32.0),
                OPAQUE,
                Some(black_box(&font)),
                black_box(&mut scratch),
            );
        });
    });

    group.bench_function("draw_char_owned_px32", |b| {
        b.iter(|| {
            fb.renderer.draw_char_owned(
                black_box(Pixel::new(100, 200)),
                black_box('W'),
                black_box(32.0),
                OPAQUE,
                Some(black_box(&font)),
            );
        });
    });

    group.bench_function("draw_text_scratch_64chars", |b| {
        b.iter(|| {
            fb.renderer.draw_text(
                black_box(Pixel::new(10, 40)),
                black_box(&text),
                black_box(16.0),
                OPAQUE,
                black_box(&font),
                black_box(&mut scratch),
            );
        });
    });

    group.bench_function("draw_text_owned_64chars", |b| {
        b.iter(|| {
            fb.renderer.draw_text_owned(
                black_box(Pixel::new(10, 40)),
                black_box(&text),
                black_box(16.0),
                OPAQUE,
                black_box(&font),
            );
        });
    });

    group.finish();
}

/// `draw_glyph`：已光栅化字形的定位与混合。
///
/// 这是「调用方自行缓存字形」的路径——没有度量查询与光栅化，
/// 只剩 bearing 定位、左/上边界裁剪与掩码混合。是字体渲染的成本下限。
fn bench_draw_glyph(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_glyph");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);

    for side in [8usize, 16, 32, 64] {
        let mask = shared::antialiased_mask(side, side);
        let layout = GlyphLayout {
            width: side,
            height: side,
            bearing_x: 0,
            bearing_y: side as i32,
            advance: side as f32,
        };
        group.throughput(Throughput::Elements((side * side) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(side), &side, |b, _| {
            b.iter(|| {
                fb.renderer.draw_glyph(
                    black_box(Pixel::new(100, 200)),
                    &layout,
                    black_box(&mask),
                    OPAQUE,
                );
            });
        });
    }

    // 负 bearing：悬挂部触发 `draw_glyph` 的源掩码左边界裁剪。
    // 这是「只裁目标不裁源」会让字形整体右移的那个分支。
    let side = 32usize;
    let mask = shared::antialiased_mask(side, side);
    let hanging = GlyphLayout {
        width: side,
        height: side,
        bearing_x: -8,
        bearing_y: side as i32,
        advance: 20.0,
    };
    group.bench_function("negative_bearing_x", |b| {
        b.iter(|| {
            fb.renderer.draw_glyph(
                black_box(Pixel::new(100, 200)),
                &hanging,
                black_box(&mask),
                OPAQUE,
            );
        });
    });

    // 原点在帧缓冲上边界之外：顶部裁剪分支。
    group.bench_function("clipped_above_top", |b| {
        let layout = GlyphLayout {
            width: side,
            height: side,
            bearing_x: 0,
            bearing_y: side as i32,
            advance: side as f32,
        };
        b.iter(|| {
            fb.renderer.draw_glyph(black_box(Pixel::new(100, 0)), &layout, black_box(&mask), OPAQUE);
        });
    });

    // 位图完全在画面外：应静默不画。
    group.bench_function("fully_outside", |b| {
        let layout = GlyphLayout {
            width: side,
            height: side,
            bearing_x: 0,
            bearing_y: side as i32,
            advance: side as f32,
        };
        b.iter(|| {
            fb.renderer.draw_glyph(
                black_box(Pixel::new(100_000, 100_000)),
                &layout,
                black_box(&mask),
                OPAQUE,
            );
        });
    });

    group.finish();
}

/// `PenX` 画笔：定点量化与亚像素累加。
///
/// 26.16 定点（1/65536 px）的存在理由就是「量化误差不随字距累积」。
/// 这组基准测的是每次步进的量化 + 检查成本，长串累加是排版热路径。
fn bench_pen_x(c: &mut Criterion) {
    let mut group = c.benchmark_group("pen_x");

    group.bench_function("from_px", |b| {
        b.iter(|| {
            black_box(PenX::from_px(black_box(123.456)));
        });
    });

    group.bench_function("to_px", |b| {
        let pen = PenX::from_px(123.456).unwrap();
        b.iter(|| {
            black_box(black_box(pen).to_px());
        });
    });

    group.bench_function("pixel_x", |b| {
        let pen = PenX::from_px(123.456).unwrap();
        b.iter(|| {
            black_box(black_box(pen).pixel_x());
        });
    });

    // 常见比例字体 advance：1 + 1/128，1/64 网格表示不了、1/65536 可以。
    group.bench_function("advance_typical", |b| {
        let mut pen = PenX::from_px(0.0).unwrap();
        b.iter(|| {
            black_box(pen.advance(black_box(9.0078125)));
        });
    });

    // 长串累加：1000 次步进，量化误差应恒为 0（不漂移）。
    group.bench_function("advance_1000_steps", |b| {
        b.iter(|| {
            let mut pen = PenX::from_px(0.0).unwrap();
            for _ in 0..1000 {
                // 显式忽略返回值：长串累加中也不该出现不可表示的值。
                let _ = pen.advance(9.0078125);
            }
            black_box(pen.to_px());
        });
    });

    // 不可表示的值：应返回 false 且画笔保持原值。
    group.bench_function("advance_rejected", |b| {
        let mut pen = PenX::from_px(0.0).unwrap();
        b.iter(|| {
            black_box(pen.advance(black_box(f32::NAN)));
        });
    });

    group.finish();
}

/// 字体度量与布局类型：纯值运算。
fn bench_font_layout_types(c: &mut Criterion) {
    let mut group = c.benchmark_group("font_layout_types");

    group.bench_function("mask_len", |b| {
        let layout = GlyphLayout { width: 16, height: 24, bearing_x: 0, bearing_y: 20, advance: 17.5 };
        b.iter(|| {
            black_box(black_box(&layout).mask_len());
        });
    });

    group.bench_function("metrics_line_height", |b| {
        let metrics = FontMetrics { ascent: 12.0, descent: 4.0, line_gap: 2.0 };
        b.iter(|| {
            black_box(black_box(&metrics).line_height());
        });
    });

    group.bench_function("metrics_is_finite", |b| {
        let metrics = FontMetrics { ascent: 12.0, descent: 4.0, line_gap: 2.0 };
        b.iter(|| {
            black_box(black_box(&metrics).is_finite());
        });
    });

    group.bench_function("font_layout_new", |b| {
        let metrics = FontMetrics { ascent: 12.0, descent: 4.0, line_gap: 2.0 };
        b.iter(|| {
            black_box(FontLayout::new(black_box(metrics), black_box(8.0)));
        });
    });

    group.bench_function("glyph_layout_empty", |b| {
        b.iter(|| {
            black_box(GlyphLayout::empty());
        });
    });

    // `Char::new` 带断言（掩码长度 ≥ 面积），是字形缓存的构造入口。
    let mask = [0u8; 16 * 16];
    group.bench_function("char_new", |b| {
        b.iter(|| {
            black_box(Char::new(black_box(&mask), black_box((16, 16))));
        });
    });

    group.finish();
}

/// 文本绘制的行数扫描：确认成本随行数线性增长（不逐行累加误差）。
#[cfg(feature = "font_bitmap")]
fn bench_text_rows(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_rows");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let mut scratch = [0u8; 8 * 16];
    let font = BitmapFont;

    for rows in [1usize, 4, 16, 64] {
        let text: String = (0..rows)
            .map(|i| format!("row {i} of the bitmap font sample"))
            .collect::<Vec<_>>()
            .join("\n");
        group.throughput(Throughput::Elements(rows as u64));
        let id = format!("rows{rows}");
        group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
            b.iter(|| {
                fb.renderer.draw_text(
                    black_box(Pixel::new(10, 10)),
                    black_box(&text),
                    black_box(16.0),
                    OPAQUE,
                    black_box(&font),
                    black_box(&mut scratch),
                );
            });
        });
    }

    group.finish();
}

/// 颜色对字体的影响：半透明字形走混合路径。
#[cfg(feature = "font_bitmap")]
fn bench_text_color(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_color");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let mut scratch = [0u8; 8 * 16];
    let font = BitmapFont;
    let line: String = SAMPLE_LINE.chars().cycle().take(64).collect();

    for (label, color) in [
        ("opaque", OPAQUE),
        ("alpha128", Color::with_alpha(255, 255, 255, 128)),
        ("transparent", Color::with_alpha(255, 255, 255, 0)),
    ] {
        group.bench_with_input(BenchmarkId::from_parameter(label), &label, |b, _| {
            b.iter(|| {
                fb.renderer.draw_text(
                    black_box(Pixel::new(10, 10)),
                    black_box(&line),
                    black_box(16.0),
                    color,
                    black_box(&font),
                    black_box(&mut scratch),
                );
            });
        });
    }

    group.finish();
}

// ---- 组装：按 feature 组合挑选目标 --------------------------------------

#[cfg(feature = "font_bitmap")]
criterion_group!(
    name = bitmap;
    config = criterion_config();
    targets =
    bench_bitmap_draw_char,
    bench_bitmap_draw_text,
    bench_text_rows,
    bench_text_color,
    bench_draw_glyph,
    bench_pen_x,
    bench_font_layout_types
);

#[cfg(all(feature = "font_truetype", not(feature = "font_bitmap")))]
criterion_group!(
    name = ttf_only;
    config = criterion_config();
    targets =
    bench_ttf_metrics_and_rasterize,
    bench_ttf_draw_char,
    bench_ttf_draw_text,
    bench_owned_variants,
    bench_draw_glyph,
    bench_pen_x,
    bench_font_layout_types
);

#[cfg(all(feature = "font_truetype", feature = "font_bitmap"))]
criterion_group!(
    name = truetype;
    config = criterion_config();
    targets =
    bench_ttf_metrics_and_rasterize,
    bench_ttf_draw_char,
    bench_ttf_draw_text,
    bench_owned_variants
);

// `criterion_main!` 展开后就是「依次调用各 group + final_summary()」。
// 这里手写等价实现，只为在最前面插入一次抖动自检——它决定本次数字是否可信。
//
// 三个 cfg 分支对应三种 feature 组合：只开位图、只开 TrueType、两个都开。

#[cfg(all(feature = "font_truetype", feature = "font_bitmap"))]
fn main() {
    shared::report_jitter();
    bitmap();
    truetype();
    Criterion::default().configure_from_args().final_summary();
}

#[cfg(all(feature = "font_truetype", not(feature = "font_bitmap")))]
fn main() {
    shared::report_jitter();
    ttf_only();
    Criterion::default().configure_from_args().final_summary();
}

#[cfg(all(feature = "font_bitmap", not(feature = "font_truetype")))]
fn main() {
    shared::report_jitter();
    bitmap();
    Criterion::default().configure_from_args().final_summary();
}
