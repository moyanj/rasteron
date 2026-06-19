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
//! - 多种 `BlendMode`
//! - `Texture2D` 的 nearest / bilinear 采样
//!
extern crate alloc;

use crate::color::Color;
use crate::core::Renderer;
use crate::pixel::Pixel;
use crate::utils::{clip_line, floor_to_usize};
use alloc::vec::Vec;
use core::marker::PhantomData;

/// 光栅化原语类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveType {
    Points,
    Lines,
    Triangles,
}

/// 顶点着色器输出到光栅化阶段的数据。
///
/// `position` 使用归一化 viewport 坐标：
///
/// - `x` / `y` 可见范围为 `[0.0, 1.0]`
/// - `[0.0, 0.0]` 对应 viewport 左上角
/// - `[1.0, 1.0]` 对应 viewport 右下角
///
/// 超出这个范围的顶点不会被直接 clamp，而是交给图元裁剪阶段处理。
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

    #[inline(always)]
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        Self::interpolate(a, b, b, 1.0 - t, t, 0.0)
    }
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

/// flat varying 包装器。
///
/// 插值和裁剪生成新顶点时，始终保留 provoking vertex（当前固定为图元第一个顶点）
/// 的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flat<T>(pub T);

impl<T: Copy> Interpolate for Flat<T> {
    #[inline(always)]
    fn interpolate(a: Self, _: Self, _: Self, _: f32, _: f32, _: f32) -> Self {
        a
    }

    #[inline(always)]
    fn lerp(a: Self, _: Self, _: f32) -> Self {
        a
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
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// 视口。
pub type Viewport = Rect;
pub type ScissorRect = Rect;

impl Rect {
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
    Add,
    Multiply,
    PremultipliedAlpha,
}

/// 纹理采样环绕模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    Clamp,
    Repeat,
}

/// 纹理采样过滤模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Nearest,
    Bilinear,
}

/// 采样状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler2D {
    pub wrap_u: WrapMode,
    pub wrap_v: WrapMode,
    pub filter: FilterMode,
}

impl Sampler2D {
    pub const fn nearest_clamp() -> Self {
        Self {
            wrap_u: WrapMode::Clamp,
            wrap_v: WrapMode::Clamp,
            filter: FilterMode::Nearest,
        }
    }

    pub const fn bilinear_clamp() -> Self {
        Self {
            wrap_u: WrapMode::Clamp,
            wrap_v: WrapMode::Clamp,
            filter: FilterMode::Bilinear,
        }
    }
}

impl Default for Sampler2D {
    fn default() -> Self {
        Self::nearest_clamp()
    }
}

/// `Texture2D` 构造错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureError {
    DimensionOverflow,
    LengthMismatch {
        expected: usize,
        actual: usize,
    },
}

/// 2D 纹理。
///
/// 纹理坐标语义固定为：
///
/// - `uv` 通常以 `[0.0, 1.0]` 表示整个纹理
/// - `[0.0, 0.0]` 对应左上角 texel 中心附近
/// - `[1.0, 1.0]` 对应右下角 texel 中心附近
/// - `u` 向右增大，`v` 向下增大
///
/// 默认便捷采样 `sample_nearest` / `sample_bilinear` 使用 clamp 到边界。
#[derive(Debug, Clone, Copy)]
pub struct Texture2D<'a> {
    width: usize,
    height: usize,
    texels: &'a [Color],
}

impl<'a> Texture2D<'a> {
    pub fn new(width: usize, height: usize, texels: &'a [Color]) -> Self {
        match Self::try_new(width, height, texels) {
            Ok(texture) => texture,
            Err(TextureError::DimensionOverflow) => {
                panic!("texture dimensions overflow: {}x{}", width, height)
            }
            Err(TextureError::LengthMismatch { expected, actual }) => {
                panic!(
                    "texture texel length mismatch: expected {}, got {}",
                    expected, actual
                )
            }
        }
    }

