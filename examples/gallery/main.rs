//! gallery — 全能力展示示例，也是人工比对渲染结果的基准图。
//!
//! 把 rasteron 的**全部**能力排成一张带编号的网格画布：固定功能图元、双缓冲与
//! 子视口、字体排版、可编程管线、像素格式。既是示例，也是回归基准图——改动物理
//! 任一图元后重跑它，与上一版输出逐格对比。
//!
//! 运行:
//!   cargo run --example gallery
//!
//! 产出 `gallery-example.bmp`（BGR24，1280×1086）。
//!
//! # Feature 前置
//!
//! 本示例展示 `programmable` 与两个字体 feature，故需**默认 feature 全开**
//! （`cargo run --example gallery` 即是）。`cargo build --no-default-features
//! --examples` 本来就会因 `examples/programmable` 而失败——示例目录整体按默认
//! feature 编译，示例不承担 feature 门控演示。
//!
//! # 分格结构
//!
//! 4 列 × 7 行共 28 格。格子登记在 `main` 的 `cells` 表：写一个
//! `fn case(r, c)` 按 `c`（内容区左上角）绘制，再在表里登记——序号即格号，
//! 由表的位置算出，不必硬编码。
//!
//! 需要 `Renderer` 之外资源的格子（子视口）由 `main` 的第二阶段处理，
//! 第一阶段只留占位函数 [`cell_placeholder_subview`]。
//!
//! # 标题为什么用库自己的字体
//!
//! 格子标题由 [`Renderer::draw_text`] + 内置 8×16 点阵字体绘制，不再维护一份
//! 手写字库——那等于绕着自己的功能写标签。点阵字体只有 8×16 这一份位图、
//! 不缩放，故标题一律按 1 倍绘制。

use rasteron::Renderer;
use rasteron::angle::Angle;
use rasteron::color::{
    BLACK, BLUE, CYAN, GRAY, GREEN, MAGENTA, RED, WHITE, YELLOW, Color, PixelFormat,
};
use rasteron::font::{DEFAULT_FONT, Font};
use rasteron::font::ttf::TtfFont;
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;
use rasteron::programmable::shader::{
    GradientFs, Passthrough2D, TextureFs as BuiltinTextureFs,
};
use rasteron::programmable::texture::{FilterMode, Texture2D, WrapMode};
use rasteron::programmable::{
    BlendMode, FragmentInput, FragmentOutput, FragmentShader, Pipeline, PipelineState,
    PrimitiveType, ScissorRect, Vec2, Vertex2D, Viewport,
};
use rasteron::utils::sub_view_with_back_buffer;

#[path = "../shared/bmp.rs"]
mod bmp;

/// 画布宽。四列均分。
const WIDTH: usize = 1280;
/// 列数。
const COLS: usize = 4;
/// 行数（28 格 = 7 行 × 4 列）。
const ROWS: usize = 7;
/// 格子间距（同时是画布的四周留白）。
const GAP: usize = 8;
/// 每格宽度（含间距）。
const CELL_W: usize = WIDTH / COLS;
/// 每格内容区高度。
const CELL_H: usize = 128;
/// 标题条高度：8×16 点阵行高 16，上下各留 5 px。
const TITLE_H: usize = 26;
/// 每格总高。
const CELL_H_TOTAL: usize = CELL_H + TITLE_H;
/// 画布高 = 行数 × 每格总高 + 底部留白。
const HEIGHT: usize = ROWS * CELL_H_TOTAL + GAP;

/// 内置 8×16 点阵字体的高度（像素），等于其行高；也决定 scratch 的下限容量。
const GLYPH_H: usize = 16;
/// 标题/注释字号。点阵字体不缩放，`px` 只做合法性检查，取 16.0 即可。
const TITLE_PX: f32 = 16.0;
/// 说明文字相对内容区顶部的 y。取 `CELL_H - GLYPH_H`，
/// 让 16 px 行高的文字**恰好**贴住内容区底边而不越到标题行上。
const NOTE_DY: usize = CELL_H - GLYPH_H;
/// TTF 排版格的 scratch 容量：字号最大 30 px，64×64 绰绰有余。
const TTF_SCRATCH: usize = 64 * 64;

/// 单格绘制函数签名：只给内容区左上角，尺寸由调用方保证不越界。
type CellFn = fn(&mut Renderer, Pixel);

/// 每格左侧强调色，按行取。
const ACCENTS: [Color; 4] = [RED, GREEN, BLUE, MAGENTA];

/// 画面底色（深灰蓝，让彩色图元都跳出来）。
const BG: Color = Color::new(24, 24, 30);
/// 格子边框色。
const BORDER: Color = Color::new(70, 70, 84);
/// 标题文字色。
const TITLE_FG: Color = Color::new(226, 230, 240);
/// 格内注释文字色（比标题暗一档，表示是说明而非标题）。
const NOTE_FG: Color = Color::new(140, 146, 162);
/// 刻度线色。
const RULE_FG: Color = Color::new(90, 96, 112);

/// 测试用 TrueType 字体（DejaVu Sans 子集，仅 7 字形）。
/// 编译期内联，示例不依赖运行时读文件。
const MINIMAL_FONT: &[u8] = include_bytes!("../../tests/fixtures/minimal_ascii.ttf");

/// fixture 只含 `A W g i j |` 七个字形，排版串只能用它们。
const TTF_SAMPLE: &str = "AWgij|";

/// 单元格左上角（父帧缓冲坐标系）。
fn cell_origin(col: usize, row: usize) -> Pixel {
    Pixel::new(col * CELL_W + GAP, row * CELL_H_TOTAL + GAP)
}

/// 画格子边框与标题条，返回**内容区**左上角。
fn draw_frame(r: &mut Renderer, origin: Pixel, index: usize, name: &str) -> Pixel {
    r.draw_rect(origin, CELL_W - GAP, CELL_H_TOTAL - GAP, &BORDER);
    // 左侧强调色块：按行取色，避免每格颜色雷同。
    r.fill_rect(
        Pixel::new(origin.x + 1, origin.y + 1),
        4,
        TITLE_H - 2,
        ACCENTS[index / COLS % ACCENTS.len()],
    );
    let mut scratch = [0u8; 8 * GLYPH_H];
    r.draw_text(
        Pixel::new(origin.x + 10, origin.y + 5),
        name,
        TITLE_PX,
        TITLE_FG,
        &DEFAULT_FONT,
        &mut scratch,
    );
    Pixel::new(origin.x + GAP, origin.y + TITLE_H)
}

