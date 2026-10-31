//! antialias — 超采样抗锯齿对比示例。
//!
//! 把同一组图元用两种方式各画一遍，并排输出，便于肉眼比对边缘质量：
//!
//! | 列 | 方法 | 做法 |
//! |----|------|------|
//! | 1 | 无抗锯齿 | 直接画，边缘是硬像素台阶 |
//! | 2 | SSAA 4× | 4 倍分辨率绘制后面积平均降采样 |
//!
//! 运行:
//!   cargo run --example antialias
//!
//! 输出 `antialias-test.bmp` 到仓库根目录。

use rasteron::Renderer;
use rasteron::color::{BLUE, Color, GREEN, PixelFormat, RED, WHITE, YELLOW};
use rasteron::core::SsaaFactor;
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;

#[path = "../shared/bmp.rs"]
mod bmp;

/// 单格的边长（像素）。
const CELL: usize = 200;
/// 两列并排。
const COLUMNS: usize = 2;

fn main() {
    let total_width = CELL * COLUMNS;
    let total_height = CELL;
    let bpp = 3;
    let pitch = total_width * bpp;
    let info = FramebufferInfo::new(total_width, total_height, pitch, PixelFormat::Bgr24);
    let mut fb = vec![0u8; info.buffer_size()];

    // ---- 第 1 列：无抗锯齿 ----
    {
        let cell_info = FramebufferInfo::new(CELL, CELL, CELL * bpp, PixelFormat::Bgr24);
        let mut cell = vec![0u8; cell_info.buffer_size()];
        let mut r = Renderer::new(cell_info, &mut cell);
        draw_scene(&mut r, 1);
        blit_cell(&mut fb, pitch, &cell, CELL * bpp, 0);
    }

    // ---- 第 2 列：SSAA 4× ----
    {
        let cell_info = FramebufferInfo::new(CELL, CELL, CELL * bpp, PixelFormat::Bgr24);
        let mut cell = vec![0u8; cell_info.buffer_size()];
        let mut r = Renderer::new(cell_info, &mut cell);
        // 4 倍分辨率的离屏缓冲：紧凑行距 pitch × 高度 × 4。
        let hi_pitch = CELL * 4 * bpp;
        let mut scratch = vec![0u8; hi_pitch * CELL * 4];
        let applied = r.ssaa(&mut scratch, SsaaFactor::X4, |hi| draw_scene(hi, 4));
        assert!(applied, "SSAA 应成功执行");
        blit_cell(&mut fb, pitch, &cell, CELL * bpp, CELL);
    }

    let out = std::path::Path::new("antialias-test.bmp");
    bmp::write_bmp(out, total_width, total_height, PixelFormat::Bgr24, &fb).expect("写出 BMP");
    println!("已输出 {}", out.display());
    println!("列 1 = 无 AA，列 2 = SSAA 4×");
}

/// 在 `cell × cell` 的画布上画一组最能暴露锯齿的图元。
///
/// `scale` 是坐标放大倍数：SSAA 需要在高分辨率画布上用放大后的坐标绘制。
fn draw_scene(r: &mut Renderer<'_>, scale: usize) {
    let s = scale;
    r.clear(Color::new(24, 24, 32));

    // 斜边三角形：锯齿最明显的形状。
    r.fill_triangle(
        Pixel::new(20 * s, 170 * s),
        Pixel::new(100 * s, 20 * s),
        Pixel::new(180 * s, 170 * s),
        &RED,
    );

    // 圆：曲边锯齿。
    r.fill_circle(Pixel::new(100 * s, 100 * s), 48 * s, GREEN);

    // 细斜线：1px 线的锯齿。
    for i in 0..6 {
        let offset = i * 6;
        r.draw_line(
            Pixel::new((10 + offset) * s, 10 * s),
            Pixel::new((60 + offset) * s, 60 * s),
            &YELLOW,
        );
    }

    // 近水平/近垂直的细线：最考验 AA 的浅角度。
    r.draw_line(
        Pixel::new(10 * s, 190 * s),
        Pixel::new(190 * s, 186 * s),
        &WHITE,
    );
    r.draw_line(
        Pixel::new(180 * s, 10 * s),
        Pixel::new(184 * s, 190 * s),
        &BLUE,
    );

    // 小字不一定能看清，这里用密集同心圆代替「细节纹理」检查是否被糊掉。
    for radius in [10usize, 14, 18] {
        r.draw_circle(Pixel::new(40 * s, 40 * s), radius * s, &WHITE);
    }
}

/// 把单格缓冲拷贝到总画布的指定列起始位置。
fn blit_cell(
    dst: &mut [u8],
    dst_pitch: usize,
    cell: &[u8],
    cell_pitch: usize,
    column_start: usize,
) {
    let bpp = 3;
    for y in 0..CELL {
        let dst_off = y * dst_pitch + column_start * bpp;
        let src_off = y * cell_pitch;
        dst[dst_off..dst_off + CELL * bpp].copy_from_slice(&cell[src_off..src_off + CELL * bpp]);
    }
}
