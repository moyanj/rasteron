use crate::utils::normalized_to_screen;
use crate::{Renderer, color::Color, error::DrawError};
pub mod shader;

pub type Vec2 = [f32; 2];
pub type Vec3 = [f32; 3];
pub type Vec4 = [f32; 4];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrimitiveType {
    Points,
    Lines,
    Triangles,
}

impl PrimitiveType {
    pub fn to_step(self) -> u8 {
        match self {
            PrimitiveType::Points => 1,
            PrimitiveType::Lines => 2,
            PrimitiveType::Triangles => 3,
        }
    }
}

/// varying 插值 trait。
///
/// 光栅化阶段用重心坐标 `(w0, w1, w2)` 对三个顶点的属性做线性组合，
/// `w0 + w1 + w2 == 1`。各实现必须保证 `Self::interpolate(a, a, a, ..) == a`，
/// 否则同一片元在不同顶点顺序下会得到不同结果。
pub trait Interpolate: Copy {
    /// 按重心权重组合三个顶点属性。
    ///
    /// `w0` / `w1` / `w2` 分别对应 `a` / `b` / `c`，三者之和为 `1.0`。
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self;

    /// 在两属性之间线性插值，`t = 0` 取 `a`、`t = 1` 取 `b`。
    ///
    /// 采用 `interpolate(a, b, b, 1 - t, t, 0)` 的等价形式实现，
    /// 这样自定义类型只需实现 [`Interpolate::interpolate`] 即可获得线性插值。
    #[inline(always)]
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        Self::interpolate(a, b, b, 1.0 - t, t, 0.0)
    }
}

/// [`Color`] 的插值实现：四个通道各自独立地按重心权重线性组合。
///
/// 通道值以 `f32` 参与运算，结果四舍五入后钳制到 `[0, 255]`，
/// 因此三角形内部的渐变不会因累加而溢出或产生负值。
impl Interpolate for Color {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        /// 单通道插值：加权求和后四舍五入并钳制到 `u8` 值域。
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

/// `f32` 的插值实现：单个标量属性的加权求和。
impl Interpolate for f32 {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        a * w0 + b * w1 + c * w2
    }
}

/// 二维向量的插值实现：两个分量各自独立插值。
///
/// 典型用途是把 UV 坐标作为 varying 传递，交由片元着色器采样纹理。
impl Interpolate for Vec2 {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        [
            a[0] * w0 + b[0] * w1 + c[0] * w2,
            a[1] * w0 + b[1] * w1 + c[1] * w2,
        ]
    }
}

/// 三维向量的插值实现：三个分量各自独立插值。
impl Interpolate for Vec3 {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        [
            a[0] * w0 + b[0] * w1 + c[0] * w2,
            a[1] * w0 + b[1] * w1 + c[1] * w2,
            a[2] * w0 + b[2] * w1 + c[2] * w2,
        ]
    }
}

/// 四维向量的插值实现：四个分量各自独立插值。
impl Interpolate for Vec4 {
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
///
/// 用于「整个图元共享同一属性」的语义，例如常量材质索引或拾取 ID；
/// 若不做 flat 包装，这些值会在顶点间被错误地渐变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flat<T>(pub T);

impl<T: Copy> Interpolate for Flat<T> {
    /// 忽略权重，恒取第一个顶点（provoking vertex）的值。
    #[inline(always)]
    fn interpolate(a: Self, _: Self, _: Self, _: f32, _: f32, _: f32) -> Self {
        a
    }