/// 在格内画一行说明文字（1 倍点阵字体）。
fn note(r: &mut Renderer, c: Pixel, dy: usize, text: &str) {
    let mut scratch = [0u8; 8 * GLYPH_H];
    r.draw_text(
        Pixel::new(c.x, c.y + dy),
        text,
        TITLE_PX,
        NOTE_FG,
        &DEFAULT_FONT,
        &mut scratch,
    );
}

/// 把任意 `PixelFormat` 的小图贴到主画布。
///
/// `Renderer::blit` 的第一个短路条件就是「源格式必须与目标格式完全一致」，
/// 格式不同直接静默返回——而像素格式这几格要的恰恰是「同一图案在不同格式下的
/// 样子」。故这里逐像素走一趟 `read_color`，再交给 `set_pixel` 按主画布
/// （恒为 `Bgr24`）的通道顺序写回。两者互为逆运算，于是本 cell 显示的是
/// **格式差异本身**，而不是转换误差。
fn blit_converted(
    r: &mut Renderer,
    dst: Pixel,
    width: usize,
    height: usize,
    src_format: PixelFormat,
    src: &[u8],
    src_pitch: usize,
) {
    let bpp = src_format.bytes_per_pixel();
    for row in 0..height {
        for col in 0..width {
            let start = row * src_pitch + col * bpp;
            let Some(pixel) = src.get(start..start + bpp) else {
                // 源切片短于声明尺寸：与 `blit` 的既有契约一致，静默停止。
                return;
            };
            let color = src_format.read_color(pixel);
            r.set_pixel(dst.x + col, dst.y + row, &color);
        }
    }
}

fn main() {
    let format = PixelFormat::Bgr24;
    let info = FramebufferInfo::new(WIDTH, HEIGHT, WIDTH * format.bytes_per_pixel(), format);
    let mut fb = vec![0u8; info.buffer_size()];
    let mut back = vec![0u8; info.buffer_size()];

    // ── 第一阶段：主渲染器 ──
    // `sub_view` 需要父缓冲的裸切片，而 `Renderer` 在存活期间独占它，
    // 所以子渲染器那一格必须放到第二阶段（作用域结束、缓冲区归还之后）。
    {
        let mut r = Renderer::with_back_buffer(
            FramebufferInfo::new(WIDTH, HEIGHT, WIDTH * format.bytes_per_pixel(), format),
            &mut fb,
            &mut back,
        );
        r.clear(BG);

        let cells: [(&str, CellFn); 28] = [
            ("01 SET PIXEL", cell_set_pixel),
            ("02 LINE", cell_line),
            ("03 TRIANGLE", cell_triangle),
            ("04 RECT", cell_rect),
            ("05 CIRCLE", cell_circle),
            ("06 ARC OUTLINE", cell_arc_outline),
            ("07 ARC FILL", cell_arc_fill),
            ("08 ALPHA BLEND", cell_alpha),
            ("09 BLIT", cell_blit),
            ("10 BLIT MASK", cell_blit_mask),
            ("11 SUB VIEW", cell_placeholder_subview),
            ("12 PIXEL FORMAT", cell_pixel_format),
            ("13 FONT BITMAP", cell_font_bitmap),
            ("14 FONT TTF SIZE", cell_font_ttf),
            ("15 FONT BASELINE", cell_text_align),
            ("16 FONT MULTILINE", cell_text_multiline),
            ("17 TTF BEARING", cell_ttf_bearing),
            ("18 MISSING GLYPH", cell_missing_glyph),
            ("19 VS POINTS", cell_pipeline_points),
            ("20 VS GRADIENT", cell_pipeline_gradient),
            ("21 VS TEXTURE", cell_pipeline_texture),
            ("22 VS ADD BLEND", cell_pipeline_add_blend),
            ("23 RGB565 QUANT", cell_format_565),
            ("24 RGBA32 ORDER", cell_format_32bpp),
            ("25 SPAN FILL", cell_span_fill),
            ("26 GET PIXEL", cell_get_pixel),
            ("27 CLEAR SWAP", cell_clear_swap),
            ("28 FRAMEBUFFER", cell_info),
        ];

        for (i, &(name, draw)) in cells.iter().enumerate() {
            let origin = cell_origin(i % COLS, i / COLS);
            let content = draw_frame(&mut r, origin, i, name);
            draw(&mut r, content);
        }

        r.swap_buffers();
    }

    // ── 第二阶段：子渲染器 ──
    // 现在 `fb` 已归还，可以按切片构造受限渲染器。
    {
        let origin = cell_origin(2, 2); // 第 11 格
        // 窗口落在**内容区**（标题条下方），否则面板的 clear 会把第一阶段画好的
        // 标题与边框一并抹掉。宽度再减 8 留出右边距。
        let panel_x = origin.x + GAP;
        let panel_y = origin.y + TITLE_H;
        let panel_w = CELL_W - 2 * GAP - 8;
        let panel_h = CELL_H - 16;
        // 子视口同样双缓冲：后台按窗口的 `pitch × 高度` 口径准备。
        let mut panel_back = vec![0u8; info.pitch * panel_h];
        let Some(mut panel) = sub_view_with_back_buffer(
            &mut fb,
            &mut panel_back,
            &info,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
        ) else {
            panic!("sub_view_with_back_buffer 未能构造子渲染器");
        };
        let (pw, ph) = (panel.info().width, panel.info().height);
        assert!(
            panel.is_double_buffered(),
            "子视口同样支持 swap_buffers"
        );

        // 子渲染器内坐标从 (0, 0) 开始——这就是「独立坐标系」。
        panel.clear(Color::new(34, 34, 44));

        // 关键演示：圆心故意放在窗口**左外侧**，半径又大到足以跨过整窗，
        // 于是窗口内只应看到一条弧边——超出的部分被自然裁掉。
        let outside = Pixel::new(pw / 4, ph / 2);
        panel.fill_circle(outside, pw, BLUE);
        panel.draw_circle(outside, pw, &WHITE);

        // 窗口内再叠一个扇形与矩形，验证子渲染器内各图元都正常。
        panel.fill_arc(
            Pixel::new(pw * 2 / 3, ph / 2),
            pw / 4,
            Angle::from_degrees(90),
            Angle::from_degrees(210),
            Color::with_alpha(255, 255, 0, 150),
        );
        panel.fill_rect(
            Pixel::new(pw / 2, ph - 22),
            pw / 2,
            14,
            Color::with_alpha(0, 255, 0, 160),
        );
        // 边框画在最后，确保可见（否则会被前面的填充覆盖）。
        panel.draw_rect(Pixel::new(0, 0), pw, ph, &GRAY);

        panel.swap_buffers();
    }

    // ── 第三阶段：写文件 ──
    bmp::write_bmp(
        std::path::Path::new("gallery-example.bmp"),
        WIDTH,
        HEIGHT,
        format,
        &fb,
    )
    .expect("failed to write BMP");

    println!("gallery-example.bmp");
    println!(
        "framebuffer {}x{} pitch={} bpp={} total={} bytes",
        info.width,
        info.height,
        info.pitch,
        info.bytes_per_pixel(),
        info.buffer_size()
    );
}