    pub fn try_new(width: usize, height: usize, texels: &'a [Color]) -> Result<Self, TextureError> {
        let expected = width
            .checked_mul(height)
            .ok_or(TextureError::DimensionOverflow)?;
        if texels.len() != expected {
            return Err(TextureError::LengthMismatch {
                expected,
                actual: texels.len(),
            });
        }

        Ok(Self {
            width,
            height,
            texels,
        })
    }

    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub fn sample_nearest(&self, uv: [f32; 2]) -> Color {
        self.sample(uv, Sampler2D::nearest_clamp())
    }

    pub fn sample_bilinear(&self, uv: [f32; 2]) -> Color {
        self.sample(uv, Sampler2D::bilinear_clamp())
    }

    pub fn sample(&self, uv: [f32; 2], sampler: Sampler2D) -> Color {
        if self.width == 0 || self.height == 0 {
            return Color::with_alpha(0, 0, 0, 0);
        }

        match sampler.filter {
            FilterMode::Nearest => self.sample_nearest_with_wrap(uv, sampler.wrap_u, sampler.wrap_v),
            FilterMode::Bilinear => {
                self.sample_bilinear_with_wrap(uv, sampler.wrap_u, sampler.wrap_v)
            }
        }
    }

    fn sample_nearest_with_wrap(&self, uv: [f32; 2], wrap_u: WrapMode, wrap_v: WrapMode) -> Color {
        let u = self.wrap_coord(uv[0], wrap_u);
        let v = self.wrap_coord(uv[1], wrap_v);
        let x = (u * (self.width.saturating_sub(1) as f32) + 0.5) as usize;
        let y = (v * (self.height.saturating_sub(1) as f32) + 0.5) as usize;
        self.texel(x, y)
    }

    fn sample_bilinear_with_wrap(
        &self,
        uv: [f32; 2],
        wrap_u: WrapMode,
        wrap_v: WrapMode,
    ) -> Color {
        let u = self.wrap_coord(uv[0], wrap_u) * self.width.saturating_sub(1) as f32;
        let v = self.wrap_coord(uv[1], wrap_v) * self.height.saturating_sub(1) as f32;
        let x0 = floor_to_usize(u);
        let y0 = floor_to_usize(v);
        let x1 = self.wrap_texel_index(x0.saturating_add(1), self.width, wrap_u);
        let y1 = self.wrap_texel_index(y0.saturating_add(1), self.height, wrap_v);
        let tx = u - x0 as f32;
        let ty = v - y0 as f32;

        let c00 = self.texel(x0, y0);
        let c10 = self.texel(x1, y0);
        let c01 = self.texel(x0, y1);
        let c11 = self.texel(x1, y1);

        fn blend_channel(a: u8, b: u8, t: f32) -> f32 {
            a as f32 * (1.0 - t) + b as f32 * t
        }

        fn color_lerp(a: Color, b: Color, t: f32) -> [f32; 4] {
            [
                blend_channel(a.r, b.r, t),
                blend_channel(a.g, b.g, t),
                blend_channel(a.b, b.b, t),
                blend_channel(a.a, b.a, t),
            ]
        }

        let top = color_lerp(c00, c10, tx);
        let bottom = color_lerp(c01, c11, tx);
        Color::with_alpha(
            (top[0] * (1.0 - ty) + bottom[0] * ty + 0.5).clamp(0.0, 255.0) as u8,
            (top[1] * (1.0 - ty) + bottom[1] * ty + 0.5).clamp(0.0, 255.0) as u8,
            (top[2] * (1.0 - ty) + bottom[2] * ty + 0.5).clamp(0.0, 255.0) as u8,
            (top[3] * (1.0 - ty) + bottom[3] * ty + 0.5).clamp(0.0, 255.0) as u8,
        )
    }

    #[inline(always)]
    fn texel(&self, x: usize, y: usize) -> Color {
        self.texels[y * self.width + x]
    }

