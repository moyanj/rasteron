//! gallery — 全图元展示示例。
//!
//! 把 rasteron 的**全部**固定功能图元排成一张带编号的网格画布，每个图元占
//! 一格，方便在输出图里一眼定位。既是示例，也是人工比对渲染结果的基准图。
//!
//! 运行:
//!   cargo run --example gallery
//!
//! 产出 `gallery-example.bmp`（BGR24，960×760）。

use rasteron::Renderer;
use rasteron::angle::Angle;
use rasteron::color::{BLACK, BLUE, CYAN, Color, GRAY, GREEN, MAGENTA, RED, WHITE, YELLOW};
use rasteron::framebuffer::{FramebufferInfo, PixelFormat};
use rasteron::pixel::Pixel;
use rasteron::utils::sub_view;

#[path = "../shared/bmp.rs"]
mod bmp;

/// 画布宽。四列均分。
const WIDTH: usize = 960;
/// 每格宽度。
const CELL_W: usize = WIDTH / 4;
/// 每格内容区高度。
const CELL_H: usize = 170;
/// 标题占用的额外高度。5x7 字模放大 2 倍后是 14px，再留 4px 上下边距。
const TITLE_H: usize = 20;
/// 格子间距。
const GAP: usize = 8;
/// 行数（12 格 = 3 行 × 4 列）。
const ROWS: usize = 3;
/// 画布高 = 行数 × 每格总高 + 底部留白。
const HEIGHT: usize = ROWS * (CELL_H + TITLE_H) + GAP;

/// 单元格左上角（父帧缓冲坐标系）。
fn cell_origin(col: usize, row: usize) -> Pixel {
    Pixel::new(col * CELL_W + GAP, row * (CELL_H + TITLE_H) + GAP)
}

/// 单元格边框 + 标题条，返回内容区左上角。
fn draw_frame(r: &mut Renderer, origin: Pixel, index: usize, name: &str) -> Pixel {
    let w = CELL_W - 2 * GAP;
    let h = CELL_H + TITLE_H - GAP;
    r.draw_rect(origin, w, h, &GRAY);
    // 标题条：左侧强调色块 + 5x7 点阵文字（2 倍放大）
    let accent = ACCENTS[index % ACCENTS.len()];
    r.fill_rect(
        Pixel::new(origin.x + 1, origin.y + 1),
        4,
        TITLE_H - 2,
        accent,
    );
    // 标题在标题条内垂直居中：字模高 14，标题条高 TITLE_H
    let text_y = origin.y + (TITLE_H - GLYPH_H * SCALE) / 2;
    draw_text(r, Pixel::new(origin.x + 9, text_y), name, WHITE);
    // 内容区左上角（标题条下方）
    Pixel::new(origin.x + GAP, origin.y + TITLE_H)
}

/// 每格左侧强调色。
const ACCENTS: [Color; 6] = [RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA];

