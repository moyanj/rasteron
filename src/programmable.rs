//! 2D 可编程光栅化接口。
//!
//! 这一层不引入 OpenGL 式状态机；所有资源和状态都通过 `Pipeline` 与
//! `draw_arrays` / `draw_indexed` 的显式参数传入。
//!
//! 当前支持：
//!
//! - `PrimitiveType::Points`
//! - `PrimitiveType::Lines`
//! - `PrimitiveType::Triangles`
//! - 顶点着色器与片元着色器
//! - varying 插值
//! - viewport / scissor
//! - `BlendMode::None` / `BlendMode::Alpha`
//! - `Texture2D::sample_nearest`
//!
extern crate alloc;

use crate::color::Color;
use core::marker::PhantomData;

/// 光栅化原语类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveType {
    Points,
    Lines,
    Triangles,
}

/// 顶点着色器输出到光栅化阶段的数据。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgramVertex<V> {
    pub position: [f32; 2],
    pub varying: V,
}

/// 片元着色器输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentOutput {
    Color(Color),
    Discard,
}

/// varying 插值 trait。
pub trait Interpolate: Copy {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self;
}

impl Interpolate for () {
    fn interpolate(_: Self, _: Self, _: Self, _: f32, _: f32, _: f32) -> Self {}
}

impl Interpolate for Color {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        fn channel(a: u8, b: u8, c: u8, w0: f32, w1: f32, w2: f32) -> u8 {
            let value = a as f32 * w0 + b as f32 * w1 + c as f32 * w2;
            (value + 0.5).clamp(0.0, 255.0) as u8
        }

        Color::with_alpha(
            channel(a.r, b.r, c.r, w0, w1, w2),
            channel(a.g, b.g, c.g, w0, w1, w2),
            channel(a.b, b.b, c.b, w0, w1, w2),
            channel(a.a, b.a, c.a, w0, w1, w2),
        )
    }
}

impl Interpolate for f32 {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        a * w0 + b * w1 + c * w2
    }
}

impl Interpolate for [f32; 2] {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        [
            a[0] * w0 + b[0] * w1 + c[0] * w2,
            a[1] * w0 + b[1] * w1 + c[1] * w2,
        ]
    }
}

impl Interpolate for [f32; 4] {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        [
            a[0] * w0 + b[0] * w1 + c[0] * w2,
            a[1] * w0 + b[1] * w1 + c[1] * w2,
            a[2] * w0 + b[2] * w1 + c[2] * w2,
            a[3] * w0 + b[3] * w1 + c[3] * w2,
        ]
    }
}

/// 顶点着色器。
pub trait VertexShader {
    type Input;
    type Varying: Interpolate;

    fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying>;
}

/// 片元着色器。
pub trait FragmentShader {
    type Varying: Copy;

    fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput;
}

/// 裁剪矩形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScissorRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// 视口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Viewport {
    pub const fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// 混合模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    None,
    Alpha,
}

/// 最近邻采样的 2D 纹理。
#[derive(Debug, Clone, Copy)]
pub struct Texture2D<'a> {
    width: usize,
    height: usize,
    texels: &'a [Color],
}

impl<'a> Texture2D<'a> {
    pub const fn new(width: usize, height: usize, texels: &'a [Color]) -> Self {
        Self {
            width,
            height,
            texels,
        }
    }

    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub fn sample_nearest(&self, uv: [f32; 2]) -> Color {
        if self.width == 0 || self.height == 0 || self.texels.is_empty() {
            return Color::with_alpha(0, 0, 0, 0);
        }

        let u = uv[0].clamp(0.0, 1.0);
        let v = uv[1].clamp(0.0, 1.0);
        let x = (u * (self.width.saturating_sub(1) as f32) + 0.5) as usize;
        let y = (v * (self.height.saturating_sub(1) as f32) + 0.5) as usize;
        self.texels[y * self.width + x]
    }
}

/// 管线状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineState {
    pub blend: BlendMode,
    pub scissor: Option<ScissorRect>,
    pub viewport: Viewport,
}

impl PipelineState {
    pub const fn alpha_blend(viewport: Viewport) -> Self {
        Self {
            blend: BlendMode::Alpha,
            scissor: None,
            viewport,
        }
    }
}

/// 显式传入的 2D 可编程管线。
#[derive(Debug, Clone, Copy)]
pub struct Pipeline<VS, FS> {
    pub vertex_shader: VS,
    pub fragment_shader: FS,
    pub state: PipelineState,
}