    #[inline(always)]
    fn wrap_coord(&self, coord: f32, mode: WrapMode) -> f32 {
        match mode {
            WrapMode::Clamp => coord.clamp(0.0, 1.0),
            WrapMode::Repeat => Self::repeat_unit(coord),
        }
    }

    #[inline(always)]
    fn wrap_texel_index(&self, index: usize, extent: usize, mode: WrapMode) -> usize {
        match mode {
            WrapMode::Clamp => index.min(extent.saturating_sub(1)),
            WrapMode::Repeat => {
                if extent == 0 {
                    0
                } else {
                    index % extent
                }
            }
        }
    }

    #[inline(always)]
    fn repeat_unit(value: f32) -> f32 {
        if value >= 0.0 {
            value - value as usize as f32
        } else {
            let integer = value as i32;
            let frac = value - integer as f32;
            if frac == 0.0 { 0.0 } else { frac + 1.0 }
        }
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

impl<V: Copy> From<Vertex2D<V>> for ProgramVertex<V> {
    fn from(value: Vertex2D<V>) -> Self {
        Self {
            position: value.position,
            varying: value.varying,
        }
    }
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
        (*input).into()
    }
}

#[derive(Clone, Copy)]
struct ClippedLine<V> {
    start: ProgramVertex<V>,
    end: ProgramVertex<V>,
}

#[derive(Clone, Copy)]
struct ClippedTriangle<V> {
    vertices: [ProgramVertex<V>; 3],
}

impl<'a> Renderer<'a> {
    #[inline(always)]
    fn framebuffer_viewport(&self) -> Viewport {
        Viewport::new(0, 0, self.info().width, self.info().height)
    }

    #[inline(always)]
    fn ndc_to_screen(position: [f32; 2], viewport: Viewport) -> [f32; 2] {
        let vx = viewport.x as f32;
        let vy = viewport.y as f32;
        let vw = viewport.width.max(1) as f32;
        let vh = viewport.height.max(1) as f32;
        [vx + position[0] * (vw - 1.0), vy + position[1] * (vh - 1.0)]
    }