// ── 01. set_pixel / get_pixel ──

/// 逐像素写出一块棋盘，再读回中心像素改白。
fn cell_set_pixel(r: &mut Renderer, c: Pixel) {
    for dy in 0..70 {
        for dx in 0..140 {
            let col = if (dx / 10 + dy / 10) % 2 == 0 {
                RED
            } else {
                YELLOW
            };
            r.set_pixel(c.x + dx, c.y + dy, &col);
        }
    }
    // 读回 → 覆盖为白，展示 get_pixel 的往返。
    let got = r.get_pixel(c.x + 70, c.y + 35);
    assert!(got.is_some(), "格内像素应可读回");
    r.set_pixel(c.x + 70, c.y + 35, &WHITE);
    note(r, c, NOTE_DY, "SET + GET PIXEL ROUNDTRIP");
}

// ── 02. draw_line ──

/// 扇形展开的线束；底部两条水平线用颠倒的同一对坐标，视觉上应一致
/// （渲染器对线段端点顺序不敏感）。
fn cell_line(r: &mut Renderer, c: Pixel) {
    for i in 0..14 {
        let x0 = c.x + 4 + i * 10;
        let color = if i % 2 == 0 { GREEN } else { BLUE };
        r.draw_line(
            Pixel::new(x0, c.y + 112),
            Pixel::new(x0, c.y + 4 + (i * 17) % 88),
            &color,
        );
    }
    r.draw_line(
        Pixel::new(c.x + 4, c.y + 106),
        Pixel::new(c.x + 140, c.y + 106),
        &CYAN,
    );
    r.draw_line(
        Pixel::new(c.x + 140, c.y + 106),
        Pixel::new(c.x + 4, c.y + 106),
        &CYAN,
    );
    // 退化线段（两端同点）：仍应点亮一个像素。
    r.draw_line(
        Pixel::new(c.x + 70, c.y + 118),
        Pixel::new(c.x + 70, c.y + 118),
        &WHITE,
    );
    note(r, c, NOTE_DY, "LINE ORDER + DEGENERATE");
}

// ── 03. draw_triangle / fill_triangle ──

/// 描边三角 + 实心三角 + 半透明三角叠加。
fn cell_triangle(r: &mut Renderer, c: Pixel) {
    let outer = [
        Pixel::new(c.x + 6, c.y + 108),
        Pixel::new(c.x + 70, c.y + 8),
        Pixel::new(c.x + 134, c.y + 108),
    ];
    r.draw_triangle(outer[0], outer[1], outer[2], &WHITE);
    r.fill_triangle(outer[0], outer[1], outer[2], &BLUE);

    let inner = [
        Pixel::new(c.x + 44, c.y + 108),
        Pixel::new(c.x + 86, c.y + 56),
        Pixel::new(c.x + 128, c.y + 108),
    ];
    r.fill_triangle(
        inner[0],
        inner[1],
        inner[2],
        &Color::with_alpha(255, 0, 0, 130),
    );
    r.draw_triangle(inner[0], inner[1], inner[2], &RED);
    note(r, c, NOTE_DY, "FILL + STROKE + ALPHA LAYER");
}

// ── 04. draw_rect / fill_rect ──

/// 三层矩形：实心、描边、半透明。
fn cell_rect(r: &mut Renderer, c: Pixel) {
    r.fill_rect(Pixel::new(c.x + 4, c.y + 6), 120, 38, GREEN);
    r.draw_rect(Pixel::new(c.x + 4, c.y + 6), 120, 38, &WHITE);

    r.fill_rect(
        Pixel::new(c.x + 60, c.y + 40),
        120,
        38,
        Color::with_alpha(255, 255, 0, 120),
    );
    r.draw_rect(Pixel::new(c.x + 60, c.y + 40), 120, 38, &YELLOW);

    // 1×1 与细长条，验证边界情形。
    r.fill_rect(Pixel::new(c.x + 186, c.y + 6), 1, 1, WHITE);
    r.fill_rect(Pixel::new(c.x + 4, c.y + 88), 180, 2, GRAY);
    note(r, c, NOTE_DY, "RECT 1PX AND 2PX TALL");
}

// ── 05. draw_circle / fill_circle ──

/// 同心圆：实心 + 描边交替，半透明叠一层。
fn cell_circle(r: &mut Renderer, c: Pixel) {
    let mid = Pixel::new(c.x + 90, c.y + 58);
    r.fill_circle(mid, 56, BLUE);
    r.draw_circle(mid, 56, &WHITE);
    r.fill_circle(mid, 40, Color::with_alpha(255, 0, 0, 150));
    r.draw_circle(mid, 40, &RED);
    r.fill_circle(mid, 20, YELLOW);
    r.draw_circle(mid, 20, &BLACK);
    // 半径 0 静默不画；半径 1 应是单个像素。
    r.fill_circle(mid, 0, WHITE);
    r.fill_circle(Pixel::new(c.x + 186, c.y + 6), 1, WHITE);
    note(r, c, NOTE_DY, "CONCENTRIC + R0 + R1");
}

// ── 06. draw_arc ──

/// 四段不同跨度的圆弧轮廓，含跨 0° 与大跨度。
fn cell_arc_outline(r: &mut Renderer, c: Pixel) {
    let mid = Pixel::new(c.x + 90, c.y + 58);
    let arcs = [
        (56usize, 0u32, 90u32, RED),
        (44, 90, 200, YELLOW),
        (32, 200, 350, GREEN),
        (20, 350, 30, CYAN), // 跨 0°，且是 >180° 的大跨度
    ];
    for (rad, s, e, col) in arcs {
        r.draw_arc(
            mid,
            rad,
            Angle::from_degrees(s),
            Angle::from_degrees(e),
            &col,
        );
    }
    // 半开区间：0°→360° 的整圈弧在起点方向缺一格。
    r.draw_arc(mid, 11, Angle::ZERO, Angle::from_degrees(360), &WHITE);
    note(r, c, NOTE_DY, "WRAP 0 AND 360 HALF OPEN");
}