impl<VS, FS> Pipeline<VS, FS> {
    pub const fn new(vertex_shader: VS, fragment_shader: FS, state: PipelineState) -> Self {
        Self {
            vertex_shader,
            fragment_shader,
            state,
        }
    }
}

/// 用于把顶点属性直接作为 varying 传给片元阶段的便捷顶点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex2D<V> {
    pub position: [f32; 2],
    pub varying: V,
}

/// 直接透传 [`Vertex2D`] 的内建顶点着色器。
#[derive(Debug, Clone, Copy, Default)]
pub struct Passthrough2D<V> {
    _marker: PhantomData<V>,
}

impl<V> Passthrough2D<V> {
    pub const fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<V> VertexShader for Passthrough2D<V>
where
    V: Interpolate + Copy,
{
    type Input = Vertex2D<V>;
    type Varying = V;

    fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying> {
        ProgramVertex {
            position: input.position,
            varying: input.varying,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::PixelFormat;
    use crate::framebuffer::FramebufferInfo;
    use crate::Renderer;
    use alloc::boxed::Box;
    use alloc::vec;

    #[derive(Clone, Copy)]
    struct Vertex {
        position: [f32; 2],
        color: Color,
    }

    struct PassthroughVs;

    impl VertexShader for PassthroughVs {
        type Input = Vertex;
        type Varying = Color;

        fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying> {
            ProgramVertex {
                position: input.position,
                varying: input.color,
            }
        }
    }

    struct SolidFs;

    impl FragmentShader for SolidFs {
        type Varying = Color;

        fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
            FragmentOutput::Color(*varying)
        }
    }

    struct WhiteFs;

    impl FragmentShader for WhiteFs {
        type Varying = ();

        fn shade_fragment(&self, _: &Self::Varying) -> FragmentOutput {
            FragmentOutput::Color(Color::new(255, 255, 255))
        }
    }

    struct DiscardRightHalfFs;

    impl FragmentShader for DiscardRightHalfFs {
        type Varying = [f32; 2];

        fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
            if varying[0] > 0.5 {
                FragmentOutput::Discard
            } else {
                FragmentOutput::Color(Color::new(255, 255, 255))
            }
        }
    }

    #[derive(Clone, Copy)]
    struct PositionVs;

    impl VertexShader for PositionVs {
        type Input = [f32; 2];
        type Varying = [f32; 2];

        fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying> {
            ProgramVertex {
                position: *input,
                varying: *input,
            }
        }
    }

    #[derive(Clone, Copy)]
    struct UvVertex {
        position: [f32; 2],
        uv: [f32; 2],
    }

    struct UvVs;

    impl VertexShader for UvVs {
        type Input = UvVertex;
        type Varying = [f32; 2];

        fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying> {
            ProgramVertex {
                position: input.position,
                varying: input.uv,
            }
        }
    }

    struct TextureFs<'a> {
        texture: Texture2D<'a>,
    }

    impl FragmentShader for TextureFs<'_> {
        type Varying = [f32; 2];

        fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
            FragmentOutput::Color(self.texture.sample_nearest(*varying))
        }
    }

    fn test_renderer(width: usize, height: usize) -> Renderer<'static> {
        let info = FramebufferInfo::new(width, height, width * 4, PixelFormat::Rgba8888);
        let buffer = vec![0u8; info.buffer_size()].into_boxed_slice();
        let front = Box::leak(buffer);
        Renderer::new(info, front)
    }

    #[test]
    fn interpolate_color_varying() {
        let a = Color::new(255, 0, 0);
        let b = Color::new(0, 255, 0);
        let c = Color::new(0, 0, 255);

        let out = Color::interpolate(a, b, c, 0.5, 0.25, 0.25);
        assert_eq!(out, Color::with_alpha(128, 64, 64, 255));
    }

    #[test]
    fn draw_indexed_triangles_rasterizes_quad() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex {
                position: [0.0, 0.0],
                color: Color::new(255, 0, 0),
            },
            Vertex {
                position: [1.0, 0.0],
                color: Color::new(0, 255, 0),
            },
            Vertex {
                position: [1.0, 1.0],
                color: Color::new(0, 0, 255),
            },
            Vertex {
                position: [0.0, 1.0],
                color: Color::new(255, 255, 0),
            },
        ];
        let indices = [0u16, 1, 2, 0, 2, 3];
        let pipeline = Pipeline::new(
            PassthroughVs,
            SolidFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline);

        assert_ne!(renderer.get_pixel(1, 1), Some(Color::new(0, 0, 0)));
        assert_ne!(renderer.get_pixel(2, 2), Some(Color::new(0, 0, 0)));
        assert_ne!(renderer.get_pixel(1, 2), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn fragment_discard_skips_pixels() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let indices = [0u16, 1, 2, 0, 2, 3];
        let pipeline = Pipeline::new(
            PositionVs,
            DiscardRightHalfFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline);

        assert_eq!(renderer.get_pixel(3, 1), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(0, 1), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn scissor_limits_programmable_draw() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex {
                position: [0.0, 0.0],
                color: Color::new(255, 0, 0),
            },
            Vertex {
                position: [1.0, 0.0],
                color: Color::new(0, 255, 0),
            },
            Vertex {
                position: [1.0, 1.0],
                color: Color::new(0, 0, 255),
            },
            Vertex {
                position: [0.0, 1.0],
                color: Color::new(255, 255, 0),
            },
        ];
        let indices = [0u16, 1, 2, 0, 2, 3];
        let pipeline = Pipeline::new(
            PassthroughVs,
            SolidFs,
            PipelineState {
                blend: BlendMode::Alpha,
                scissor: Some(ScissorRect {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 2,
                }),
                viewport: Viewport::new(0, 0, 4, 4),
            },
        );

        renderer.draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline);

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
        assert_ne!(renderer.get_pixel(1, 1), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn draw_arrays_triangles_rasterizes_single_triangle() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: Color::new(255, 0, 0),
            },
            Vertex2D {
                position: [1.0, 0.0],
                varying: Color::new(0, 255, 0),
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: Color::new(0, 0, 255),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            SolidFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline);

        assert_ne!(renderer.get_pixel(0, 1), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn draw_arrays_points_rasterizes_each_vertex() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: (),
            },
            Vertex2D {
                position: [1.0, 1.0],
                varying: (),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            WhiteFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_arrays(PrimitiveType::Points, &vertices, &pipeline);

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn draw_arrays_lines_rasterizes_segment() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: (),
            },
            Vertex2D {
                position: [1.0, 1.0],
                varying: (),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            WhiteFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_arrays(PrimitiveType::Lines, &vertices, &pipeline);

        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(2, 2), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn texture_sampling_shades_fragment_from_uv() {
        let mut renderer = test_renderer(8, 8);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            UvVertex {
                position: [0.0, 0.0],
                uv: [0.0, 0.0],
            },
            UvVertex {
                position: [1.0, 0.0],
                uv: [1.0, 0.0],
            },
            UvVertex {
                position: [1.0, 1.0],
                uv: [1.0, 1.0],
            },
            UvVertex {
                position: [0.0, 1.0],
                uv: [0.0, 1.0],
            },
        ];
        let indices = [0u16, 1, 2, 0, 2, 3];
        let texels = [
            Color::new(255, 0, 0),
            Color::new(0, 255, 0),
            Color::new(0, 0, 255),
            Color::new(255, 255, 255),
        ];
        let texture = Texture2D::new(2, 2, &texels);
        let pipeline = Pipeline::new(
            UvVs,
            TextureFs { texture },
            PipelineState::alpha_blend(Viewport::new(0, 0, 8, 8)),
        );

        renderer.draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline);

        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(255, 0, 0)));
        assert_eq!(renderer.get_pixel(6, 6), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn blend_none_overwrites_destination() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        renderer.set_pixel(1, 1, &Color::new(0, 255, 0));
        let vertices = [
            Vertex2D {
                position: [1.0 / 3.0, 1.0 / 3.0],
                varying: Color::with_alpha(255, 0, 0, 64),
            },
            Vertex2D {
                position: [1.0, 1.0 / 3.0],
                varying: Color::with_alpha(255, 0, 0, 64),
            },
            Vertex2D {
                position: [1.0 / 3.0, 1.0],
                varying: Color::with_alpha(255, 0, 0, 64),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, 4, 4),
            },
        );

        renderer.draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline);

        assert_eq!(renderer.get_pixel(1, 1), Some(Color::with_alpha(255, 0, 0, 64)));
    }
}