    /// 裁剪插入的新顶点同样沿用起始顶点的值，避免裁剪改变 flat 语义。
    #[inline(always)]
    fn lerp(a: Self, _: Self, _: f32) -> Self {
        a
    }
}

/// 片元着色器输入。
///
/// `position` 使用归一化 viewport 坐标：
///
/// - `x` / `y` 可见范围为 `[0.0, 1.0]`
/// - `[0.0, 0.0]` 对应 viewport 左上角
/// - `[1.0, 1.0]` 对应 viewport 右下角
///
/// 超出这个范围的顶点不会被直接 clamp，而是交给图元裁剪阶段处理。
///
/// `varying` 是该顶点的插值属性，会在光栅化时按重心权重在三个顶点间插值，
/// 因此其类型必须实现 [`Interpolate`]。
///
/// # 格式
///
/// `[0, 1]` 是 normalized viewport 坐标而非 OpenGL 的 `[-1, 1]` clip space；
/// 后者被映射到 viewport 的那一步由 `ndc_to_screen` 在此约定下完成。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FragmentInput<Varying: Interpolate> {
    pub position: Vec2,
    pub varying: Varying,
}

pub type Vertex2D<V> = FragmentInput<V>;

/// 片元着色器输出。
///
/// `Discard` 表示丢弃该片元、不产生任何写入，可用于实现模板/镂空效果；
/// 它只影响当前片元，不会终止后续片元的着色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentOutput {
    /// 输出该颜色；是否真正落到帧缓冲由 [`BlendMode`] 决定。
    Color(Color),
    /// 丢弃当前片元，不写入帧缓冲。
    Discard,
}

/// 顶点着色器：输入用户顶点，输出归一化坐标与 varying。
///
/// # 副作用约束
/// `shade_vertex` 必须是纯函数：`draw_indexed` 的缓存机制（§4.6）按**顶点索引**
/// 复用其结果，非纯实现会导致缓存命中的顶点与未命中的行为不一致。
pub trait VertexShader {
    type Input;
    type Varying: Interpolate;
    fn shade_vertex(&self, input: &Self::Input) -> FragmentInput<Self::Varying>;
}

/// 片元着色器。
///
/// 接收插值后的 varying 与片元屏幕坐标，输出颜色或丢弃该片元。
pub trait FragmentShader {
    /// 输入的 varying 类型，必须与顶点着色器的 [`VertexShader::Varying`] 一致。
    type Varying: Copy + Interpolate;

    /// 对单个片元执行着色。
    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput;
}

/// 混合模式：决定片元颜色如何与帧缓冲中已有颜色组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    /// 直接覆盖目标像素，不做任何混合。
    None,
    /// 标准 Alpha 混合：`src × src.a + dst × (1 - src.a)`。
    Alpha,
    /// 加法混合：各通道饱和相加，常用于发光/叠加效果。
    Add,
    /// 乘法混合：各通道相乘，用于阴影/染色。
    Multiply,
    /// 预乘 Alpha 混合：片元颜色被视为已乘过 `src.a`，只对目标做 `× (1 - src.a)` 衰减。
    PremultipliedAlpha,
}

/// 裁剪矩形。
///
/// 同时用作 [`Viewport`] 与 [`ScissorRect`]，字段语义随用途而定：
/// 作为 viewport 时表示 normalized 坐标映射到的屏幕矩形，作为 scissor 时
/// 表示片元允许写入的屏幕矩形（超出部分被丢弃）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// 矩形左边界（含）的 X 像素坐标。
    pub x: usize,
    /// 矩形上边界（含）的 Y 像素坐标。
    pub y: usize,
    /// 矩形宽度（像素）。
    pub width: usize,
    /// 矩形高度（像素）。
    pub height: usize,
}