// ── 07. fill_arc ──

/// 四分之一扇形拼成整圆，中心再叠一个大跨度扇形。
fn cell_arc_fill(r: &mut Renderer, c: Pixel) {
    let mid = Pixel::new(c.x + 90, c.y + 58);
    r.fill_arc(mid, 56, Angle::ZERO, Angle::RIGHT_DOWN, RED);
    r.fill_arc(mid, 56, Angle::RIGHT_DOWN, Angle::DOWN_LEFT, GREEN);
    r.fill_arc(
        mid,
        56,
        Angle::DOWN_LEFT,
        Angle::LEFT_UP,
        Color::with_alpha(0, 0, 255, 150),
    );
    r.fill_arc(
        mid,
        56,
        Angle::LEFT_UP,
        Angle::ZERO,
        Color::with_alpha(255, 255, 0, 140),
    );
    // 大跨度扇形：0°→270°，缺口在右上。
    r.fill_arc(
        mid,
        28,
        Angle::ZERO,
        Angle::from_degrees(270),
        Color::with_alpha(255, 0, 255, 110),
    );
    note(r, c, NOTE_DY, "4 QUARTERS + 270 DEG");
}

// ── 08. alpha blend ──

/// 三层半透明矩形叠加，演示通道混合顺序。
fn cell_alpha(r: &mut Renderer, c: Pixel) {
    for (i, col) in [RED, GREEN, BLUE].into_iter().enumerate() {
        r.fill_rect(
            Pixel::new(c.x + 8 + i * 36, c.y + 12 + i * 20),
            120,
            46,
            Color::with_alpha(col.r, col.g, col.b, 150),
        );
    }
    // 不透明覆盖，确认混合后仍可正常直写。
    r.fill_rect(Pixel::new(c.x + 120, c.y + 80), 60, 26, WHITE);
    note(r, c, NOTE_DY, "A150 RGB STACK");
}

// ── 09. blit ──

/// 区域拷贝。`blit` 是 1:1 拷贝（不做缩放），故按不同源尺寸各贴一块。
fn cell_blit(r: &mut Renderer, c: Pixel) {
    let small = bmp::make_checkerboard(8, 8, 2, 2, MAGENTA, BLACK, PixelFormat::Bgr24);
    let checker = bmp::make_checkerboard(4, 4, 8, 6, CYAN, BLACK, PixelFormat::Bgr24);
    let grad = {
        let (w, h) = (16usize, 16usize);
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let v = (x * 255 / (w - 1)) as u8;
                let o = (y * w + x) * 3;
                data[o] = v; // B
                data[o + 1] = v / 2; // G
                data[o + 2] = 255 - v; // R
            }
        }
        data
    };

    r.blit(
        Pixel::new(c.x + 6, c.y + 6),
        16,
        16,
        PixelFormat::Bgr24,
        &small,
        16 * 3,
    );
    r.blit(
        Pixel::new(c.x + 30, c.y + 6),
        32,
        24,
        PixelFormat::Bgr24,
        &checker,
        32 * 3,
    );
    r.blit(
        Pixel::new(c.x + 70, c.y + 6),
        16,
        16,
        PixelFormat::Bgr24,
        &grad,
        16 * 3,
    );
    // 故意贴到格子右缘，验证越界部分被自动裁掉（不 panic、不溢出）。
    r.blit(
        Pixel::new(c.x + 100, c.y + 52),
        32,
        24,
        PixelFormat::Bgr24,
        &checker,
        32 * 3,
    );
    // 再贴一次覆盖上一块，验证重复 blit 会覆盖而非叠加。
    r.blit(
        Pixel::new(c.x + 100, c.y + 78),
        16,
        16,
        PixelFormat::Bgr24,
        &small,
        16 * 3,
    );
    note(r, c, NOTE_DY, "CLIPPED AT RIGHT EDGE");
}

// ── 10. blit_mask ──

/// 径向渐变掩码画柔和圆斑；再用一个硬边掩码画方块。
fn cell_blit_mask(r: &mut Renderer, c: Pixel) {
    const MW: usize = 80;
    const MH: usize = 80;
    let mut soft = vec![0u8; MW * MH];
    for y in 0..MH {
        for x in 0..MW {
            let dx = (x as f32 - MW as f32 / 2.0).abs();
            let dy = (y as f32 - MH as f32 / 2.0).abs();
            let dist = (dx * dx + dy * dy).sqrt();
            soft[y * MW + x] = (255.0 * (1.0 - dist / 40.0).clamp(0.0, 1.0)) as u8;
        }
    }
    r.blit_mask(Pixel::new(c.x + 4, c.y + 4), MW, MH, &soft, MW, YELLOW);

    // 硬边掩码：只点亮左半边。
    let mut half = vec![0u8; MW * MH];
    for y in 0..MH {
        for x in 0..(MW / 2) {
            half[y * MW + x] = 255;
        }
    }
    r.blit_mask(Pixel::new(c.x + 90, c.y + 4), MW, MH, &half, MW, GREEN);
    note(r, c, NOTE_DY, "RADIAL FALLOFF + HARD MASK");
}

// ── 11. sub view ──

/// 第一阶段占位：真正的绘制在 `main` 的第二阶段完成（见那里的注释）。
fn cell_placeholder_subview(_r: &mut Renderer, _c: Pixel) {}

// ── 12. pixel format ──

