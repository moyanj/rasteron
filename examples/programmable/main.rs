//! programmable — 2D 可编程管线示例。
//!
//! 最小用法：内建的 [`Passthrough2D`] 顶点着色器 + 自定义片元着色器，
//! 用 `PrimitiveType::Points` 画一片点阵。
//!
//! 运行：
//!
//! ```text
//! cargo run --example programmable
//! ```
//!
//! 产出 `programmable-example.bmp`。
//!
//! # 坐标约定
//!
//! 顶点着色器输出的 `position` 与片元着色器看到的 [`FragmentInput::position`]
//! 是同一个坐标系：normalized `[0, 1]` viewport 坐标，`[0, 0]` 为 viewport 左上角、
//! Y 轴向下。

use rasteron::Renderer;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::programmable::shader::Passthrough2D;
use rasteron::programmable::{
    FragmentInput, FragmentOutput, FragmentShader, Pipeline, PipelineState, PrimitiveType,
    Vertex2D, Viewport,
};

#[path = "../shared/bmp.rs"]
mod bmp;

const WIDTH: usize = 640;
const HEIGHT: usize = 480;
/// 点阵边长（点数 = 边长的平方）。
const LATTICE: usize = 64;

/// 片元着色器：把 varying 携带的颜色直接写出去。
struct SolidFs;

impl FragmentShader for SolidFs {
    type Varying = Color;

    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
        FragmentOutput::Color(input.varying)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let format = PixelFormat::Bgr24;
    let info = FramebufferInfo::new(WIDTH, HEIGHT, WIDTH * format.bytes_per_pixel(), format);
    let mut framebuffer = vec![0u8; info.buffer_size()];
    let mut renderer = Renderer::new(info, &mut framebuffer);

    renderer.clear(Color::new(10, 12, 24));

    // 顶点位置是 normalized [0, 1] viewport 坐标，`[0, 0]` 为左上角。
    // 这里铺一片点阵，varying 让颜色随行列位置渐变。
    let step = 1.0 / (LATTICE - 1) as f32;
    let points: Vec<Vertex2D<Color>> = (0..LATTICE)
        .flat_map(|row| {
            (0..LATTICE).map(move |col| {
                let u = col as f32 * step;
                let v = row as f32 * step;
                Vertex2D {
                    position: [0.05 + u * 0.9, 0.05 + v * 0.9],
                    varying: Color::new((u * 255.0) as u8, 160, (v * 255.0) as u8),
                }
            })
        })
        .collect();

    let pipeline = Pipeline::new(
        Passthrough2D::new(),
        SolidFs,
        PipelineState::alpha_blend(Viewport::new(0, 0, WIDTH, HEIGHT)),
    );
    // 顺序绘制没有索引缓冲，`DrawError` 唯一的变体（索引越界）不可能触发。
    renderer
        .draw_arrays(PrimitiveType::Points, &points, &pipeline)
        .expect("顺序绘制没有索引缓冲，不可能越界");

    let output = "programmable-example.bmp";
    bmp::write_bmp(
        std::path::Path::new(output),
        WIDTH,
        HEIGHT,
        format,
        renderer.buffer_mut(),
    )?;
    println!("{output}");
    Ok(())
}