impl Rect {
    /// 创建矩形。
    ///
    /// 不校验 `x + width` / `y + height` 是否溢出，越界部分由后续的
    /// `rect_intersection` 与帧缓冲边界求交处理。
    pub const fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// 视口：把 `[0, 1]` 的 normalized 坐标映射到的屏幕矩形。
pub type Viewport = Rect;
/// 裁剪矩形：片元允许写入的屏幕区域，超出部分既不写着色器也不写帧缓冲。
pub type ScissorRect = Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bounds {
    /// 可见区域左边界（含）。
    min_x: usize,
    /// 可见区域上边界（含）。
    min_y: usize,
    /// 可见区域右边界（不含）。
    max_x: usize,
    /// 可见区域下边界（不含）。
    max_y: usize,
}

impl Bounds {
    /// 与矩形求交，交集为空时返回 `None`。
    ///
    /// 右/下边界用 `saturating_add`：`Rect::new` 不校验 `x + width` 是否溢出，
    /// 直接相加会在 `usize::MAX` 附近回绕成一个很小的上界。
    #[inline]
    fn intersect(self, rect: Rect) -> Option<Self> {
        let min_x = self.min_x.max(rect.x);
        let min_y = self.min_y.max(rect.y);
        let max_x = self.max_x.min(rect.x.saturating_add(rect.width));
        let max_y = self.max_y.min(rect.y.saturating_add(rect.height));
        if min_x < max_x && min_y < max_y {
            Some(Self {
                min_x,
                min_y,
                max_x,
                max_y,
            })
        } else {
            None
        }
    }

    /// 判断该坐标是否在可见区域中。
    pub fn contains(self, x: usize, y: usize) -> bool {
        x >= self.min_x && x < self.max_x && y >= self.min_y && y < self.max_y
    }

    /// 判断**尚未取整**的屏幕空间浮点坐标是否落在可见区域内。
    ///
    /// 参数是视口变换（[`normalized_to_screen`]）之后、**尚未** `as usize` 的浮点
    /// 像素坐标；名称中的 `normalized` 指「未经取整」，与 `[0, 1]` 归一化坐标空间
    /// 无关。整型版本 [`Bounds::contains`] 无法承担这一步：`f32 as usize` 是**饱和
    /// 转换**，负坐标会变成 `0`，恰好通过 `x >= min_x` 的检查，把屏幕外的点误判为
    /// 可见——viewport 原点为 `(0, 0)` 时，归一化坐标 `-0.5` 即会命中该陷阱。
    ///
    /// 边界约定与 [`Bounds::contains`] 一致：下界含、上界不含。
    ///
    /// 非有限坐标（`NaN` / `±∞`）一律判为不可见，无需调用方预先过滤。
    #[inline]
    pub fn contains_normalized(self, x: f32, y: f32) -> bool {
        x >= self.min_x as f32
            && x < self.max_x as f32
            && y >= self.min_y as f32
            && y < self.max_y as f32
    }
}

impl From<Rect> for Bounds {
    fn from(rect: Rect) -> Self {
        Self {
            min_x: rect.x,
            min_y: rect.y,
            max_x: rect.x.saturating_add(rect.width),
            max_y: rect.y.saturating_add(rect.height),
        }
    }
}

/// 管线状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineState {
    /// 片元与目标像素的混合方式。
    pub blend: BlendMode,
    /// 可选的裁剪矩形，`None` 表示不额外裁剪（仅受 viewport 与帧缓冲边界限制）。
    pub scissor: Option<ScissorRect>,
    /// normalized 坐标映射到的屏幕矩形。
    pub viewport: Viewport,
}

impl PipelineState {
    /// 以 Alpha 混合、无 scissor 构造管线状态。
    pub const fn alpha_blend(viewport: Viewport) -> Self {
        Self {
            blend: BlendMode::Alpha,
            scissor: None,
            viewport,
        }
    }
}

/// 显式传入的 2D 可编程管线。
///
/// 把顶点着色器、片元着色器与管线状态打包在一起，作为绘制调用的参数传入；
/// 库本身不维护任何全局状态，因此同一个管线可被多个 [`Renderer`] 复用。
#[derive(Debug, Clone)]
pub struct Pipeline<VS: VertexShader, FS: FragmentShader> {
    /// 顶点着色器实例。
    pub vertex_shader: VS,
    /// 片元着色器实例。
    pub fragment_shader: FS,
    /// 混合、裁剪与视口状态。
    pub state: PipelineState,
}