    #[inline(always)]
    fn edge_function(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
        (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
    }

    #[inline(always)]
    fn ceil_to_usize(value: f32) -> usize {
        if value <= 0.0 {
            return 0;
        }
        let truncated = value as usize;
        if truncated as f32 == value {
            truncated
        } else {
            truncated.saturating_add(1)
        }
    }

    #[inline(always)]
    fn round_to_usize(value: f32) -> usize {
        floor_to_usize(value + 0.5)
    }

    #[inline(always)]
    fn viewport_intersection(&self, viewport: Viewport) -> Option<(usize, usize, usize, usize)> {
        let fb = self.framebuffer_viewport();
        let min_x = viewport.x.max(fb.x);
        let min_y = viewport.y.max(fb.y);
        let max_x = viewport
            .x
            .saturating_add(viewport.width)
            .min(fb.x.saturating_add(fb.width));
        let max_y = viewport
            .y
            .saturating_add(viewport.height)
            .min(fb.y.saturating_add(fb.height));
        if min_x >= max_x || min_y >= max_y {
            None
        } else {
            Some((min_x, min_y, max_x, max_y))
        }
    }

    #[inline(always)]
    fn scissor_contains(rect: Option<ScissorRect>, x: usize, y: usize) -> bool {
        match rect {
            Some(rect) => {
                let x_max = rect.x.saturating_add(rect.width);
                let y_max = rect.y.saturating_add(rect.height);
                x >= rect.x && y >= rect.y && x < x_max && y < y_max
            }
            None => true,
        }
    }

    #[inline(always)]
    fn write_color_raw_at_offset(&mut self, offset: usize, color: &Color) {
        let format = self.info().format;
        let bpp = format.bytes_per_pixel();
        let bytes = color.to_bytes(format);
        self.current_buffer_mut()[offset..offset + bpp].copy_from_slice(&bytes[..bpp]);
    }

    #[inline(always)]
    fn read_color_at_offset(&self, offset: usize) -> Color {
        let format = self.info().format;
        let bpp = format.bytes_per_pixel();
        Color::from_bytes(format, &self.current_buffer()[offset..offset + bpp])
    }

    #[inline(always)]
    fn blend_pixel(src: Color, dst: Color, blend: BlendMode) -> Color {
        fn add_channel(src: u8, dst: u8) -> u8 {
            src.saturating_add(dst)
        }
        fn mul_channel(src: u8, dst: u8) -> u8 {
            ((src as u16 * dst as u16 + 127) / 255) as u8
        }

        match blend {
            BlendMode::None => src,
            BlendMode::Alpha => src.mix(&dst, src.a),
            BlendMode::Add => Color::with_alpha(
                add_channel(src.r, dst.r),
                add_channel(src.g, dst.g),
                add_channel(src.b, dst.b),
                add_channel(src.a, dst.a),
            ),
            BlendMode::Multiply => Color::with_alpha(
                mul_channel(src.r, dst.r),
                mul_channel(src.g, dst.g),
                mul_channel(src.b, dst.b),
                mul_channel(src.a, dst.a),
            ),
            BlendMode::PremultipliedAlpha => {
                let inv_alpha = 255 - src.a;
                Color::with_alpha(
                    src.r.saturating_add((dst.r as u16 * inv_alpha as u16 / 255) as u8),
                    src.g.saturating_add((dst.g as u16 * inv_alpha as u16 / 255) as u8),
                    src.b.saturating_add((dst.b as u16 * inv_alpha as u16 / 255) as u8),
                    src.a.saturating_add((dst.a as u16 * inv_alpha as u16 / 255) as u8),
                )
            }
        }
    }

    #[inline(always)]
    fn write_fragment_color(&mut self, x: usize, y: usize, color: &Color, blend: BlendMode) {
        if let Some(offset) = self.try_pixel_offset_i64(x as i64, y as i64) {
            match blend {
                BlendMode::None => self.write_color_raw_at_offset(offset, color),
                _ => {
                    let dst = self.read_color_at_offset(offset);
                    self.write_color_raw_at_offset(offset, &Self::blend_pixel(*color, dst, blend));
                }
            }
        }
    }

    #[inline(always)]
    fn clip_position_code(position: [f32; 2]) -> u8 {
        const LEFT: u8 = 1;
        const RIGHT: u8 = 2;
        const TOP: u8 = 4;
        const BOTTOM: u8 = 8;

        let mut code = 0;
        if position[0] < 0.0 {
            code |= LEFT;
        } else if position[0] > 1.0 {
            code |= RIGHT;
        }
        if position[1] < 0.0 {
            code |= TOP;
        } else if position[1] > 1.0 {
            code |= BOTTOM;
        }
        code
    }

    #[inline(always)]
    fn interpolate_vertex<V: Interpolate + Copy>(
        a: ProgramVertex<V>,
        b: ProgramVertex<V>,
        t: f32,
    ) -> ProgramVertex<V> {
        ProgramVertex {
            position: [
                a.position[0] + (b.position[0] - a.position[0]) * t,
                a.position[1] + (b.position[1] - a.position[1]) * t,
            ],
            varying: V::lerp(a.varying, b.varying, t),
        }
    }

    fn clip_line_segment<V: Interpolate + Copy>(
        vertices: [ProgramVertex<V>; 2],
    ) -> Option<ClippedLine<V>> {
        const SCALE: f32 = 10000.0;
        const OFFSET: usize = 10000;
        const DIM: usize = OFFSET * 3;

        let to_pixel = |pos: [f32; 2]| -> (usize, usize) {
            (
                (pos[0] * SCALE + OFFSET as f32).max(0.0) as usize,
                (pos[1] * SCALE + OFFSET as f32).max(0.0) as usize,
            )
        };
        let to_ndc = |px: usize| -> f32 { (px as f32 - OFFSET as f32) / SCALE };

        let (px0, py0) = to_pixel(vertices[0].position);
        let (px1, py1) = to_pixel(vertices[1].position);
        let (cp0, cp1) = clip_line(Pixel::new(px0, py0), Pixel::new(px1, py1), DIM, DIM)?;
        let pos0 = [to_ndc(cp0.x), to_ndc(cp0.y)];
        let pos1 = [to_ndc(cp1.x), to_ndc(cp1.y)];
        let dx = vertices[1].position[0] - vertices[0].position[0];
        let dy = vertices[1].position[1] - vertices[0].position[1];
        let (t0, t1) = if dx.abs() >= dy.abs() && dx.abs() > f32::EPSILON {
            (
                (pos0[0] - vertices[0].position[0]) / dx,
                (pos1[0] - vertices[0].position[0]) / dx,
            )
        } else if dy.abs() > f32::EPSILON {
            (
                (pos0[1] - vertices[0].position[1]) / dy,
                (pos1[1] - vertices[0].position[1]) / dy,
            )
        } else {
            (0.0, 1.0)
        };

        Some(ClippedLine {
            start: ProgramVertex {
                position: pos0,
                varying: V::lerp(vertices[0].varying, vertices[1].varying, t0),
            },
            end: ProgramVertex {
                position: pos1,
                varying: V::lerp(vertices[0].varying, vertices[1].varying, t1),
            },
        })
    }

    fn clip_triangle<V: Interpolate + Copy>(
        vertices: [ProgramVertex<V>; 3],
    ) -> Option<Vec<ClippedTriangle<V>>> {
        const MAX_VERTICES: usize = 8;
        let mut input = [vertices[0]; MAX_VERTICES];
        input[0] = vertices[0];
        input[1] = vertices[1];
        input[2] = vertices[2];
        let mut input_len = 3usize;
        let mut output = [vertices[0]; MAX_VERTICES];

        for edge in 0..4 {
            if input_len == 0 {
                return None;
            }

            let mut output_len = 0usize;
            let mut prev = input[input_len - 1];
            let mut prev_inside = Self::clip_triangle_inside(prev.position, edge);

            for index in 0..input_len {
                let curr = input[index];
                let curr_inside = Self::clip_triangle_inside(curr.position, edge);

                if curr_inside {
                    if !prev_inside {
                        output[output_len] = Self::clip_triangle_intersection(prev, curr, edge);
                        output_len += 1;
                    }
                    output[output_len] = curr;
                    output_len += 1;
                } else if prev_inside {
                    output[output_len] = Self::clip_triangle_intersection(prev, curr, edge);
                    output_len += 1;
                }

                prev = curr;
                prev_inside = curr_inside;
            }

            input[..output_len].copy_from_slice(&output[..output_len]);
            input_len = output_len;
        }

        if input_len < 3 {
            return None;
        }

        let mut triangles = Vec::new();
        for index in 1..input_len - 1 {
            triangles.push(ClippedTriangle {
                vertices: [input[0], input[index], input[index + 1]],
            });
        }
        Some(triangles)
    }

    #[inline(always)]
    fn clip_triangle_inside(position: [f32; 2], edge: usize) -> bool {
        match edge {
            0 => position[0] >= 0.0,
            1 => position[0] <= 1.0,
            2 => position[1] >= 0.0,
            _ => position[1] <= 1.0,
        }
    }

    fn clip_triangle_intersection<V: Interpolate + Copy>(
        a: ProgramVertex<V>,
        b: ProgramVertex<V>,
        edge: usize,
    ) -> ProgramVertex<V> {
        let t = match edge {
            0 => (0.0 - a.position[0]) / (b.position[0] - a.position[0]),
            1 => (1.0 - a.position[0]) / (b.position[0] - a.position[0]),
            2 => (0.0 - a.position[1]) / (b.position[1] - a.position[1]),
            _ => (1.0 - a.position[1]) / (b.position[1] - a.position[1]),
        };
        Self::interpolate_vertex(a, b, t)
    }

    #[inline(always)]
    fn shade_vertex_point<V, FS>(
        &mut self,
        vertex: ProgramVertex<V>,
        pipeline: &Pipeline<impl VertexShader<Input = impl Copy, Varying = V>, FS>,
    ) where
        V: Copy,
        FS: FragmentShader<Varying = V>,
    {
        let Some((clip_min_x, clip_min_y, clip_max_x, clip_max_y)) =
            self.viewport_intersection(pipeline.state.viewport)
        else {
            return;
        };
        if Self::clip_position_code(vertex.position) != 0 {
            return;
        }

        let point = Self::ndc_to_screen(vertex.position, pipeline.state.viewport);
        let x = point[0] as usize;
        let y = point[1] as usize;
        if x < clip_min_x || x >= clip_max_x || y < clip_min_y || y >= clip_max_y {
            return;
        }
        if !Self::scissor_contains(pipeline.state.scissor, x, y) {
            return;
        }

        if let FragmentOutput::Color(color) = pipeline.fragment_shader.shade_fragment(&vertex.varying)
        {
            self.write_fragment_color(x, y, &color, pipeline.state.blend);
        }
    }

    fn rasterize_line<V, FS>(
        &mut self,
        vertices: [ProgramVertex<V>; 2],
        pipeline: &Pipeline<impl VertexShader<Input = impl Copy, Varying = V>, FS>,
    ) where
        V: Interpolate + Copy,
        FS: FragmentShader<Varying = V>,
    {
        let Some(clipped) = Self::clip_line_segment(vertices) else {
            return;
        };
        let Some((clip_min_x, clip_min_y, clip_max_x, clip_max_y)) =
            self.viewport_intersection(pipeline.state.viewport)
        else {
            return;
        };

        let p0 = Self::ndc_to_screen(clipped.start.position, pipeline.state.viewport);
        let p1 = Self::ndc_to_screen(clipped.end.position, pipeline.state.viewport);
        let dx = p1[0] - p0[0];
        let dy = p1[1] - p0[1];
        let steps = Self::ceil_to_usize(dx.abs().max(dy.abs()));
        if steps == 0 {
            self.shade_vertex_point(clipped.start, pipeline);
            return;
        }

        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let px = Self::round_to_usize(p0[0] + dx * t);
            let py = Self::round_to_usize(p0[1] + dy * t);
            if px < clip_min_x || px >= clip_max_x || py < clip_min_y || py >= clip_max_y {
                continue;
            }
            if !Self::scissor_contains(pipeline.state.scissor, px, py) {
                continue;
            }

            let varying = V::lerp(clipped.start.varying, clipped.end.varying, t);
            if let FragmentOutput::Color(color) = pipeline.fragment_shader.shade_fragment(&varying) {
                self.write_fragment_color(px, py, &color, pipeline.state.blend);
            }
        }
    }