/// 8 种像素格式各起一个小画布，用完全相同的调用序列画同一图案，并排贴回。
///
/// 这是**唯一**能看出「通道顺序写错了会变成什么颜色」的方式：单看一幅图永远
/// 发现不了红蓝互换。每格下方标出格式名（`RR…` = 红蓝位段互换）。
fn cell_pixel_format(r: &mut Renderer, c: Pixel) {
    const TW: usize = 44;
    const TH: usize = 34;
    /// 列距必须 ≥ 6 字符标签（`RGB565`）的 48 px，再留 8 px 余量。
    const COL_PITCH: usize = 56;
    let formats: [(PixelFormat, &str); 8] = [
        (PixelFormat::Rgb24, "RGB24"),
        (PixelFormat::Bgr24, "BGR24"),
        (PixelFormat::Xrgb8888, "XRGB"),
        (PixelFormat::Argb8888, "ARGB"),
        (PixelFormat::Rgba8888, "RGBA"),
        (PixelFormat::Bgra8888, "BGRA"),
        (PixelFormat::Rgb565, "RGB565"),
        (PixelFormat::Bgr565, "BGR565"),
    ];
    let mut scratch = [0u8; 8 * GLYPH_H];
    // 四列 × 两行：标签压在小图**下方**（1 倍点阵行高 16），
    // 两行合计 2 × (34 + 16) = 100 px，正好落在内容区 128 px 内。
    for (i, &(format, name)) in formats.iter().enumerate() {
        let x = c.x + (i % 4) * COL_PITCH;
        let y = c.y + (i / 4) * (TH + 17);
        let mut buf = vec![0u8; TW * TH * format.bytes_per_pixel()];
        {
            let info =
                FramebufferInfo::new(TW, TH, TW * format.bytes_per_pixel(), format);
            let mut sub = Renderer::new(info, &mut buf);
            sub.clear(BLACK);
            // 同一图案：红条 / 绿条 / 蓝条 / 半透明黄圆。
            sub.fill_rect(Pixel::new(0, 2), TW, 8, RED);
            sub.fill_rect(Pixel::new(0, 12), TW, 8, GREEN);
            sub.fill_rect(Pixel::new(0, 22), TW, 8, BLUE);
            sub.fill_circle(
                Pixel::new(TW / 2, 17),
                6,
                Color::with_alpha(255, 255, 0, 128),
            );
        }
        blit_converted(
            r,
            Pixel::new(x, y),
            TW,
            TH,
            format,
            &buf,
            TW * format.bytes_per_pixel(),
        );
        r.draw_text(
            Pixel::new(x, y + TH),
            name,
            TITLE_PX,
            NOTE_FG,
            &DEFAULT_FONT,
            &mut scratch,
        );
    }
    note(r, c, NOTE_DY, "SAME PATTERN, 8 FORMATS");
}

// ── 13. font bitmap ──

/// 内置 8×16 点阵字体：整行 + 逐字。
fn cell_font_bitmap(r: &mut Renderer, c: Pixel) {
    let mut scratch = [0u8; 8 * GLYPH_H];
    // 整行：draw_text 自动对齐基线并用 PenX 推进画笔。
    r.draw_text(
        Pixel::new(c.x + 6, c.y + 4),
        "RASTERON 123",
        TITLE_PX,
        WHITE,
        &DEFAULT_FONT,
        &mut scratch,
    );
    // 逐字：draw_char 每字独立指定基线原点。
    for (i, ch) in ['A', 'B', 'g', 'j', '|', '?'].into_iter().enumerate() {
        r.draw_char(
            Pixel::new(c.x + 6 + i * 20, c.y + 44),
            ch,
            TITLE_PX,
            Color::new(200, 210, 230),
            Some(&DEFAULT_FONT),
            &mut scratch,
        );
    }
    // 多行 + 换行：draw_text 遇 `\n` 重置画笔并下移一个行高。
    r.draw_text(
        Pixel::new(c.x + 6, c.y + 66),
        "LINE 1\nLINE 2",
        TITLE_PX,
        YELLOW,
        &DEFAULT_FONT,
        &mut scratch,
    );
    note(r, c, NOTE_DY, "DRAW TEXT + DRAW CHAR");
}

// ── 14-17. 字体排版（TTF fixture）──

/// 14. TrueType 字体在多个字号下的排版，每行都画一条参考基线。
fn cell_font_ttf(r: &mut Renderer, c: Pixel) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut scratch = [0u8; TTF_SCRATCH];
    for (i, px) in [12.0f32, 18.0, 24.0, 30.0].into_iter().enumerate() {
        // 基线 y 直接给定：`draw_text` 的 top_left.y 换算成首行基线是
        // `y + ascent`，故此处按字号反推，保证四条基线等距可读。
        let ascent = font.metrics(px).ascent;
        let top = c.y + 6 + i * 28;
        let baseline = top + ascent as usize;
        r.draw_line(
            Pixel::new(c.x + 4, baseline),
            Pixel::new(c.x + 170, baseline),
            &RULE_FG,
        );
        r.draw_text(
            Pixel::new(c.x + 6, top),
            TTF_SAMPLE,
            px,
            [CYAN, YELLOW, MAGENTA, GREEN][i],
            &font,
            &mut scratch,
        );
    }
    note(r, c, NOTE_DY, "TTF 12 18 24 30 PX");
}

/// 15. 同一基线上画不同字号的 `AWg`：基线横线贯穿，`A` 的底应落在同一条线上。
fn cell_text_align(r: &mut Renderer, c: Pixel) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut scratch = [0u8; TTF_SCRATCH];
    let baseline = c.y + 88;
    r.draw_line(
        Pixel::new(c.x + 4, baseline),
        Pixel::new(c.x + 190, baseline),
        &WHITE,
    );
    for (i, px) in [16.0f32, 24.0, 32.0].into_iter().enumerate() {
        // top_left.y 需回推 ascent，首行基线才正好落在上面那条横线上。
        let top = baseline as f32 - font.metrics(px).ascent;
        r.draw_text(
            Pixel::new(c.x + 6 + i * 62, top as usize),
            "AWg",
            px,
            [RED, GREEN, BLUE][i],
            &font,
            &mut scratch,
        );
    }
    note(r, c, NOTE_DY, "SHARED BASELINE 16 24 32");
}

/// 16. `\n` 换行 + 行距：每行基线画一条淡线。
fn cell_text_multiline(r: &mut Renderer, c: Pixel) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut scratch = [0u8; TTF_SCRATCH];
    const PX: f32 = 14.0;
    let top = c.y + 4;
    r.draw_text(
        Pixel::new(c.x + 6, top),
        "AWgij|\nAWgij|\nAWgij|",
        PX,
        Color::new(210, 220, 240),
        &font,
        &mut scratch,
    );
    // 行距刻度：首行基线 = top_left.y + ascent，其后每行 +行高。
    let metrics = font.metrics(PX);
    for row in 0..3 {
        let y = top + metrics.ascent as usize + row * metrics.line_height() as usize;
        r.draw_line(Pixel::new(c.x + 4, y), Pixel::new(c.x + 120, y), &RULE_FG);
    }
    note(r, c, NOTE_DY, "NEWLINE + LINE GAP");
}