impl<VS: VertexShader, FS: FragmentShader> Pipeline<VS, FS> {
    /// 组装管线。
    pub const fn new(vertex_shader: VS, fragment_shader: FS, state: PipelineState) -> Self {
        Self {
            vertex_shader,
            fragment_shader,
            state,
        }
    }
}

/// 可编程管线对 [`Renderer`] 的扩展。
///
/// 固定功能的 2D 绘图 API（`draw_line` / `fill_triangle` 等）与这里的方法
/// 操作的是同一份帧缓冲与同一套格式缓存，可以混用。
impl<'a> Renderer<'a> {
    pub fn viewport(&self) -> Viewport {
        Viewport::new(0, 0, self.info().width, self.info().height)
    }

    /// 求出本次绘制调用的可见矩形：帧缓冲 ∩ viewport ∩ scissor。
    ///
    /// 返回 `None` 表示三者交集为空——本次调用的任何图元都不可能产生片元，
    /// 调用方应直接成功返回，连顶点着色都不必执行。非空时该矩形是各光栅化
    /// 函数共享的裁剪上界，故只需在进入图元装配前计算这一次。
    ///
    /// scissor 为 `None` 时不引入额外裁剪，仅受帧缓冲与 viewport 限制。
    #[inline]
    fn visible_bounds(&self, state: &PipelineState) -> Option<Bounds> {
        let info = self.info();
        let framebuffer = Bounds {
            min_x: 0,
            min_y: 0,
            max_x: info.width,
            max_y: info.height,
        };
        let bounds = framebuffer.intersect(state.viewport)?;
        match state.scissor {
            Some(scissor) => bounds.intersect(scissor),
            None => Some(bounds),
        }
    }

    /// 光栅化点图元。
    ///
    /// 点没有「部分可见」的概念：要么整点绘制，要么整点丢弃。因此这里不做线段与
    /// 三角形所用的参数域裁剪，而是直接用 `bound`（帧缓冲 ∩ viewport ∩ scissor）
    /// 对变换后的屏幕坐标做二值判定——这正是点图元唯一的裁剪手段。
    ///
    /// 判定必须在**浮点域**完成：先 `as usize` 会让负坐标饱和成 `0` 而误通过下界
    /// 检查，详见 [`Bounds::contains_normalized`]。
    ///
    /// 裁剪发生在 [`FragmentShader::shade_fragment`] **之前**：scissor 的语义是
    /// 「超出部分既不写着色器也不写帧缓冲」，故不能交由写入层兜底。
    fn rasterize_point<FS: FragmentShader, VS: VertexShader>(
        &mut self,
        input: FragmentInput<FS::Varying>,
        bound: &Bounds,
        pipeline: &Pipeline<VS, FS>,
    ) where
        VS: VertexShader<Varying = FS::Varying>,
        FS: FragmentShader,
    {
        let point = normalized_to_screen(input.position, pipeline.state.viewport);
        if !bound.contains_normalized(point[0], point[1]) {
            return;
        }

        if let FragmentOutput::Color(color) = pipeline.fragment_shader.shade_fragment(input) {}
    }

    pub fn draw_arrays<VS, FS>(
        &mut self,
        primitive: PrimitiveType,
        array: &[VS::Input],
        pipeline: &Pipeline<VS, FS>,
    ) -> Result<(), DrawError>
    where
        VS: VertexShader,
        FS: FragmentShader<Varying = VS::Varying>,
    {
        // 图元装配前先做视口校验：可见矩形为空说明整批图元都落在屏幕外，
        // 直接成功返回，连顶点着色都不执行。
        let bound = match self.visible_bounds(&pipeline.state) {
            Some(bound) => bound,
            None => return Ok(()),
        };

        match primitive {
            PrimitiveType::Points => {
                for vertex in array {
                    let input = pipeline.vertex_shader.shade_vertex(vertex);
                    self.rasterize_point(input, &bound, pipeline);
                }
            }
            PrimitiveType::Lines => {}
            PrimitiveType::Triangles => {}
        }
        Ok(())
    }
}
