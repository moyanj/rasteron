//! programmable — 2D 可编程管线示例。
//!
//! 演示 rasteron 的顶点着色器、片元着色器、varying 插值、纹理采样
//! 和 scissor 裁剪功能，输出 BMP 图片。
//!
//! 运行:
//!   cargo run --example programmable --features programmable

#![cfg(feature = "programmable")]

use rasteron::Renderer;
use rasteron::color::{BLACK, BLUE, Color, GREEN, RED, WHITE};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;
use rasteron::programmable::{
    FragmentOutput, FragmentShader, Passthrough2D, Pipeline, PipelineState, PrimitiveType,
    ScissorRect, Texture2D, Vertex2D, VertexShader, Viewport,
};

#[path = "../shared/bmp.rs"]
mod bmp;

// ──────────────────────────────────────────────
//  自定义顶点着色器：将 UV 坐标传递到片元阶段
// ──────────────────────────────────────────────

#[derive(Clone, Copy)]
struct UvVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

struct UvVs;

impl VertexShader for UvVs {
    type Input = UvVertex;
    type Varying = [f32; 2];

    fn shade_vertex(
        &self,
        input: &Self::Input,
    ) -> rasteron::programmable::ProgramVertex<Self::Varying> {
        rasteron::programmable::ProgramVertex {
            position: input.position,
            varying: input.uv,
        }
    }
}

// ──────────────────────────────────────────────
//  自定义片元着色器：基于 UV 输出棋盘格颜色
// ──────────────────────────────────────────────

struct CheckerFs;

impl FragmentShader for CheckerFs {
    type Varying = [f32; 2];

    fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
        let u = varying[0];
        let v = varying[1];
        let tile_u = (u * 8.0).floor() as i32;
        let tile_v = (v * 8.0).floor() as i32;
        let is_white = (tile_u + tile_v) % 2 == 0;
        FragmentOutput::Color(if is_white { WHITE } else { BLACK })
    }
}

struct PositionVs;

impl VertexShader for PositionVs {
    type Input = [f32; 2];
    type Varying = [f32; 2];

    fn shade_vertex(
        &self,
        input: &Self::Input,
    ) -> rasteron::programmable::ProgramVertex<Self::Varying> {
        rasteron::programmable::ProgramVertex {
            position: *input,
            varying: *input,
        }
    }
}

/// 将右半部分像素 discard 的片元着色器。
struct DiscardRightFs;

impl FragmentShader for DiscardRightFs {
    type Varying = [f32; 2];

    fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
        if varying[0] > 0.5 {
            FragmentOutput::Discard
        } else {
            FragmentOutput::Color(WHITE)
        }
    }
}