/// 17. 可视化 `bearing_x` / `bearing_y`。
///
/// `j` 有**左悬挂**（`bearing_x` 为负），基线原点在字形左沿**右侧**——这正是
/// `draw_glyph` 必须裁源掩码左边界的原因。用基线十字标出原点，悬挂关系一眼可见。
fn cell_ttf_bearing(r: &mut Renderer, c: Pixel) {
    let font = TtfFont::new(MINIMAL_FONT);
    let mut scratch = [0u8; TTF_SCRATCH];
    for (i, ch) in ['A', 'W', 'g', 'i', 'j', '|'].into_iter().enumerate() {
        let gx = c.x + 6 + (i % 6) * 31;
        let gy = c.y + 6;
        r.draw_rect(Pixel::new(gx, gy), 28, 50, &BORDER);
        // 基线（横）与原点（纵）
        let base_y = gy + 38;
        r.draw_line(Pixel::new(gx, base_y), Pixel::new(gx + 28, base_y), &GRAY);
        r.draw_line(
            Pixel::new(gx + 14, gy),
            Pixel::new(gx + 14, gy + 50),
            &Color::new(110, 116, 134),
        );
        r.draw_char(
            Pixel::new(gx + 14, base_y),
            ch,
            28.0,
            WHITE,
            Some(&font),
            &mut scratch,
        );
    }
    note(r, c, NOTE_DY, "J HANGS LEFT OF ORIGIN");
}

/// 18. 缺字与替代字形：点阵字体只认 ASCII，非 ASCII 落到手绘的方框叉，
/// 且**绘制不中断**——整行继续按替代字形的固定宽度推进。
fn cell_missing_glyph(r: &mut Renderer, c: Pixel) {
    let mut scratch = [0u8; 8 * GLYPH_H];
    r.draw_text(
        Pixel::new(c.x + 4, c.y + 6),
        "A?",
        TITLE_PX,
        WHITE,
        &DEFAULT_FONT,
        &mut scratch,
    );
    for (i, ch) in ['A', '中', '?'].into_iter().enumerate() {
        r.draw_char(
            Pixel::new(c.x + 4 + i * 24, c.y + 44),
            ch,
            TITLE_PX,
            if i == 1 {
                Color::new(255, 170, 90)
            } else {
                WHITE
            },
            Some(&DEFAULT_FONT),
            &mut scratch,
        );
    }
    note(r, c, NOTE_DY, "FALLBACK GLYPH, NO ABORT");
}

// ── 19-22. 可编程管线 ──

/// 自定义片元着色器：把 varying 携带的颜色直接写出去。
///
/// `draw_arrays` 持 `&mut Renderer` 时只能持 `&Pipeline`，故片元着色器的可调
/// 状态必须存在 `&self` 里；这里它是无状态单元类型。
struct VaryingFs;

impl FragmentShader for VaryingFs {
    type Varying = Color;

    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
        FragmentOutput::Color(input.varying)
    }
}

/// 19. `Points` 与 `Lines` 对比：同一份点阵，颜色随行列渐变。
fn cell_pipeline_points(r: &mut Renderer, c: Pixel) {
    const LATTICE: usize = 8;
    let step = 1.0 / (LATTICE - 1) as f32;
    let points: Vec<Vertex2D<Color>> = (0..LATTICE)
        .flat_map(|row| {
            (0..LATTICE).map(move |col| {
                let u = col as f32 * step;
                let v = row as f32 * step;
                Vertex2D {
                    // 留边距，点阵不贴视口边缘。
                    position: [0.1 + u * 0.8, 0.1 + v * 0.8],
                    varying: Color::new((u * 255.0) as u8, 170, (v * 255.0) as u8),
                }
            })
        })
        .collect();
    // 同一份点阵两种装配方式：左块 `Points` 只画 64 个孤立像素，
    // 右块 `Lines` 把相邻顶点连成 56 条线段。
    for (i, primitive) in [PrimitiveType::Points, PrimitiveType::Lines]
        .into_iter()
        .enumerate()
    {
        r.draw_arrays(
            primitive,
            &points,
            &Pipeline::new(
                Passthrough2D::new(),
                VaryingFs,
                PipelineState::alpha_blend(Viewport::new(c.x + i * 100, c.y, 92, 92)),
            ),
        )
        .expect("顺序绘制没有索引缓冲，不可能失败");
    }
    note(r, c, NOTE_DY, "POINTS LEFT, LINES RIGHT");
}

/// 20. 内建 `GradientFs`：横向 `[1, 0]` 与对角 `[0.5, 0.5]`。
///
/// `dir` 的长度即渐变速率，不是只表方向：横向要走完全宽就得给 `[1, 0]`；
/// 对角方向 normalized 空间的 t 峰值是 `|dx| + |dy|`，故 `[0.5, 0.5]` 恰好
/// 在右下角到头（给 `[1, 1]` 会让渐变在中点就饱和）。
fn cell_pipeline_gradient(r: &mut Renderer, c: Pixel) {
    let quad = [
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 1.0], varying: () },
    ];
    r.draw_arrays(
        PrimitiveType::Triangles,
        &quad,
        &Pipeline::new(
            Passthrough2D::new(),
            GradientFs::new(Color::new(20, 20, 40), Color::new(240, 120, 60), [1.0, 0.0]),
            PipelineState::alpha_blend(Viewport::new(c.x, c.y, 188, 42)),
        ),
    )
    .expect("顺序绘制没有索引缓冲，不可能失败");
    r.draw_arrays(
        PrimitiveType::Triangles,
        &quad,
        &Pipeline::new(
            Passthrough2D::new(),
            GradientFs::new(BLUE, WHITE, [0.5, 0.5]),
            PipelineState::alpha_blend(Viewport::new(c.x, c.y + 48, 188, 42)),
        ),
    )
    .expect("顺序绘制没有索引缓冲，不可能失败");
    note(r, c, NOTE_DY, "DIR 1,0 AND 0.5,0.5");
}

