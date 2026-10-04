//! 演示二进制文件 — 生成测试 BMP 图像。
//!
//! 创建一个 1280 × 720 的 BGR24 帧缓冲，绘制一条线段后将结果写入
//! `rasteron-test.bmp`。

use rasteron::color::{BLACK, BLUE, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::{Renderer, pixel};
use std::path::Path;

#[path = "../examples/shared/bmp.rs"]
mod bmp;

fn main() {
    let output = String::from("rasteron-test.bmp");

    let width = 1280usize;
    let height = 720usize;
    let pitch = width * 3;
    let info = FramebufferInfo::new(width, height, pitch, PixelFormat::Bgr24);
    let mut front_buffer = vec![0u8; info.buffer_size()];
    let mut back_buffer = vec![0u8; info.buffer_size()];
    let mut renderer = Renderer::with_back_buffer(info, &mut front_buffer, &mut back_buffer);

    renderer.clear(BLUE);
    renderer.draw_line(pixel!(1, 0), pixel!(1000, 200), &BLACK);

    renderer.swap_buffers();

    bmp::write_bmp(
        Path::new(&output),
        width,
        height,
        PixelFormat::Bgr24,
        renderer.buffer_mut(),
    )
    .expect("failed to write bmp");
    println!("{}", output);
}