fn main() {
    let info = FramebufferInfo::new(WIDTH, HEIGHT, WIDTH * 3, PixelFormat::Bgr24);
    let mut fb = vec![0u8; info.buffer_size()];

    // ── 第一阶段：主渲染器 ──
    // `sub_view` 需要父缓冲的裸切片，而 `Renderer` 在存活期间独占它，
    // 所以子渲染器那一格必须放到第二阶段（作用域结束、缓冲区归还之后）。
    {
        let mut r = Renderer::new(
            FramebufferInfo::new(WIDTH, HEIGHT, WIDTH * 3, PixelFormat::Bgr24),
            &mut fb,
        );
        r.clear(Color::new(24, 24, 30));

        // (标题, 绘制函数)。标题自带编号，字库转大写后绘制。
        let cells: [(&str, CellFn); 12] = [
            ("1 set_pixel", cell_set_pixel),
            ("2 draw_line", cell_line),
            ("3 triangle", cell_triangle),
            ("4 rect", cell_rect),
            ("5 circle", cell_circle),
            ("6 arc outline", cell_arc_outline),
            ("7 arc sector", cell_arc_fill),
            ("8 alpha blend", cell_alpha),
            ("9 blit", cell_blit),
            ("10 blit_mask", cell_blit_mask),
            ("11 sub_view", cell_placeholder_subview),
            ("12 info", cell_info),
        ];

        for (i, &(name, draw)) in cells.iter().enumerate() {
            let origin = cell_origin(i % 4, i / 4);
            // `i` 决定左侧强调色，避免每格颜色雷同
            let content = draw_frame(&mut r, origin, i, name);
            draw(&mut r, content);
        }

        #[cfg(feature = "double_buffer")]
        r.swap_buffers();
    }

    // ── 第二阶段：子渲染器 ──
    // 现在 `fb` 已归还，可以按切片构造受限渲染器。
    {
        let origin = cell_origin(2, 2); // 第 11 格
        // 窗口必须落在**内容区**（标题条下方），否则面板的 clear 会把
        // 第一阶段画好的标题与边框一并抹掉。
        let panel_x = origin.x + GAP;
        let panel_y = origin.y + TITLE_H;
        let panel_w = CELL_W - 2 * GAP - 8;
        let panel_h = CELL_H - 12;
        let Some(mut panel) = sub_view(&mut fb, &info, panel_x, panel_y, panel_w, panel_h) else {
            panic!("sub_view 未能构造子渲染器");
        };
        let (pw, ph) = (panel.info().width, panel.info().height);

        // 子渲染器内坐标从 (0,0) 开始。
        panel.clear(Color::new(34, 34, 44));

        // 关键演示：圆心故意放在窗口**左外侧**，半径又大到足以跨过整窗，
        // 于是窗口内只应看到一条弧边——超出的部分被自然裁掉。
        let outside = Pixel::new(pw / 4, ph / 2);
        panel.fill_circle(outside, pw, BLUE);
        panel.draw_circle(outside, pw, &WHITE);

        // 窗口内再叠一个适中的扇形与矩形，验证子渲染器内各图元都正常
        panel.fill_arc(
            Pixel::new(pw * 2 / 3, ph / 2),
            pw / 4,
            Angle::from_degrees(90),
            Angle::from_degrees(210),
            Color::with_alpha(255, 255, 0, 150),
        );
        panel.fill_rect(
            Pixel::new(pw / 2, ph - 26),
            pw / 2,
            18,
            Color::with_alpha(0, 255, 0, 160),
        );
        // 边框画在最后，确保可见（否则会被前面的填充覆盖）
        panel.draw_rect(Pixel::new(0, 0), pw, ph, &GRAY);

        #[cfg(feature = "double_buffer")]
        panel.swap_buffers();
    }

    // ── 第三阶段：写文件 ──
    bmp::write_bmp(
        std::path::Path::new("gallery-example.bmp"),
        WIDTH,
        HEIGHT,
        PixelFormat::Bgr24,
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

/// 单格绘制函数签名：内容区左上角，尺寸由调用方保证不越界。
type CellFn = fn(&mut Renderer, Pixel);

// ── 1. set_pixel / get_pixel ──

/// 逐像素写出一块棋盘，再用 `get_pixel` 读回中心像素并改白。
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
    // 读回 → 覆盖为白，展示 get_pixel 的往返
    let got = r.get_pixel(c.x + 70, c.y + 35);
    assert!(got.is_some(), "格内像素应可读回");
    r.set_pixel(c.x + 70, c.y + 35, &WHITE);
}

// ── 2. draw_line / draw_line_unchecked ──

/// 扇形展开的线束；底部一条用 unchecked 版本画，二者行为一致。
fn cell_line(r: &mut Renderer, c: Pixel) {
    for i in 0..14 {
        let x0 = c.x + 2 + i * 10;
        // 两种绿蓝交替，`draw_line` 收 `&Color`，故借用
        let color = if i % 2 == 0 { &GREEN } else { &BLUE };
        r.draw_line(
            Pixel::new(x0, c.y + 160),
            Pixel::new(x0, c.y + 4 + (i * 17) % 120),
            color,
        );
    }
    // 安全/非安全版本各画一条水平线，视觉上应一致
    r.draw_line(
        Pixel::new(c.x + 4, c.y + 152),
        Pixel::new(c.x + 140, c.y + 152),
        &CYAN,
    );
    // SAFETY: 端点均在格内且由 `draw_line_unchecked` 自行裁剪，无需调用方保证
    unsafe {
        r.draw_line_unchecked(
            Pixel::new(c.x + 4, c.y + 160),
            Pixel::new(c.x + 140, c.y + 160),
            &CYAN,
        );
    }
}

// ── 3. draw_triangle / fill_triangle ──

/// 描边三角 + 实心三角 + 半透明三角叠加。
fn cell_triangle(r: &mut Renderer, c: Pixel) {
    let outer = [
        Pixel::new(c.x + 6, c.y + 156),
        Pixel::new(c.x + 70, c.y + 10),
        Pixel::new(c.x + 134, c.y + 156),
    ];
    r.draw_triangle(outer[0], outer[1], outer[2], &WHITE);
    r.fill_triangle(outer[0], outer[1], outer[2], &BLUE);

    let inner = [
        Pixel::new(c.x + 44, c.y + 156),
        Pixel::new(c.x + 86, c.y + 62),
        Pixel::new(c.x + 128, c.y + 156),
    ];
    r.fill_triangle(
        inner[0],
        inner[1],
        inner[2],
        &Color::with_alpha(255, 0, 0, 130),
    );
    r.draw_triangle(inner[0], inner[1], inner[2], &RED);
}

// ── 4. draw_rect / fill_rect ──

/// 三层矩形：实心、描边、半透明。
fn cell_rect(r: &mut Renderer, c: Pixel) {
    r.fill_rect(Pixel::new(c.x + 4, c.y + 8), 120, 56, GREEN);
    r.draw_rect(Pixel::new(c.x + 4, c.y + 8), 120, 56, &WHITE);

    r.fill_rect(
        Pixel::new(c.x + 60, c.y + 58),
        120,
        56,
        Color::with_alpha(255, 255, 0, 120),
    );
    r.draw_rect(Pixel::new(c.x + 60, c.y + 58), 120, 56, &YELLOW);

    // 1x1 与细长条，验证边界情形
    r.fill_rect(Pixel::new(c.x + 140, c.y + 8), 1, 1, WHITE);
    r.fill_rect(Pixel::new(c.x + 4, c.y + 130), 180, 2, GRAY);
}

// ── 5. draw_circle / fill_circle ──

/// 同心圆：实心 + 描边交替，半透明叠一层。
fn cell_circle(r: &mut Renderer, c: Pixel) {
    let mid = Pixel::new(c.x + 90, c.y + 80);
    r.fill_circle(mid, 78, BLUE);
    r.draw_circle(mid, 78, &WHITE);
    r.fill_circle(mid, 56, Color::with_alpha(255, 0, 0, 150));
    r.draw_circle(mid, 56, &RED);
    r.fill_circle(mid, 30, YELLOW);
    r.draw_circle(mid, 30, &BLACK);
    // 半径 0：静默不画
    r.fill_circle(mid, 0, WHITE);
}

// ── 6. draw_arc ──

/// 四段不同跨度的圆弧轮廓，含跨 0° 与大跨度。
fn cell_arc_outline(r: &mut Renderer, c: Pixel) {
    let mid = Pixel::new(c.x + 90, c.y + 80);
    let arcs = [
        (78usize, 0u32, 90u32, RED),
        (62, 90, 200, YELLOW),
        (46, 200, 350, GREEN),
        (30, 350, 30, CYAN), // 跨 0°，且是 >180° 的大跨度
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
}

// ── 7. fill_arc ──

/// 四分之一扇形拼成整圆，中心再叠一个大跨度扇形。
fn cell_arc_fill(r: &mut Renderer, c: Pixel) {
    let mid = Pixel::new(c.x + 90, c.y + 80);
    r.fill_arc(mid, 78, Angle::ZERO, Angle::RIGHT_DOWN, RED);
    r.fill_arc(mid, 78, Angle::RIGHT_DOWN, Angle::DOWN_LEFT, GREEN);
    r.fill_arc(
        mid,
        78,
        Angle::DOWN_LEFT,
        Angle::LEFT_UP,
        Color::with_alpha(0, 0, 255, 150),
    );
    r.fill_arc(
        mid,
        78,
        Angle::LEFT_UP,
        Angle::ZERO,
        Color::with_alpha(255, 255, 0, 140),
    );
    // 大跨度扇形：0°→270°，缺口在右上
    r.fill_arc(
        mid,
        40,
        Angle::ZERO,
        Angle::from_degrees(270),
        Color::with_alpha(255, 0, 255, 110),
    );
}

// ── 8. alpha blend ──

/// 三层半透明矩形叠加，演示通道混合顺序。
fn cell_alpha(r: &mut Renderer, c: Pixel) {
    for (i, col) in [RED, GREEN, BLUE].into_iter().enumerate() {
        r.fill_rect(
            Pixel::new(c.x + 8 + i * 36, c.y + 20 + i * 30),
            120,
            60,
            Color::with_alpha(col.r, col.g, col.b, 150),
        );
    }
    // 不透明覆盖，确认混合后仍可正常直写
    r.fill_rect(Pixel::new(c.x + 120, c.y + 120), 60, 40, WHITE);
}

// ── 9. blit ──

/// 区域拷贝。`blit` 是 1:1 拷贝（不做缩放），故按不同源尺寸各贴一块。
fn cell_blit(r: &mut Renderer, c: Pixel) {
    // 三种源尺寸，对应贴到目标的不同大小
    let src_small = bmp::make_checkerboard(8, 8, 2, 2, MAGENTA, BLACK, PixelFormat::Bgr24);
    let src_mid = bmp::make_checkerboard(8, 8, 4, 3, CYAN, BLACK, PixelFormat::Bgr24);
    let src_grad = {
        // 16x16 的水平渐变，演示非棋盘内容
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

    // 16x16 棋盘
    r.blit(
        Pixel::new(c.x + 6, c.y + 6),
        16,
        16,
        PixelFormat::Bgr24,
        &src_small,
        16 * 3,
    );
    // 32x24 棋盘（源 pitch 同样是 32 像素）
    r.blit(
        Pixel::new(c.x + 30, c.y + 6),
        32,
        24,
        PixelFormat::Bgr24,
        &src_mid,
        32 * 3,
    );
    // 16x16 渐变
    r.blit(
        Pixel::new(c.x + 70, c.y + 6),
        16,
        16,
        PixelFormat::Bgr24,
        &src_grad,
        16 * 3,
    );
    // 故意贴到格子右缘，验证越界部分被自动裁掉（不 panic、不溢出）
    r.blit(
        Pixel::new(c.x + 100, c.y + 90),
        32,
        24,
        PixelFormat::Bgr24,
        &src_mid,
        32 * 3,
    );
    // 再贴一次，覆盖上一块，验证重复 blit 会覆盖而非叠加
    r.blit(
        Pixel::new(c.x + 100, c.y + 120),
        16,
        16,
        PixelFormat::Bgr24,
        &src_small,
        16 * 3,
    );
}

// ── 10. blit_mask ──

/// 径向渐变掩码画柔和圆斑；再用一个硬边掩码画方块。
fn cell_blit_mask(r: &mut Renderer, c: Pixel) {
    let (mw, mh) = (80usize, 80usize);
    let mut soft = vec![0u8; mw * mh];
    for y in 0..mh {
        for x in 0..mw {
            let dx = (x as f32 - mw as f32 / 2.0).abs();
            let dy = (y as f32 - mh as f32 / 2.0).abs();
            let dist = (dx * dx + dy * dy).sqrt();
            soft[y * mw + x] = (255.0 * (1.0 - dist / 40.0).clamp(0.0, 1.0)) as u8;
        }
    }
    r.blit_mask(Pixel::new(c.x + 6, c.y + 6), mw, mh, &soft, mw, YELLOW);

    // 硬边掩码：只点亮左半边
    let mut half = vec![0u8; mw * mh];
    for y in 0..mh {
        for x in 0..(mw / 2) {
            half[y * mw + x] = 255;
        }
    }
    r.blit_mask(Pixel::new(c.x + 92, c.y + 6), mw, mh, &half, mw, GREEN);
}

// ── 11. sub_view ──

/// 第一阶段占位：真正的绘制在 `main` 的第二阶段完成（见那里的注释）。
fn cell_placeholder_subview(_r: &mut Renderer, _c: Pixel) {}

// ── 12. info ──

/// 打印帧缓冲属性，并画一条按像素格式取色的色卡。
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
    // 色卡：8 色竖排
    for (i, col) in [RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA, WHITE, GRAY]
        .into_iter()
        .enumerate()
    {
        r.fill_rect(Pixel::new(c.x + 10 + i * 22, c.y + 30), 20, 80, col);
    }
    // 逐像素读取的读回校验：把某一列改成自己读取到的值，验证 get_pixel 可用
    for y in 0..60 {
        if let Some(got) = r.get_pixel(c.x + 10, c.y + 130 + y) {
            r.set_pixel(c.x + 150, c.y + 130 + y, &got);
        }
    }
    r.draw_rect(Pixel::new(c.x + 4, c.y + 124), 168, 70, &GRAY);
}

// ── 点阵文本 ──
//
// 输出图自带标签，便于对照。刻意不引第三方字体，保持示例零依赖。

/// 文本放大倍数。1x 的 5x7 在 960px 宽的图上几乎看不清，故整字放大 2 倍。
const SCALE: usize = 2;

/// 字模宽度（像素）。
const GLYPH_W: usize = 5;

/// 字模高度（像素）。
const GLYPH_H: usize = 7;

/// 绘制一行点阵文本，按 [`SCALE`] 倍放大。
///
/// 每字步进 `(GLYPH_W + 1) * SCALE` 列——多出的 1 列是字间隔。
fn draw_text(r: &mut Renderer, at: Pixel, text: &str, color: Color) {
    let mut x = at.x;
    for ch in text.chars() {
        // 统一转大写：字库只收了大写字母与数字
        let cols = glyph(ch.to_ascii_uppercase());
        for (dx, column) in cols.iter().enumerate() {
            for (dy, on) in column.iter().enumerate() {
                if !*on {
                    continue;
                }
                r.fill_rect(
                    Pixel::new(x + dx * SCALE, at.y + dy * SCALE),
                    SCALE,
                    SCALE,
                    color,
                );
            }
        }
        x += (GLYPH_W + 1) * SCALE;
    }
}

/// 查字符的 5 列 × 7 行点阵，未收录时返回一个空心方块。
///
/// 返回值形状与 [`FONT5X7`] 一致：`[[bool; GLYPH_H]; GLYPH_W]`（列优先）。
fn glyph(ch: char) -> [[bool; GLYPH_H]; GLYPH_W] {
    FONT5X7
        .iter()
        .find(|(c, _)| *c == ch)
        .map(|(_, cols)| *cols)
        .unwrap_or([
            // 空心方块。`[[bool; GLYPH_H]; GLYPH_W]` = 5 列，每列 7 行
            [false, true, false, false, false, false, false],
            [true, false, true, false, false, false, false],
            [true, false, true, false, false, false, false],
            [true, false, true, false, false, false, false],
            [false, true, false, false, false, false, false],
        ])
}

/// 5x7 点阵字库（列优先：每列 7 行）。
///
/// 只收录示例标题用到的 ASCII 子集；未收录字符在 `glyph` 中退化为方块。
/// 刻意不引第三方字体，保持示例零依赖。
const FONT5X7: [(char, [[bool; 7]; 5]); 45] = [
    (
        ' ',
        [
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        'A',
        [
            [false, true, true, true, true, true, true],
            [true, false, false, true, false, false, false],
            [true, false, false, true, false, false, false],
            [true, false, false, true, false, false, false],
            [false, true, true, true, true, true, true],
        ],
    ),
    (
        'B',
        [
            [true, true, true, true, true, true, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [false, true, true, false, true, true, false],
        ],
    ),
    (
        'C',
        [
            [false, true, true, true, true, true, false],
            [true, false, false, false, false, false, true],
            [true, false, false, false, false, false, true],
            [true, false, false, false, false, false, true],
            [false, true, false, false, false, true, false],
        ],
    ),
    (
        'D',
        [
            [true, true, true, true, true, true, true],
            [true, false, false, false, false, false, true],
            [true, false, false, false, false, false, true],
            [true, false, false, false, false, false, true],
            [false, true, true, true, true, true, false],
        ],
    ),
    (
        'E',
        [
            [true, true, true, true, true, true, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, false, false, false, true],
        ],
    ),
    (
        'F',
        [
            [true, true, true, true, true, true, true],
            [true, false, false, true, false, false, false],
            [true, false, false, true, false, false, false],
            [true, false, false, true, false, false, false],
            [true, false, false, false, false, false, false],
        ],
    ),
    (
        'G',
        [
            [false, true, true, true, true, true, false],
            [true, false, false, false, false, false, true],
            [true, false, false, false, false, false, true],
            [true, false, false, true, false, false, true],
            [false, true, false, true, true, true, false],
        ],
    ),
    (
        'H',
        [
            [true, true, true, true, true, true, true],
            [false, false, false, true, false, false, false],
            [false, false, false, true, false, false, false],
            [false, false, false, true, false, false, false],
            [true, true, true, true, true, true, true],
        ],
    ),
    (
        'I',
        [
            [false, false, false, false, false, false, false],
            [true, false, false, false, false, false, true],
            [true, true, true, true, true, true, true],
            [true, false, false, false, false, false, true],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        'J',
        [
            [false, false, false, false, true, true, false],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [true, true, true, true, true, true, false],
        ],
    ),
    (
        'K',
        [
            [true, true, true, true, true, true, true],
            [false, false, false, true, false, false, false],
            [false, false, true, false, true, false, false],
            [false, true, false, false, false, true, false],
            [true, false, false, false, false, false, true],
        ],
    ),
    (
        'L',
        [
            [true, true, true, true, true, true, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
        ],
    ),
    (
        'M',
        [
            [true, true, true, true, true, true, true],
            [false, true, false, false, false, false, false],
            [false, false, true, true, false, false, false],
            [false, true, false, false, false, false, false],
            [true, true, true, true, true, true, true],
        ],
    ),
    (
        'N',
        [
            [true, true, true, true, true, true, true],
            [false, true, false, false, false, false, false],
            [false, false, true, false, false, false, false],
            [false, false, false, true, false, false, false],
            [true, true, true, true, true, true, true],
        ],
    ),
    (
        'O',
        [
            [false, true, true, true, true, true, false],
            [true, false, false, false, false, false, true],
            [true, false, false, false, false, false, true],
            [true, false, false, false, false, false, true],
            [false, true, true, true, true, true, false],
        ],
    ),
    (
        'P',
        [
            [true, true, true, true, true, true, true],
            [true, false, false, true, false, false, false],
            [true, false, false, true, false, false, false],
            [true, false, false, true, false, false, false],
            [false, true, true, false, false, false, false],
        ],
    ),
    (
        'Q',
        [
            [false, true, true, true, true, true, false],
            [true, false, false, false, false, false, true],
            [true, false, false, false, true, false, true],
            [true, false, false, false, false, true, false],
            [false, true, true, true, true, false, true],
        ],
    ),
    (
        'R',
        [
            [true, true, true, true, true, true, true],
            [true, false, false, true, false, false, false],
            [true, false, false, true, true, false, false],
            [true, false, false, true, false, true, false],
            [false, true, true, false, false, false, true],
        ],
    ),
    (
        'S',
        [
            [false, true, true, false, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, false, true, true, false],
        ],
    ),
    (
        'T',
        [
            [true, false, false, false, false, false, false],
            [true, false, false, false, false, false, false],
            [true, true, true, true, true, true, true],
            [true, false, false, false, false, false, false],
            [true, false, false, false, false, false, false],
        ],
    ),
    (
        'U',
        [
            [true, true, true, true, true, true, false],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [true, true, true, true, true, true, false],
        ],
    ),
    (
        'V',
        [
            [true, true, true, true, true, false, false],
            [false, false, false, false, false, true, false],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, true, false],
            [true, true, true, true, true, false, false],
        ],
    ),
    (
        'W',
        [
            [true, true, true, true, true, true, true],
            [false, false, false, false, false, true, false],
            [false, false, false, true, true, false, false],
            [false, false, false, false, false, true, false],
            [true, true, true, true, true, true, true],
        ],
    ),
    (
        'X',
        [
            [true, true, false, false, false, true, true],
            [false, false, true, false, true, false, false],
            [false, false, false, true, false, false, false],
            [false, false, true, false, true, false, false],
            [true, true, false, false, false, true, true],
        ],
    ),
    (
        'Y',
        [
            [true, true, false, false, false, false, false],
            [false, false, true, false, false, false, false],
            [false, false, false, true, true, true, true],
            [false, false, true, false, false, false, false],
            [true, true, false, false, false, false, false],
        ],
    ),
    (
        'Z',
        [
            [true, false, false, false, false, true, true],
            [true, false, false, false, true, false, true],
            [true, false, false, true, false, false, true],
            [true, false, true, false, false, false, true],
            [true, true, false, false, false, false, true],
        ],
    ),
    (
        '0',
        [
            [false, true, true, true, true, true, false],
            [true, false, false, false, true, false, true],
            [true, false, false, true, false, false, true],
            [true, false, true, false, false, false, true],
            [false, true, true, true, true, true, false],
        ],
    ),
    (
        '1',
        [
            [false, false, false, false, false, false, false],
            [false, true, false, false, false, false, true],
            [true, true, true, true, true, true, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        '2',
        [
            [false, true, false, false, false, false, true],
            [true, false, false, false, false, true, true],
            [true, false, false, false, true, false, true],
            [true, false, false, true, false, false, true],
            [false, true, true, false, false, false, true],
        ],
    ),
    (
        '3',
        [
            [true, false, false, false, false, true, false],
            [true, false, false, false, false, false, true],
            [true, false, true, false, false, false, true],
            [true, true, false, true, false, false, true],
            [true, false, false, false, true, true, false],
        ],
    ),
    (
        '4',
        [
            [false, false, false, true, true, false, false],
            [false, false, true, false, true, false, false],
            [false, true, false, false, true, false, false],
            [true, true, true, true, true, true, true],
            [false, false, false, false, true, false, false],
        ],
    ),
    (
        '5',
        [
            [true, true, true, false, false, true, false],
            [true, false, true, false, false, false, true],
            [true, false, true, false, false, false, true],
            [true, false, true, false, false, false, true],
            [true, false, false, true, true, true, false],
        ],
    ),
    (
        '6',
        [
            [false, false, true, true, true, true, false],
            [false, true, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [false, false, false, false, true, true, false],
        ],
    ),
    (
        '7',
        [
            [true, false, false, false, false, false, false],
            [true, false, false, false, true, true, true],
            [true, false, false, true, false, false, false],
            [true, false, true, false, false, false, false],
            [true, true, false, false, false, false, false],
        ],
    ),
    (
        '8',
        [
            [false, true, true, false, true, true, false],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [false, true, true, false, true, true, false],
        ],
    ),
    (
        '9',
        [
            [false, true, true, false, false, false, false],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, false, true],
            [true, false, false, true, false, true, false],
            [false, true, true, true, true, false, false],
        ],
    ),
    (
        '.',
        [
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, true, true],
            [false, false, false, false, false, true, true],
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        '-',
        [
            [false, false, false, true, false, false, false],
            [false, false, false, true, false, false, false],
            [false, false, false, true, false, false, false],
            [false, false, false, true, false, false, false],
            [false, false, false, true, false, false, false],
        ],
    ),
    (
        '(',
        [
            [false, false, false, false, false, false, false],
            [false, false, true, true, true, false, false],
            [false, true, false, false, false, true, false],
            [true, false, false, false, false, false, true],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        ')',
        [
            [false, false, false, false, false, false, false],
            [true, false, false, false, false, false, true],
            [false, true, false, false, false, true, false],
            [false, false, true, true, true, false, false],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        '/',
        [
            [false, false, false, false, false, true, true],
            [false, false, false, false, true, false, false],
            [false, false, false, true, false, false, false],
            [false, false, true, false, false, false, false],
            [true, true, false, false, false, false, false],
        ],
    ),
    (
        ':',
        [
            [false, false, false, false, false, false, false],
            [false, true, true, false, true, true, false],
            [false, true, true, false, true, true, false],
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        ',',
        [
            [false, false, false, false, false, false, false],
            [false, false, false, false, true, true, true],
            [false, false, false, false, true, true, false],
            [false, false, false, false, false, false, false],
            [false, false, false, false, false, false, false],
        ],
    ),
    (
        '_',
        [
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
            [false, false, false, false, false, false, true],
        ],
    ),
];