/// 21. UV 采样纹理：`Clamp` 边缘拉伸 vs `Repeat` 平铺，各画一块。
fn cell_pipeline_texture(r: &mut Renderer, c: Pixel) {
    const TW: usize = 8;
    const TH: usize = 8;
    let texels = bmp::make_checkerboard(4, 4, 2, 2, MAGENTA, BLACK, PixelFormat::Bgr24);
    let texture = Texture2D::new(TW, TH, &texels, PixelFormat::Bgr24);

    // UV 故意超出 [0, 1]，两种寻址方式的差别才看得出来。
    let quad = [
        Vertex2D { position: [0.0, 0.0], varying: [0.0f32, 0.0f32] },
        Vertex2D { position: [1.0, 0.0], varying: [2.0f32, 0.0f32] },
        Vertex2D { position: [1.0, 1.0], varying: [2.0f32, 2.0f32] },
        Vertex2D { position: [0.0, 0.0], varying: [0.0f32, 0.0f32] },
        Vertex2D { position: [1.0, 1.0], varying: [2.0f32, 2.0f32] },
        Vertex2D { position: [0.0, 1.0], varying: [0.0f32, 2.0f32] },
    ];
    let modes = [
        (FilterMode::Nearest, WrapMode::Clamp, "CLAMP STRETCH"),
        (FilterMode::Bilinear, WrapMode::Repeat, "REPEAT TILE"),
    ];
    for (i, (filter, wrap, label)) in modes.into_iter().enumerate() {
        let x0 = c.x + i * 96;
        r.draw_arrays(
            PrimitiveType::Triangles,
            &quad,
            &Pipeline::new(
                Passthrough2D::<Vec2>::new(),
                BuiltinTextureFs::new(texture, filter, wrap),
                PipelineState::alpha_blend(Viewport::new(x0, c.y, 90, 90)),
            ),
        )
        .expect("顺序绘制没有索引缓冲，不可能失败");
        // 采样探针：把 `Texture2D` 便捷入口的单点结果也画出来。
        let probe = match filter {
            FilterMode::Nearest => texture.sample_nearest([0.25, 0.25]),
            FilterMode::Bilinear => texture.sample_bilinear([0.5, 0.5]),
        };
        r.fill_rect(Pixel::new(x0, c.y + 92), 24, 8, probe);
        // 12 字符 × 8 px = 96 px 正好一列宽，故每列只放一行标签，
        // 不再在格底重复一遍——否则两行说明会叠在 128 px 的内容区里。
        note(r, Pixel::new(x0, c.y), 102, label);
    }
}

/// 22. 混合模式对比：左侧 `BlendMode::Add`，右侧 `BlendMode::None` + scissor。
fn cell_pipeline_add_blend(r: &mut Renderer, c: Pixel) {
    // 三个三角形两两交叠，Add 下交叠区更亮；None 下只剩最后写入的片元。
    let verts = [
        Vertex2D { position: [0.15, 0.15], varying: Color::with_alpha(255, 40, 40, 170) },
        Vertex2D { position: [0.85, 0.45], varying: Color::with_alpha(40, 255, 90, 170) },
        Vertex2D { position: [0.50, 0.90], varying: Color::with_alpha(50, 90, 255, 170) },
    ];
    r.draw_arrays(
        PrimitiveType::Triangles,
        &verts,
        &Pipeline::new(
            Passthrough2D::new(),
            VaryingFs,
            PipelineState {
                blend: BlendMode::Add,
                scissor: None,
                viewport: Viewport::new(c.x, c.y, 140, 92),
            },
        ),
    )
    .expect("顺序绘制没有索引缓冲，不可能失败");
    // 同一份几何，叠加 `BlendMode::None`：应只剩最后写入的片元（第三片元无重叠，
    // 故与第一次绘制完全相同）——对比即说明 Add 才是「叠加」、None 是「替换」。
    r.draw_arrays(
        PrimitiveType::Triangles,
        &verts,
        &Pipeline::new(
            Passthrough2D::new(),
            VaryingFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: Some(ScissorRect::new(c.x + 156, c.y, 44, 44)),
                viewport: Viewport::new(c.x + 150, c.y, 140, 92),
            },
        ),
    )
    .expect("顺序绘制没有索引缓冲，不可能失败");
    note(r, c, NOTE_DY, "ADD LEFT VS NONE RIGHT");
}

// ── 23-24. 像素格式 ──

/// 23. RGB565 量化：同一渐变在 565 与 24bpp 下的色带差异。
fn cell_format_565(r: &mut Renderer, c: Pixel) {
    const TW: usize = 190;
    const TH: usize = 40;
    /// 在一个临时小画布上画色带，返回其原始字节。
    fn band(format: PixelFormat) -> Vec<u8> {
        let mut buf = vec![0u8; TW * TH * format.bytes_per_pixel()];
        {
            let info =
                FramebufferInfo::new(TW, TH, TW * format.bytes_per_pixel(), format);
            let mut sub = Renderer::new(info, &mut buf);
            sub.clear(BLACK);
            // 渐变经过 5/6/5 量化——量化误差在这条带上最明显。
            for i in 0..32 {
                let v = (i * 8) as u8;
                sub.fill_rect(Pixel::new(i * TW / 32, 4), TW / 32 + 1, 12, Color::new(v, 255 - v, v));
            }
            sub.fill_rect(Pixel::new(0, 22), TW, 14, Color::new(255, 128, 0));
        }
        buf
    }
    // blit 要求格式匹配，故 565 那份先逐像素读回再转成 Bgr24 贴上。
    let raw565 = band(PixelFormat::Bgr565);
    let mut flat = vec![0u8; TW * TH * 3];
    for y in 0..TH {
        for x in 0..TW {
            let off = (y * TW + x) * 2;
            let col = PixelFormat::Bgr565.read_color(&raw565[off..off + 2]);
            let d = (y * TW + x) * 3;
            flat[d] = col.b;
            flat[d + 1] = col.g;
            flat[d + 2] = col.r;
        }
    }
    blit_converted(
            r,
            Pixel::new(c.x, c.y),
            TW,
            TH,
            PixelFormat::Bgr24,
            &flat,
            TW * 3,
        );
    // 左侧一条逐级色阶，看量化台阶。
    for i in 0..8 {
        let v = (i * 36) as u8;
        r.fill_rect(Pixel::new(c.x, c.y + 50 + i * 8), 50, 8, Color::new(v, 255 - v, v));
    }
    note(r, c, NOTE_DY, "BGR565 5 6 5 QUANTIZE");
}