fn main() {
    let width = 640;
    let height = 480;
    // 使用 Rgba8888 示例 BMP 写入对 32-bit 的支持
    let bpp = 4;
    let pitch = width * bpp;
    let info = FramebufferInfo::new(width, height, pitch, rasteron::color::PixelFormat::Rgba8888);
    let mut fb = vec![0u8; info.buffer_size()];
    let mut renderer = Renderer::new(info, &mut fb);

    // ── 背景：深灰色 ──
    renderer.clear(Color::new(30, 30, 30));

    // ───────────────────────────────────────────
    // 1. 带 Checkerboard 纹理的四边（索引绘制）
    // ───────────────────────────────────────────
    let quad_vertices = [
        UvVertex {
            position: [0.05, 0.05],
            uv: [0.0, 0.0],
        },
        UvVertex {
            position: [0.55, 0.05],
            uv: [1.0, 0.0],
        },
        UvVertex {
            position: [0.55, 0.55],
            uv: [1.0, 1.0],
        },
        UvVertex {
            position: [0.05, 0.55],
            uv: [0.0, 1.0],
        },
    ];
    let indices = [0u16, 1, 2, 0, 2, 3];
    let pipeline_checker = Pipeline::new(
        UvVs,
        CheckerFs,
        PipelineState::alpha_blend(Viewport::new(0, 0, width, height)),
    );
    renderer.draw_indexed(
        PrimitiveType::Triangles,
        &quad_vertices,
        &indices,
        &pipeline_checker,
    );

    // ───────────────────────────────────────────
    // 2. Texture2D 采样（使用内存中的颜色图）
    // ───────────────────────────────────────────
    let tex_w = 4;
    let tex_h = 4;
    let texels: Vec<Color> = (0..tex_h)
        .flat_map(|y| {
            (0..tex_w).map(move |x| {
                let r = (x * 255 / (tex_w - 1)) as u8;
                let g = (y * 255 / (tex_h - 1)) as u8;
                let b = 128;
                Color::new(r, g, b)
            })
        })
        .collect();
    let texture = Texture2D::new(tex_w, tex_h, &texels);

    struct TexFs<'a> {
        tex: Texture2D<'a>,
    }

    impl FragmentShader for TexFs<'_> {
        type Varying = [f32; 2];

        fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
            FragmentOutput::Color(self.tex.sample_nearest(*varying))
        }
    }

    let tex_quad = [
        UvVertex {
            position: [0.55, 0.55],
            uv: [0.0, 0.0],
        },
        UvVertex {
            position: [0.95, 0.55],
            uv: [1.0, 0.0],
        },
        UvVertex {
            position: [0.95, 0.95],
            uv: [1.0, 1.0],
        },
        UvVertex {
            position: [0.55, 0.95],
            uv: [0.0, 1.0],
        },
    ];
    let pipeline_tex = Pipeline::new(
        UvVs,
        TexFs { tex: texture },
        PipelineState::alpha_blend(Viewport::new(0, 0, width, height)),
    );
    renderer.draw_indexed(PrimitiveType::Triangles, &tex_quad, &indices, &pipeline_tex);

    // ───────────────────────────────────────────
    // 3. 颜色插值三角形（draw_arrays）
    // ───────────────────────────────────────────
    let gradient_tri = [
        Vertex2D {
            position: [0.1, 0.6],
            varying: RED,
        },
        Vertex2D {
            position: [0.5, 0.6],
            varying: GREEN,
        },
        Vertex2D {
            position: [0.3, 0.95],
            varying: BLUE,
        },
    ];
    let pipeline_gradient = Pipeline::new(
        Passthrough2D::new(),
        SolidFs,
        PipelineState::alpha_blend(Viewport::new(0, 0, width, height)),
    );
    renderer.draw_arrays(PrimitiveType::Triangles, &gradient_tri, &pipeline_gradient);

    // ───────────────────────────────────────────
    // 4. Scissor 裁剪与 Discard 演示
    // ───────────────────────────────────────────
    // 在右下角画一个覆盖大范围的三角形，但 scissor 限制在 400x300 区域内
    let scissor_rect = ScissorRect {
        x: 400,
        y: 300,
        width: 200,
        height: 140,
    };
    let big_tri = [[0.5, 0.5], [1.0, 0.5], [1.0, 1.0]];
    let pipeline_scissor = Pipeline::new(
        PositionVs,
        DiscardRightFs,
        PipelineState {
            blend: rasteron::programmable::BlendMode::None,
            scissor: Some(scissor_rect),
            viewport: Viewport::new(0, 0, width, height),
        },
    );
    // 先用半透明蓝色填充 scissor 区域以标识边界
    renderer.fill_rect(
        Pixel::new(400, 300),
        200,
        140,
        Color::with_alpha(0, 100, 200, 80),
    );
    // 然后绘制被裁剪的三角形
    renderer.draw_arrays(PrimitiveType::Triangles, &big_tri, &pipeline_scissor);

    // ───────────────────────────────────────────
    // 5. Points 绘制星星
    // ───────────────────────────────────────────
    let stars = [
        Vertex2D {
            position: [0.1, 0.1],
            varying: WHITE,
        },
        Vertex2D {
            position: [0.3, 0.15],
            varying: WHITE,
        },
        Vertex2D {
            position: [0.5, 0.08],
            varying: WHITE,
        },
        Vertex2D {
            position: [0.7, 0.12],
            varying: WHITE,
        },
        Vertex2D {
            position: [0.9, 0.05],
            varying: WHITE,
        },
    ];
    let pipeline_points = Pipeline::new(
        Passthrough2D::new(),
        SolidFs,
        PipelineState::alpha_blend(Viewport::new(0, 0, width, height)),
    );
    renderer.draw_arrays(PrimitiveType::Points, &stars, &pipeline_points);

    // ───────────────────────────────────────────
    // 6. Lines 绘制线段
    // ───────────────────────────────────────────
    let cross_lines = [
        Vertex2D {
            position: [0.0, 0.0],
            varying: RED,
        },
        Vertex2D {
            position: [0.5, 0.5],
            varying: RED,
        },
        Vertex2D {
            position: [0.5, 0.5],
            varying: GREEN,
        },
        Vertex2D {
            position: [1.0, 0.0],
            varying: GREEN,
        },
    ];
    let pipeline_lines = Pipeline::new(
        Passthrough2D::new(),
        SolidFs,
        PipelineState::alpha_blend(Viewport::new(0, 0, width, height)),
    );
    renderer.draw_arrays(PrimitiveType::Lines, &cross_lines, &pipeline_lines);

    #[cfg(feature = "double_buffer")]
    renderer.swap_buffers();

    let output = "programmable-example.bmp";
    bmp::write_bmp(
        std::path::Path::new(output),
        width,
        height,
        rasteron::color::PixelFormat::Rgba8888,
        renderer.buffer_mut(),
    )
    .expect("failed to write BMP");
    println!("{output}");
}

// ───────── 辅助：固体色片元着色器 ─────────

struct SolidFs;

impl FragmentShader for SolidFs {
    type Varying = Color;

    fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
        FragmentOutput::Color(*varying)
    }
}