    fn rasterize_triangle<V, FS>(
        &mut self,
        vertices: [ProgramVertex<V>; 3],
        pipeline: &Pipeline<impl VertexShader<Input = impl Copy, Varying = V>, FS>,
    ) where
        V: Interpolate + Copy,
        FS: FragmentShader<Varying = V>,
    {
        let Some(clipped_tris) = Self::clip_triangle(vertices) else {
            return;
        };
        let Some((clip_min_x, clip_min_y, clip_max_x, clip_max_y)) =
            self.viewport_intersection(pipeline.state.viewport)
        else {
            return;
        };

        for clipped in clipped_tris {
            let vertices = clipped.vertices;
            let p0 = Self::ndc_to_screen(vertices[0].position, pipeline.state.viewport);
            let p1 = Self::ndc_to_screen(vertices[1].position, pipeline.state.viewport);
            let p2 = Self::ndc_to_screen(vertices[2].position, pipeline.state.viewport);

            let min_x = floor_to_usize(p0[0].min(p1[0]).min(p2[0]).max(clip_min_x as f32));
            let min_y = floor_to_usize(p0[1].min(p1[1]).min(p2[1]).max(clip_min_y as f32));
            let max_x = Self::ceil_to_usize(p0[0].max(p1[0]).max(p2[0]).min((clip_max_x - 1) as f32));
            let max_y = Self::ceil_to_usize(p0[1].max(p1[1]).max(p2[1]).min((clip_max_y - 1) as f32));
            if min_x > max_x || min_y > max_y {
                continue;
            }

            let area = Self::edge_function(p0, p1, p2);
            if area.abs() <= f32::EPSILON {
                continue;
            }

            let sign = if area < 0.0 { -1.0 } else { 1.0 };
            let inv_area = 1.0 / area.abs();
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    if !Self::scissor_contains(pipeline.state.scissor, x, y) {
                        continue;
                    }

                    let sample = [x as f32 + 0.5, y as f32 + 0.5];
                    let w0 = Self::edge_function(p1, p2, sample) * sign;
                    let w1 = Self::edge_function(p2, p0, sample) * sign;
                    let w2 = Self::edge_function(p0, p1, sample) * sign;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }

                    let varying = V::interpolate(
                        vertices[0].varying,
                        vertices[1].varying,
                        vertices[2].varying,
                        w0 * inv_area,
                        w1 * inv_area,
                        w2 * inv_area,
                    );
                    match pipeline.fragment_shader.shade_fragment(&varying) {
                        FragmentOutput::Color(color) => {
                            self.write_fragment_color(x, y, &color, pipeline.state.blend);
                        }
                        FragmentOutput::Discard => {}
                    }
                }
            }
        }
    }

    pub fn draw_arrays<VS, FS>(
        &mut self,
        primitive: PrimitiveType,
        vertices: &[VS::Input],
        pipeline: &Pipeline<VS, FS>,
    ) where
        VS: VertexShader,
        VS::Input: Copy,
        VS::Varying: Interpolate + Copy,
        FS: FragmentShader<Varying = VS::Varying>,
    {
        match primitive {
            PrimitiveType::Points => {
                for vertex in vertices {
                    self.shade_vertex_point(pipeline.vertex_shader.shade_vertex(vertex), pipeline);
                }
            }
            PrimitiveType::Lines => {
                let mut index = 0usize;
                while index + 1 < vertices.len() {
                    self.rasterize_line(
                        [
                            pipeline.vertex_shader.shade_vertex(&vertices[index]),
                            pipeline.vertex_shader.shade_vertex(&vertices[index + 1]),
                        ],
                        pipeline,
                    );
                    index += 2;
                }
            }
            PrimitiveType::Triangles => {
                let mut index = 0usize;
                while index + 2 < vertices.len() {
                    self.rasterize_triangle(
                        [
                            pipeline.vertex_shader.shade_vertex(&vertices[index]),
                            pipeline.vertex_shader.shade_vertex(&vertices[index + 1]),
                            pipeline.vertex_shader.shade_vertex(&vertices[index + 2]),
                        ],
                        pipeline,
                    );
                    index += 3;
                }
            }
        }
    }

    pub fn draw_indexed<VS, FS>(
        &mut self,
        primitive: PrimitiveType,
        vertices: &[VS::Input],
        indices: &[u16],
        pipeline: &Pipeline<VS, FS>,
    ) where
        VS: VertexShader,
        VS::Input: Copy,
        VS::Varying: Interpolate + Copy,
        FS: FragmentShader<Varying = VS::Varying>,
    {
        match primitive {
            PrimitiveType::Points => {
                for index in indices {
                    let vertex_index = *index as usize;
                    if vertex_index < vertices.len() {
                        self.shade_vertex_point(
                            pipeline.vertex_shader.shade_vertex(&vertices[vertex_index]),
                            pipeline,
                        );
                    }
                }
            }
            PrimitiveType::Lines => {
                let mut index = 0usize;
                while index + 1 < indices.len() {
                    let i0 = indices[index] as usize;
                    let i1 = indices[index + 1] as usize;
                    if i0 < vertices.len() && i1 < vertices.len() {
                        self.rasterize_line(
                            [
                                pipeline.vertex_shader.shade_vertex(&vertices[i0]),
                                pipeline.vertex_shader.shade_vertex(&vertices[i1]),
                            ],
                            pipeline,
                        );
                    }
                    index += 2;
                }
            }
            PrimitiveType::Triangles => {
                let mut index = 0usize;
                while index + 2 < indices.len() {
                    let i0 = indices[index] as usize;
                    let i1 = indices[index + 1] as usize;
                    let i2 = indices[index + 2] as usize;
                    if i0 < vertices.len() && i1 < vertices.len() && i2 < vertices.len() {
                        self.rasterize_triangle(
                            [
                                pipeline.vertex_shader.shade_vertex(&vertices[i0]),
                                pipeline.vertex_shader.shade_vertex(&vertices[i1]),
                                pipeline.vertex_shader.shade_vertex(&vertices[i2]),
                            ],
                            pipeline,
                        );
                    }
                    index += 3;
                }
            }
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

    #[test]
    fn texture_try_new_validates_texel_length() {
        let texels = [Color::new(255, 0, 0); 3];
        let error = Texture2D::try_new(2, 2, &texels).unwrap_err();
        assert_eq!(
            error,
            TextureError::LengthMismatch {
                expected: 4,
                actual: 3,
            }
        );
    }

    #[test]
    fn texture_repeat_and_bilinear_sampling_work() {
        let texels = [
            Color::new(255, 0, 0),
            Color::new(0, 255, 0),
            Color::new(0, 0, 255),
            Color::new(255, 255, 255),
        ];
        let texture = Texture2D::new(2, 2, &texels);

        let repeated = texture.sample(
            [1.25, -0.25],
            Sampler2D {
                wrap_u: WrapMode::Repeat,
                wrap_v: WrapMode::Repeat,
                filter: FilterMode::Nearest,
            },
        );
        assert_eq!(repeated, Color::new(0, 0, 255));

        let bilinear = texture.sample_bilinear([0.5, 0.5]);
        assert_eq!(bilinear, Color::with_alpha(128, 128, 128, 255));
    }

    #[test]
    fn flat_varying_uses_first_vertex_value() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: Flat(Color::new(255, 0, 0)),
            },
            Vertex2D {
                position: [1.0, 0.0],
                varying: Flat(Color::new(0, 255, 0)),
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: Flat(Color::new(0, 0, 255)),
            },
        ];

        struct FlatFs;

        impl FragmentShader for FlatFs {
            type Varying = Flat<Color>;

            fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
                FragmentOutput::Color(varying.0)
            }
        }

        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            FlatFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );
        renderer.draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline);

        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(255, 0, 0)));
    }

    #[test]
    fn line_clipping_rasterizes_partially_visible_segment() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex2D {
                position: [-0.5, 0.5],
                varying: (),
            },
            Vertex2D {
                position: [0.5, 0.5],
                varying: (),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            WhiteFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_arrays(PrimitiveType::Lines, &vertices, &pipeline);

        assert_eq!(renderer.get_pixel(0, 2), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(1, 2), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn triangle_clipping_rasterizes_partially_visible_triangle() {
        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex2D {
                position: [-0.5, 0.0],
                varying: Color::new(255, 0, 0),
            },
            Vertex2D {
                position: [0.5, 0.0],
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

        assert_ne!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
        assert_ne!(renderer.get_pixel(0, 1), Some(Color::new(0, 0, 0)));
    }
}
