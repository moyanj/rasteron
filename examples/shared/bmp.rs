//! BMP 图片写入工具，支持 24-bit BGR 和 32-bit BGRA 格式。
//!
//! 用于 examples 将帧缓冲内容输出为 BMP 文件。
#![allow(dead_code)]

use rasteron::color::PixelFormat;
use std::fs;
use std::path::Path;

/// 将帧缓冲像素数据写入 BMP 文件。
///
/// 支持的像素格式：
/// - `Bgr24` → 24-bit BMP（标准）
/// - `Rgba8888` → 32-bit BMP（Alpha 通道写入，查看器会忽略）
///
/// 其他格式会先转换为 BGR24 再写入。
pub fn write_bmp(
    path: &Path,
    width: usize,
    height: usize,
    format: PixelFormat,
    pixels: &[u8],
) -> std::io::Result<()> {
    match format {
        PixelFormat::Bgr24 => write_bmp_bgr24(path, width, height, pixels),
        PixelFormat::Rgba8888 => write_bmp_bgra32(path, width, height, pixels),
        other => {
            // 转换为 BGR24 后写入
            let bgr = convert_to_bgr24(width, height, other, pixels);
            write_bmp_bgr24(path, width, height, &bgr)
        }
    }
}

/// 写入 24-bit BMP（BGR 格式）。
fn write_bmp_bgr24(
    path: &Path,
    width: usize,
    height: usize,
    pixels: &[u8],
) -> std::io::Result<()> {
    let row_stride = width * 3;
    let row_padding = (4 - (row_stride % 4)) % 4;
    let image_size = (row_stride + row_padding) * height;
    let file_size = 14 + 40 + image_size;

    let mut bmp = Vec::with_capacity(file_size);
    // BMP 文件头
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&(54u32).to_le_bytes());
    // DIB 头 (BITMAPINFOHEADER)
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
        bmp.extend_from_slice(&pixels[start..start + row_stride]);
        bmp.extend_from_slice(&padding[..row_padding]);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bmp)
}

/// 写入 32-bit BMP（BGRA 格式，每行天然 4 字节对齐）。
fn write_bmp_bgra32(
    path: &Path,
    width: usize,
    height: usize,
    pixels: &[u8],
) -> std::io::Result<()> {
    let row_stride = width * 4;
    let image_size = row_stride * height;
    let file_size = 14 + 40 + image_size;

    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&(54u32).to_le_bytes());
    // DIB 头 (BITMAPINFOHEADER) — 32-bit
    bmp.extend_from_slice(&(40u32).to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(height as i32).to_le_bytes());
    bmp.extend_from_slice(&(1u16).to_le_bytes());
    bmp.extend_from_slice(&(32u16).to_le_bytes());
    bmp.extend_from_slice(&(0u32).to_le_bytes());
    bmp.extend_from_slice(&(image_size as u32).to_le_bytes());
    bmp.extend_from_slice(&(2835i32).to_le_bytes());
    bmp.extend_from_slice(&(2835i32).to_le_bytes());
    bmp.extend_from_slice(&(0u32).to_le_bytes());
    bmp.extend_from_slice(&(0u32).to_le_bytes());

    // 32-bit BMP 使用 BGRA 顺序，Rgba8888 需要交换 R↔B
    for y in (0..height).rev() {
        let row_offset = y * row_stride;
        for x in 0..width {
            let off = row_offset + x * 4;
            // RGBA → BGRA
            bmp.push(pixels[off + 2]); // B
            bmp.push(pixels[off + 1]); // G
            bmp.push(pixels[off + 0]); // R
            bmp.push(pixels[off + 3]); // A
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bmp)
}

/// 将任意像素格式的帧缓冲转换为 BGR24 平面数据。
fn convert_to_bgr24(
    width: usize,
    height: usize,
    format: PixelFormat,
    pixels: &[u8],
) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 3];
    let bpp = format.bytes_per_pixel();
    for y in 0..height {
        for x in 0..width {
            let src_off = y * width * bpp + x * bpp; // 假设 pitch = width * bpp
            let dst_off = (y * width + x) * 3;
            let color = format.read_color(&pixels[src_off..src_off + bpp]);
            out[dst_off..dst_off + 3].copy_from_slice(&[color.b, color.g, color.r]);
        }
    }
    out
}

/// 生成彩色棋盘格图案，用于 blit 示例的源数据。
pub fn make_checkerboard(
    tile_w: usize,
    tile_h: usize,
    tiles_x: usize,
    tiles_y: usize,
    color_a: rasteron::color::Color,
    color_b: rasteron::color::Color,
    format: PixelFormat,
) -> Vec<u8> {
    let w = tile_w * tiles_x;
    let h = tile_h * tiles_y;
    let bpp = format.bytes_per_pixel();
    let mut data = vec![0u8; w * h * bpp];
    for y in 0..h {
        for x in 0..w {
            let tx = x / tile_w;
            let ty = y / tile_h;
            let color = if (tx + ty) % 2 == 0 { color_a } else { color_b };
            let off = (y * w + x) * bpp;
            let bytes = color.to_bytes(format);
            data[off..off + bpp].copy_from_slice(&bytes[..bpp]);
        }
    }
    data
}
