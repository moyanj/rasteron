//! 演示二进制文件 — 生成测试 BMP 图像。
//!
//! 创建一个 1280 × 720 的 BGR24 帧缓冲，绘制一条线段后将结果写入
//! `rasteron-test.bmp`。

use rasteron::color::{BLACK, BLUE, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::{Renderer, pixel};
use std::fs;
use std::path::Path;

fn main() {
    let output = String::from("rasteron-test.bmp");

    let width = 1280usize;
    let height = 720usize;
    let pitch = width * 3;
    let info = FramebufferInfo::new(width, height, pitch, PixelFormat::Bgr24);
    let mut front_buffer = vec![0u8; info.buffer_size()];
    let mut renderer = Renderer::new(info, &mut front_buffer);

    renderer.clear(BLUE);
    renderer.draw_line(pixel!(1, 0), pixel!(1000, 200), &BLACK);

    #[cfg(feature = "double_buffer")]
    renderer.swap_buffers();

    write_bmp(Path::new(&output), width, height, renderer.buffer_mut())
        .expect("failed to write bmp");
    println!("{}", output);
}

/// 将原始 BGR24 像素缓冲区写入磁盘，生成 24 位 BMP 文件。
///
/// BMP 采用自底向上写入（标准 BMP 约定）。每行填充到 4 字节对齐。
fn write_bmp(path: &Path, width: usize, height: usize, pixels: &[u8]) -> std::io::Result<()> {
    let row_stride = width * 3;
    let row_padding = (4 - (row_stride % 4)) % 4;
    let image_size = (row_stride + row_padding) * height;
    let file_size = 14 + 40 + image_size;

    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&(54u32).to_le_bytes());

    bmp.extend_from_slice(&(40u32).to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(height as i32).to_le_bytes());
    bmp.extend_from_slice(&(1u16).to_le_bytes());
    bmp.extend_from_slice(&(24u16).to_le_bytes());
    bmp.extend_from_slice(&(0u32).to_le_bytes());
    bmp.extend_from_slice(&(image_size as u32).to_le_bytes());
    bmp.extend_from_slice(&(2835i32).to_le_bytes());
    bmp.extend_from_slice(&(2835i32).to_le_bytes());
    bmp.extend_from_slice(&(0u32).to_le_bytes());
    bmp.extend_from_slice(&(0u32).to_le_bytes());

    let padding = [0u8; 3];
    for y in (0..height).rev() {
        let start = y * row_stride;
        let end = start + row_stride;
        bmp.extend_from_slice(&pixels[start..end]);
        bmp.extend_from_slice(&padding[..row_padding]);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bmp)
}