/// 24. 4bpp 家族的通道布局：同一图案在 4 种 32 位格式下的取色结果。
fn cell_format_32bpp(r: &mut Renderer, c: Pixel) {
    const TW: usize = 46;
    const TH: usize = 52;
    let formats: [(PixelFormat, &str); 4] = [
        (PixelFormat::Xrgb8888, "XRGB"),
        (PixelFormat::Argb8888, "ARGB"),
        (PixelFormat::Rgba8888, "RGBA"),
        (PixelFormat::Bgra8888, "BGRA"),
    ];
    let mut scratch = [0u8; 8 * GLYPH_H];
    for (i, &(format, name)) in formats.iter().enumerate() {
        let x = c.x + i * (TW + 2);
        let mut buf = vec![0u8; TW * TH * format.bytes_per_pixel()];
        {
            let info = FramebufferInfo::new(TW, TH, TW * format.bytes_per_pixel(), format);
            let mut sub = Renderer::new(info, &mut buf);
            sub.clear(Color::new(12, 12, 16));
            // 半透明色在有无 alpha 通道的格式下落盘结果不同。
            sub.fill_rect(
                Pixel::new(2, 2),
                TW - 4,
                13,
                Color::with_alpha(255, 90, 60, 140),
            );
            sub.fill_rect(Pixel::new(2, 17), TW - 4, 11, GREEN);
            sub.fill_circle(
                Pixel::new(TW / 2, 39),
                8,
                Color::with_alpha(60, 200, 255, 200),
            );
            // 读回一枚半透明色，验证 round-trip 后通道顺序自洽。
            if let Some(got) = sub.get_pixel(TW - 6, 8) {
                sub.fill_rect(Pixel::new(TW - 10, TH - 10), 8, 8, got);
            }
        }
        blit_converted(
            r,
            Pixel::new(x, c.y + 2),
            TW,
            TH,
            format,
            &buf,
            TW * format.bytes_per_pixel(),
        );
        r.draw_text(
            Pixel::new(x, c.y + 2 + TH),
            name,
            TITLE_PX,
            NOTE_FG,
            &DEFAULT_FONT,
            &mut scratch,
        );
    }
    note(r, c, NOTE_DY, "ALPHA CHANNEL PRESENT OR NOT");
}

/// 25. 跨度填充：长跨度、跨对齐边界、极细跨度与重复覆盖。
fn cell_span_fill(r: &mut Renderer, c: Pixel) {
    // 半透明长跨度，起点故意不对齐。
    r.fill_rect(
        Pixel::new(c.x + 3, c.y + 6),
        180,
        16,
        Color::with_alpha(70, 120, 220, 170),
    );
    r.fill_rect(
        Pixel::new(c.x + 16, c.y + 28),
        150,
        13,
        Color::new(240, 190, 60),
    );
    // 1×N 与 N×1 的极细跨度。
    r.fill_rect(Pixel::new(c.x + 170, c.y + 28), 1, 13, WHITE);
    r.fill_rect(Pixel::new(c.x + 4, c.y + 48), 178, 1, GRAY);
    // 不透明色反复覆盖同一片区域：结果与写一次相同（直写不叠加）。
    for _ in 0..3 {
        r.fill_rect(
            Pixel::new(c.x + 20, c.y + 58),
            120,
            16,
            Color::new(60, 90, 180),
        );
    }
    note(r, c, NOTE_DY, "REPEAT WRITE IS IDEMPOTENT");
}

/// 26. `get_pixel` 读回：把读到的一列像素复制到另一列，视觉上应完全一致。
fn cell_get_pixel(r: &mut Renderer, c: Pixel) {
    for x in 0..40 {
        let v = (x * 6) as u8;
        r.fill_rect(
            Pixel::new(c.x + 8 + x, c.y + 8),
            1,
            56,
            Color::new(v, 255 - v, 128),
        );
    }
    r.draw_rect(Pixel::new(c.x + 8, c.y + 8), 40, 56, &WHITE);
    for y in 0..56 {
        if let Some(got) = r.get_pixel(c.x + 8 + (y * 7 % 40), c.y + 8 + y) {
            r.set_pixel(c.x + 60 + y, c.y + 8 + y, &got);
        }
    }
    r.draw_rect(Pixel::new(c.x + 60, c.y + 8), 56, 56, &GRAY);
    note(r, c, NOTE_DY, "READ BACK THEN REWRITE");
}

/// 27. 帧协议：每帧**先 `clear` 后 `swap`**。
///
/// 「前/后缓冲」在这里只能靠两条泳道示意——真的调 `clear` 会把本帧已画好的内容
/// 一并抹掉（`clear` 清的是**正在绘制的那块**缓冲，即后台）。`clear` 的真实
/// 作用域演示在第 11 格：子视口的 `clear` 只清自己那一小块窗口，不碰父画面。
fn cell_clear_swap(r: &mut Renderer, c: Pixel) {
    let bars = [RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA];
    // 上排：正在绘制（后台），满亮度。
    for (i, col) in bars.into_iter().enumerate() {
        r.fill_rect(Pixel::new(c.x + 8 + i * 30, c.y + 26), 26, 28, col);
    }
    // 下排：仍在屏上的上一帧（前台），同色但压暗。
    for (i, col) in bars.into_iter().enumerate() {
        r.fill_rect(
            Pixel::new(c.x + 8 + i * 30, c.y + 58),
            26,
            22,
            Color::with_alpha(col.r, col.g, col.b, 70),
        );
    }
    r.draw_line(
        Pixel::new(c.x + 4, c.y + 53),
        Pixel::new(c.x + 188, c.y + 53),
        &RULE_FG,
    );
    note(r, c, 4, "BACK ABOVE FRONT BELOW");
    note(r, c, NOTE_DY, "CLEAR FIRST SWAP LAST");
}

/// 28. 帧缓冲属性与色卡。
fn cell_info(r: &mut Renderer, c: Pixel) {
    let info = r.info();
    println!(
        "  info: {}x{} pitch={} bpp={} buffer={} bytes  format={:?}",
        info.width,
        info.height,
        info.pitch,
        info.bytes_per_pixel(),
        info.buffer_size(),
        info.format
    );
    for (i, col) in [RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA, WHITE, GRAY]
        .into_iter()
        .enumerate()
    {
        r.fill_rect(Pixel::new(c.x + 8 + i * 23, c.y + 12), 20, 54, col);
    }
    r.draw_rect(Pixel::new(c.x + 4, c.y + 6), 190, 66, &GRAY);
    note(r, c, NOTE_DY, "PITCH BPP BUFFER SIZE");
}