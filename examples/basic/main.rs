//! basic — 基本图形绘制示例。
//!
//! 演示 rasteron 的所有 2D 绘图原语，输出 BMP 图片。
//!
//! 运行:
//!   cargo run --example basic

use rasteron::Renderer;
use rasteron::angle::Angle;
use rasteron::color::{BLACK, BLUE, Color, GRAY, GREEN, PixelFormat, RED, WHITE, YELLOW};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;

#[path = "../shared/bmp.rs"]
mod bmp;

fn main() {
    let width = 640;
    let height = 480;
    let bpp = 3;
    let pitch = width * bpp;
    let info = FramebufferInfo::new(width, height, pitch, PixelFormat::Bgr24);
    // 双缓冲：后台缓冲区由调用方提供，库不做任何分配。
    let mut fb = vec![0u8; info.buffer_size()];
    let mut back = vec![0u8; info.buffer_size()];
    let mut renderer = Renderer::with_back_buffer(info, &mut fb, &mut back);

    // ── 背景：渐变天空 ──
    draw_sky(&mut renderer, width, height);

    // ── 地面 ──
    renderer.fill_rect(Pixel::new(0, 320), width, 160, GREEN);

    // ── 太阳（填充圆形） ──
    renderer.fill_circle(Pixel::new(520, 80), 50, YELLOW);

    // ── 太阳光芒（线条） ──
    for angle in (0..360).step_by(30) {
        let rad = (angle as f64).to_radians();
        let r = 58.0;
        let ex = (520.0 + rad.cos() * r) as usize;
        let ey = (80.0 + rad.sin() * r) as usize;
        renderer.draw_line(Pixel::new(520, 80), Pixel::new(ex, ey), &YELLOW);
    }

    // ── 圆弧与扇形（定点角度，无浮点） ──
    // 彩虹扇形：从 180°（左）扫掠 180° 到 0°（右），覆盖上半圈。
    let hub = Pixel::new(520, 300);
    let bands = [(90, RED), (70, YELLOW), (50, GREEN), (30, BLUE)];
    for (r, color) in bands {
        renderer.draw_arc(hub, r, Angle::DOWN_LEFT, Angle::ZERO, &color);
    }
    // 三个示例扇形，展示不同跨度（含跨界与大跨度）
    renderer.fill_arc(
        Pixel::new(160, 400),
        60,
        Angle::ZERO,
        Angle::from_degrees(120),
        Color::with_alpha(255, 255, 255, 90),
    );
    renderer.fill_arc(
        Pixel::new(320, 400),
        60,
        Angle::from_degrees(350),
        Angle::from_degrees(10),
        Color::with_alpha(0, 255, 255, 110),
    );
    renderer.fill_arc(
        Pixel::new(480, 400),
        60,
        Angle::ZERO,
        Angle::from_degrees(270),
        Color::with_alpha(255, 0, 255, 70),
    );

    // ── 远山（填充三角形） ──
    renderer.fill_triangle(
        Pixel::new(0, 320),
        Pixel::new(120, 140),
        Pixel::new(240, 320),
        &GRAY,
    );
    renderer.fill_triangle(
        Pixel::new(160, 320),
        Pixel::new(340, 100),
        Pixel::new(520, 320),
        &GRAY,
    );
    renderer.fill_triangle(
        Pixel::new(400, 320),
        Pixel::new(580, 160),
        Pixel::new(640, 320),
        &GRAY,
    );

    // ── 山峰轮廓（三角形描边） ──
    renderer.draw_triangle(
        Pixel::new(0, 320),
        Pixel::new(120, 140),
        Pixel::new(240, 320),
        &BLACK,
    );
    renderer.draw_triangle(
        Pixel::new(160, 320),
        Pixel::new(340, 100),
        Pixel::new(520, 320),
        &BLACK,
    );
    renderer.draw_triangle(
        Pixel::new(400, 320),
        Pixel::new(580, 160),
        Pixel::new(640, 320),
        &BLACK,
    );

    // ── 房子（填充矩形 + 圆角矩形） ──
    // 主体
    renderer.fill_rect(Pixel::new(60, 220), 120, 100, Color::new(210, 180, 140));
    renderer.draw_rect(Pixel::new(60, 220), 120, 100, &BLACK);
    // 屋顶（三角形）
    renderer.fill_triangle(
        Pixel::new(50, 220),
        Pixel::new(120, 170),
        Pixel::new(190, 220),
        &RED,
    );
    renderer.draw_triangle(
        Pixel::new(50, 220),
        Pixel::new(120, 170),
        Pixel::new(190, 220),
        &BLACK,
    );
    // 门（矩形）
    renderer.fill_rect(Pixel::new(100, 270), 40, 50, Color::new(139, 69, 19));
    renderer.draw_rect(Pixel::new(100, 270), 40, 50, &BLACK);
    // 窗户（矩形轮廓）
    renderer.draw_rect(Pixel::new(70, 240), 24, 24, &BLACK);
    renderer.draw_rect(Pixel::new(146, 240), 24, 24, &BLACK);

    // ── 圆形轮廓 ──
    renderer.draw_circle(Pixel::new(420, 240), 40, &BLUE);

    // ── 线段（对角线） ──
    renderer.draw_line(Pixel::new(0, 0), Pixel::new(width - 1, height - 1), &RED);
    renderer.draw_line(Pixel::new(width - 1, 0), Pixel::new(0, height - 1), &BLUE);

    // ── blit：棋盘格 ──
    let checker = bmp::make_checkerboard(
        8,
        8,
        8,
        6,
        WHITE,
        Color::new(100, 100, 100),
        PixelFormat::Bgr24,
    );
    renderer.blit(
        Pixel::new(width - 80, height - 60),
        64,
        48,
        PixelFormat::Bgr24,
        &checker,
        64 * 3,
    );

    // ── blit_mask：烟雾/云朵 ──
    let mask_w = 64;
    let mask_h = 32;
    let mut mask = vec![0u8; mask_w * mask_h];
    for y in 0..mask_h {
        for x in 0..mask_w {
            let dx = (x as f64 - mask_w as f64 / 2.0).abs();
            let dy = (y as f64 - mask_h as f64 / 2.0).abs();
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha = (255.0 * (1.0 - (dist / 28.0)).clamp(0.0, 1.0)) as u8;
            mask[y * mask_w + x] = alpha;
        }
    }
    renderer.blit_mask(Pixel::new(260, 50), mask_w, mask_h, &mask, mask_w, WHITE);

    // ── set_pixel：星星 ──
    let stars = [
        (50, 30),
        (180, 20),
        (300, 45),
        (420, 15),
        (550, 35),
        (620, 55),
        (80, 70),
        (350, 60),
        (480, 40),
        (590, 25),
    ];
    for &(sx, sy) in &stars {
        renderer.set_pixel(sx, sy, &WHITE);
    }

    renderer.swap_buffers();

    let output = "basic-example.bmp";
    bmp::write_bmp(
        std::path::Path::new(output),
        width,
        height,
        PixelFormat::Bgr24,
        renderer.buffer_mut(),
    )
    .expect("failed to write BMP");
    println!("{output}");
}

/// 绘制蓝色渐变天空。
fn draw_sky(renderer: &mut Renderer, width: usize, _height: usize) {
    for y in 0..320 {
        let t = y as f32 / 320.0;
        let r = (t * 200.0) as u8;
        let g = (t * 180.0 + 40.0) as u8;
        let b = (255.0 - t * 100.0) as u8;
        let color = Color::new(r, g, b);
        // 每 4 行一批，平衡精度与性能
        renderer.fill_rect(Pixel::new(0, y), width, 1, color);
    }
}
