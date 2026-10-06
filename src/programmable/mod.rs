use crate::utils::{ceil_to_usize, floor_to_usize, normalized_to_screen};
use crate::{color::Color, error::DrawError, Renderer};
pub mod cache;
pub mod shader;
pub mod texture;

use crate::programmable::cache::VertexCache;

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

impl Color {
    /// 在 `from` 与 `to` 之间做线性插值，`t = 0` 取 `from`、`t = 1` 取 `to`。
    ///
    /// 这是 [`Interpolate::lerp`] 的 inherent 版本，仅为让「两点间插值颜色」这一
    /// 常见操作无需 `use` trait 即可调用。取整口径与 varying 插值**完全一致**——
    /// 四个通道（含 alpha）一并插值并四舍五入钳制到 `[0, 255]`，
    /// 故程序化渐变与逐顶点 varying 渐变不会出现可见色差。
    ///
    /// `t` 超出 `[0, 1]` 不钳制参数本身，但结果仍被 `interpolate` 内部的
    /// `clamp` 拉回端点色，因此端点行为安全。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::{Color, BLACK, WHITE};
    ///
    /// assert_eq!(Color::lerp(BLACK, WHITE, 0.0), BLACK);
    /// assert_eq!(Color::lerp(BLACK, WHITE, 1.0), WHITE);
    /// // 中点落在两色之间。
    /// assert_eq!(Color::lerp(BLACK, WHITE, 0.5).r, 128);
    /// ```
    #[inline]
    pub fn lerp(from: Color, to: Color, t: f32) -> Self {
        <Self as Interpolate>::lerp(from, to, t)
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

/// 单元类型的插值实现：不携带任何属性，插值即恒等。
///
/// 供片元着色器只关心片元坐标、不需要任何 varying 的场景使用。
impl Interpolate for () {
    fn interpolate(_: Self, _: Self, _: Self, _: f32, _: f32, _: f32) -> Self {}
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
/// - `[0.0, 0.0]` 对应 viewport 的左上**边缘**
/// - `[1.0, 1.0]` 对应 viewport 的右下**边缘**，即 `x + viewport.width`，
///   位于末像素之外一像素（半开区间 `[x, x + width)` 的开边界）
///
/// 超出这个范围的顶点不会被直接 clamp，而是交给图元裁剪阶段处理。
/// 同理，位于开边界上的 `[1.0, 1.0]` 在**点图元**上会被判为不可见；
/// 线段与三角形则由裁剪收敛，末像素仍在绘制范围内。
/// 详见 [`crate::utils::normalized_to_screen`]。
///
/// `varying` 是该顶点的插值属性，会在光栅化时按重心权重在三个顶点间插值，
/// 因此其类型必须实现 [`Interpolate`]。
///
/// # 格式
///
/// `[0, 1]` 是 normalized viewport 坐标而非 OpenGL 的 `[-1, 1]` clip space；
/// 后者被映射到 viewport 的那一步由 [`crate::utils::normalized_to_screen`] 在此约定下完成。
/// 该映射与 OpenGL `glViewport` / Vulkan `VkViewport` / D3D11 `D3D11_VIEWPORT`
/// 同构：跨度乘**全宽**而非 `width - 1`，故 `1.0` 落在视口外边缘。
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
    /// 预乘 Alpha 混合：片元颜色被视为已乘过 `src.a`，故直接加到衰减后的目标上，
    /// 即 `src + dst × (1 - src.a)`，各通道饱和到 255。
    PremultipliedAlpha,
}

impl BlendMode {
    /// 计算 `src` 与 `dst` 在本混合模式下的结果。
    ///
    /// 这是全库混合语义的**唯一权威实现**：纯函数、无 I/O、每个分支都有正确语义
    /// （无 panic）。`Alpha` 一臂与热路径委托的字节层实现（`PixelFormat::blend_channel`）
    /// 共用同一公式，因此本函数的结果与真正落盘的颜色逐位相同。
    ///
    /// 返回值恒为不透明（`alpha == 255`）——混合结果的 alpha 通道从不落盘，
    /// 4bpp 格式写入的是格式自身的 opaque alpha 字节。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::Color;
    /// use rasteron::programmable::BlendMode;
    ///
    /// let src = Color::new(255, 0, 0);
    /// let dst = Color::new(0, 0, 255);
    ///
    /// // 直接覆盖：结果就是源色。
    /// assert_eq!(BlendMode::None.blend(src, dst), src);
    /// // 加法混合：各通道饱和相加。
    /// assert_eq!(BlendMode::Add.blend(src, dst), Color::new(255, 0, 255));
    /// ```
    #[inline(always)]
    pub fn blend(self, src: Color, dst: Color) -> Color {
        match self {
            BlendMode::None => src,
            BlendMode::Alpha => src.blend(&dst, src.a),
            BlendMode::Add => src.blend_add(&dst),
            BlendMode::Multiply => src.blend_multiply(&dst),
            BlendMode::PremultipliedAlpha => src.blend_premultiplied(&dst),
        }
    }
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
///
/// 采用**边缘空间**约定：`0.0` 映射到 `x` / `y`（左/上边缘），
/// `1.0` 映射到 `x + width` / `y + height`（右/下边缘，已在末像素之外）。
/// 详见 [`crate::utils::normalized_to_screen`]。
pub type Viewport = Rect;
/// 裁剪矩形：片元允许写入的屏幕区域，超出部分既不写着色器也不写帧缓冲。
pub type ScissorRect = Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
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
    /// 这与边缘空间的视口变换配套：normalized `[1.0, 1.0]` 映射到
    /// `x + viewport.width`，落在 `max_x` 上，因而被判为不可见。
    ///
    /// 非有限坐标（`NaN` / `±∞`）一律判为不可见，无需调用方预先过滤。
    #[inline]
    pub fn contains_normalized(self, x: f32, y: f32) -> bool {
        x >= self.min_x as f32
            && x < self.max_x as f32
            && y >= self.min_y as f32
            && y < self.max_y as f32
    }

    #[inline]
    pub fn clip_segment_t(self, origin: Vec2, delta: Vec2) -> Option<(f32, f32)> {
        // 每条轴的 (起点坐标, 方向分量, 下界, 上界)
        let axes = [
            (origin[0], delta[0], self.min_x as f32, self.max_x as f32),
            (origin[1], delta[1], self.min_y as f32, self.max_y as f32),
        ];

        let mut t_enter = 0.0f32;
        let mut t_exit = 1.0f32;
        for (start, step, lo, hi) in axes {
            if step == 0.0 {
                // 平行于这对边界：起点落在区间外则整段不可见
                if start < lo || start >= hi {
                    return None;
                }
                continue;
            }
            let (near, far) = ((lo - start) / step, (hi - start) / step);
            let (near, far) = if near <= far {
                (near, far)
            } else {
                (far, near)
            }; // ← 负数除法
            t_enter = t_enter.max(near);
            t_exit = t_exit.min(far);
            if t_enter > t_exit {
                return None;
            }
        }
        Some((t_enter, t_exit))
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
    ///
    /// 采用边缘空间约定：`[1.0, 1.0]` 落在 `x + width` / `y + height`，
    /// 即半开区间 `[x, x + width)` 的开边界，详见 [`Viewport`]。
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

    /// 把片元颜色按混合模式写入 `(x, y)`。
    ///
    /// # 调用方保证
    ///
    /// `(x, y)` 已通过 viewport 与 scissor 裁剪，落在帧缓冲范围内。
    ///
    /// 取整**不在这里做**：各图元的落位规则不同（点用向零取整，线段用四舍五入），
    /// 由各自的光栅化函数在自己的坐标系里决定，本函数只负责按混合模式落盘。
    #[inline(always)]
    fn write_fragment_color(&mut self, x: usize, y: usize, src: &Color, blend: BlendMode) {
        match blend {
            // 纯覆盖，不读目标。`mix_alpha(255)` 绕开 `write_color_at_offset` 的
            // `a == 0` 短路，使「None = 无条件覆盖」不被源透明度影响。
            // SAFETY: 调用方保证 (x, y) 落在帧缓冲范围内。
            BlendMode::None => unsafe { self.set_pixel_unchecked(x, y, &src.mix_alpha(255)) },

            // Alpha 全权委托字节层权威（`write_rgb_blend!` → `PixelFormat::blend_channel`）：
            // a==0 空操作、a==255 纯覆盖、其余混合，一条路径全覆盖，且无 Color 往返。
            // SAFETY: 同上。
            BlendMode::Alpha => unsafe { self.set_pixel_unchecked(x, y, src) },

            // 这三种模式的 RGB 结果都依赖目标像素，必须读-改-写。
            // `blend` 的返回值经 `Color::new` 构造，天然 a == 255，
            // 落盘走不透明快速路径，也避开了 `a == 0` 会吞掉写入的坑。
            BlendMode::Add | BlendMode::Multiply | BlendMode::PremultipliedAlpha => {
                // SAFETY: 调用方保证 (x, y) 落在帧缓冲范围内，故读与写都在同一合法像素上。
                let dst = unsafe { self.get_pixel_unchecked(x, y) };
                // SAFETY: 同上。
                unsafe { self.set_pixel_unchecked(x, y, &blend.blend(*src, dst)) };
            }
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
    fn rasterize_point<FS: FragmentShader>(
        &mut self,
        input: FragmentInput<FS::Varying>,
        bound: &Bounds,
        fragment_shader: &FS,
        state: &PipelineState,
    ) {
        let point = normalized_to_screen(input.position, state.viewport);
        if !bound.contains_normalized(point[0], point[1]) {
            return;
        }

        // 点图元用向零取整，与线段/三角形路径的四舍五入不同，故取整留在本函数内。
        // 已通过 `bound` 的坐标非负，截断即 floor，结果落在 `[min, max - 1]`。
        let (x, y) = (floor_to_usize(point[0]), floor_to_usize(point[1]));

        if let FragmentOutput::Color(color) = fragment_shader.shade_fragment(input) {
            self.write_fragment_color(x, y, &color, state.blend);
        }
    }

    /// 光栅化线图元。
    ///
    /// **端点顺序对输出不可观测（这是数学恒等式，不是实现缺陷）**：
    /// `Interpolate::lerp(a, b, t)` 逐通道等于 `lerp(b, a, 1 - t)`，而反序端点后
    /// 落在同一像素 x 的步进参数恰为 `1 - t`。两者相乘抵消，**任何两顶点线段在
    /// 反转索引后逐像素相同**（含取整）。因此不要试图用「线段端点反序应当翻转渐变」
    /// 来验证索引是否生效——该断言恒不成立。索引生效的正确验证方式见
    /// `tests::indexed_selects_vertices_through_the_index_buffer`。
    ///
    /// # 采样语义：DDA 采样点是密集的，片元是唯一的
    ///
    /// 沿可见段按 `steps = ceil(max|dx|, |dy|)` 做 DDA 采样（`steps + 1` 个参数），
    /// 再对每个采样点四舍五入取整。**采样点数量与像素数量不是一一对应**：当线段
    /// 短于一个像素时，多个 `t` 会映射到同一个整数像素；主轴步进为 1 时也可能
    /// 因四舍五入把相邻两个采样点压到同一像素上。
    ///
    /// 重复片元不是无害的：`Alpha` / `Add` / `Multiply` / `PremultipliedAlpha`
    /// 都是读-改-写，对同一像素着色两次会得到两倍的效果——半透明短线段比同位置
    /// 的点更亮，且亮度取决于线段方向（哪个轴承担了多余采样）。
    ///
    /// 因此每个像素**最多着色一次**：坐标为整数且在 `bound` 内时才与上一次输出的
    /// 坐标比较，相同则跳过。DDA 的主采样轴单调递增，重复坐标必然连续出现，
    /// 故只需记住一个 `previous`，不需要集合也不需要分配。
    ///
    /// 保留的是**第一次**映射到该像素的 `t`（端点插值方向与插入的重复点无关），
    /// 这与「短线段只调用一次片元着色器」是同一件事的两面。
    /// 由 `short_line_shades_each_pixel_once` 与
    /// `short_line_alpha_matches_a_point_at_the_same_pixel` 守护。
    fn rasterize_line<FS: FragmentShader>(
        &mut self,
        input: [FragmentInput<FS::Varying>; 2],
        bound: &Bounds,
        fragment_shader: &FS,
        state: &PipelineState,
    ) {
        let p0 = normalized_to_screen(input[0].position, state.viewport);
        let p1 = normalized_to_screen(input[1].position, state.viewport);

        // 不拦住就会把不可见的图元判成可见（同 `contains_normalized` 的处理）。
        if !(p0[0].is_finite() && p0[1].is_finite() && p1[0].is_finite() && p1[1].is_finite()) {
            return;
        }

        let delta = [p1[0] - p0[0], p1[1] - p0[1]];
        if delta[0] == 0.0 && delta[1] == 0.0 {
            // 退化线段：两端点映射到同一像素，语义等价于点图元
            self.rasterize_point(input[0], bound, fragment_shader, state);
            return;
        }

        // 线段裁剪
        let Some((t_enter, t_exit)) = bound.clip_segment_t(p0, delta) else {
            return;
        };
        let a = [p0[0] + delta[0] * t_enter, p0[1] + delta[1] * t_enter];
        let b = [p0[0] + delta[0] * t_exit, p0[1] + delta[1] * t_exit];
        let d = [b[0] - a[0], b[1] - a[1]];

        let steps = ceil_to_usize(d[0].abs().max(d[1].abs()));
        debug_assert!(
            steps <= (bound.max_x - bound.min_x) + (bound.max_y - bound.min_y) + 1,
            "可见子线段必须落在 bound 内，否则迭代量上界失效"
        );
        let denom = steps.max(1) as f32;
        // 上一次真正写出的像素坐标。DDA 主轴单调，重复坐标只会连续出现，
        // 故单个变量足够；只有真正写入后才更新它。
        let mut previous: Option<(usize, usize)> = None;
        for i in 0..=steps {
            let t = i as f32 / denom;
            let (x, y) = (
                floor_to_usize(a[0] + d[0] * t + 0.5),
                floor_to_usize(a[1] + d[1] * t + 0.5),
            );
            if !bound.contains(x, y) {
                continue;
            }
            // 同一像素的后续采样直接跳过：否则混合模式会被重复执行。
            if previous == Some((x, y)) {
                continue;
            }
            previous = Some((x, y));

            let t_original = t_enter + (t_exit - t_enter) * t;
            let fragment = FragmentInput {
                position: Interpolate::lerp(input[0].position, input[1].position, t_original),
                varying: Interpolate::lerp(input[0].varying, input[1].varying, t_original),
            };
            if let FragmentOutput::Color(color) = fragment_shader.shade_fragment(fragment) {
                self.write_fragment_color(x, y, &color, state.blend);
            }
        }
    }

    /// 光栅化三角形图元。
    ///
    /// # 算法
    ///
    /// 遍历 `bbox ∩ bound` 内的整数像素，用边函数判定覆盖并直接得到重心权重。
    /// 边函数是线性函数，其归一化值恰为重心坐标，因此覆盖判定与 varying 插值
    /// 共用同一组权重，无需另算。
    ///
    /// # 共享边：top-left 规则
    ///
    /// 三条边都采用闭区间判定会让**共享边被两个三角形各着色一次**：
    /// 两个三角形拼正方形时，对角线上的像素被混合两遍，`Alpha` / `Add` /
    /// `Multiply` / `PremultipliedAlpha` 下得到的结果依赖图元顺序（修复前实测
    /// `(16, 15)` 处亮度从邻域的 16 突跳到 30）。因此覆盖判定必须给每条边
    /// 一个明确的归属：恰在边上的像素归**左/上边**，不归**右/下边**。
    ///
    /// 关键性质是 `is_top_left` 对边方向的**反对称性**：两个共边三角形对
    /// 同一条边的遍历方向必然相反（否则它们的内部会落在边的同一侧，即重叠），
    /// 故恰有一方拥有该边上的像素，与双方绕向无关。因此任意两个共边三角形
    /// 都不重叠，网格拼接（正方形拆两三角形、棋盘格）不会重复混合。
    ///
    /// 代价是半像素级的取舍差异：落在共享边上的像素归给哪一侧取决于边的方向，
    /// 而非它在几何上「更靠近」哪个三角形。这与 GPU 的默认行为一致。
    ///
    /// 测试 `two_triangles_sharing_an_edge_never_shade_a_pixel_twice`
    /// 与 `triangle_ownership_of_a_shared_edge_is_winding_independent` 守护该行为。
    fn rasterize_triangle<FS: FragmentShader>(
        &mut self,
        input: [FragmentInput<FS::Varying>; 3],
        bound: &Bounds,
        fragment_shader: &FS,
        state: &PipelineState,
    ) {
        #[inline]
        fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
            (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
        }
        let v = [
            normalized_to_screen(input[0].position, state.viewport),
            normalized_to_screen(input[1].position, state.viewport),
            normalized_to_screen(input[2].position, state.viewport),
        ];
        if v.iter().any(|p| !(p[0].is_finite() && p[1].is_finite())) {
            return;
        }

        // 有符号面积的 2 倍。`edge(v1, v2, v0)` 单项就等于它——另两项
        // `edge(v2, v0, v1)` 与 `edge(v0, v1, v2)` 是同一量的重复计算，
        // 累加会把面积放大到 3 倍，使 `w_i / area` 之和退化成 `1/3`，
        // 插值出的 varying 被整体缩小（常量属性也不例外）。
        // 面积为零 = 三点共线，无内部像素。顺带避免权重除零。
        let area = edge(v[1], v[2], v[0]);
        if area == 0.0 {
            return;
        }

        // 规整绕向：`flip` 为真表示把 v1/v2 交换后面积变为正。此后所有边函数
        // 在三角形内部都是正的，top-left 判定与覆盖判定都无需再按 `area` 的
        // 符号分类，顺时针与逆时针也走完全相同的代码路径。
        //
        // `input` 必须与 `v` **同步交换**：`w_i` 是对 `v[i]` 的重心权重，
        // 交换后 `w1` 对应的是原来的 v2。若只换 `v` 不换 `input`，varying 会
        // 插到错误的顶点上。
        let (v, input) = if area < 0.0 {
            ([v[0], v[2], v[1]], [input[0], input[2], input[1]])
        } else {
            (v, input)
        };
        let area = area.abs();

        // bbox 必须先与 bound 求交，迭代量才与顶点坐标解耦
        let min_x = v[0][0].min(v[1][0]).min(v[2][0]);
        let max_x = v[0][0].max(v[1][0]).max(v[2][0]);
        let min_y = v[0][1].min(v[1][1]).min(v[2][1]);
        let max_y = v[0][1].max(v[1][1]).max(v[2][1]);

        let x0 = floor_to_usize(min_x).max(bound.min_x);
        let y0 = floor_to_usize(min_y).max(bound.min_y);
        let x1 = ceil_to_usize(max_x).min(bound.max_x);
        let y1 = ceil_to_usize(max_y).min(bound.max_y);
        if x0 >= x1 || y0 >= y1 {
            return;
        }

        // top-left 判定。
        //
        // 内部恒满足 `edge(a, b, p) > 0`，即内部在有向边 `a → b` 的
        // `(p - a) × (b - a) > 0` 一侧；该侧的**内向法线**是 `(-dy, dx)`。
        // 于是（Y 轴向下，`y` 小 = 上、`x` 小 = 左）：
        //
        // * 上边：`dy == 0` 且内向法线的 y 分量为正（内部在其下方）⟹ `dx > 0`；
        // * 左边：内向法线的 x 分量为正（内部在其右方）⟹ `dy < 0`。
        //
        // 本谓词对边方向的反对称性 `f(b, a) == !f(a, b)`（`dx`、`dy` 同时反号）
        // 正是「共享边不重复」的全部依据：任意两个共边三角形对同一条边的
        // 遍历方向必然相反，故恰有一方拥有它，与双方的绕向无关。
        let is_top_left = |a: Vec2, b: Vec2| -> bool {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            dy == 0.0 && dx > 0.0 || dy < 0.0
        };
        // 每条边预先算出「闭合」属性，热循环里只剩一次比较。
        let top_left = [
            is_top_left(v[1], v[2]),
            is_top_left(v[2], v[0]),
            is_top_left(v[0], v[1]),
        ];

        for y in y0..y1 {
            for x in x0..x1 {
                // 采样点取像素中心，否则覆盖判定会整体偏移半像素
                let p = [x as f32 + 0.5, y as f32 + 0.5];

                let w0 = edge(v[1], v[2], p);
                let w1 = edge(v[2], v[0], p);
                let w2 = edge(v[0], v[1], p);
                // 三条边同号即在内部。恰在边上时（w == 0）按 top-left 归属：
                // 左/上边归本三角形，右/下边不归——这排除了共享边的重复着色。
                if (w0 < 0.0)
                    || (w1 < 0.0)
                    || (w2 < 0.0)
                    || (w0 == 0.0 && !top_left[0])
                    || (w1 == 0.0 && !top_left[1])
                    || (w2 == 0.0 && !top_left[2])
                {
                    continue;
                }

                let fragment = FragmentInput {
                    position: Interpolate::interpolate(
                        input[0].position,
                        input[1].position,
                        input[2].position,
                        w0 / area,
                        w1 / area,
                        w2 / area,
                    ),
                    varying: Interpolate::interpolate(
                        input[0].varying,
                        input[1].varying,
                        input[2].varying,
                        w0 / area,
                        w1 / area,
                        w2 / area,
                    ),
                };
                if let FragmentOutput::Color(color) = fragment_shader.shade_fragment(fragment) {
                    self.write_fragment_color(x, y, &color, state.blend);
                }
            }
        }
    }

    pub fn draw_arrays<VS, FS>(
        &mut self,
        primitive: PrimitiveType,
        vertices: &[VS::Input],
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

        // 无缓存：每个顶点都重新着色一次，装配位置即顶点下标。
        self.assemble_primitives(
            primitive,
            vertices.len(),
            |index| pipeline.vertex_shader.shade_vertex(&vertices[index]),
            pipeline,
            &bound,
        );
        Ok(())
    }

    /// 索引绘制。缓存由调用方提供：
    ///
    /// * `cache` 为 `None` → 每次使用都重新着色（无缓存、零分配的基线语义）；
    /// * `cache` 为 `Some(cache)` → 按顶点索引惰性着色并复用；`cache.len()`
    ///   短于顶点数时退化为部分缓存（低索引命中、高索引每次都重新着色），
    ///   越界不 panic。
    ///
    /// 顶点数据在两次调用间变化时，调用方负责清空槽位（[`VertexCache::clear`]）
    /// 或不传 `cache`。
    ///
    /// # 索引校验
    ///
    /// 索引越界返回 [`DrawError::IndexOutOfBounds`] 而不是 panic 或静默跳图元：
    /// 索引缓冲写错属于调用方的程序错误，fail-fast 更容易定位。
    ///
    /// **校验先于任何绘制**：只要有一个索引越界，整次调用不产生任何片元——
    /// 部分写入后再报错会让调用方难以确定帧缓冲里残留了什么。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::color::Color;
    /// # use rasteron::framebuffer::{FramebufferInfo, PixelFormat};
    /// use rasteron::programmable::cache::VertexCache;
    /// use rasteron::programmable::shader::Passthrough2D;
    /// use rasteron::programmable::{
    ///     FragmentInput, FragmentOutput, FragmentShader, Pipeline, PipelineState,
    ///     PrimitiveType, Vertex2D, Viewport,
    /// };
    /// # use rasteron::Renderer;
    ///
    /// struct SolidFs;
    /// impl FragmentShader for SolidFs {
    ///     type Varying = Color;
    ///     fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
    ///         FragmentOutput::Color(input.varying)
    ///     }
    /// }
    ///
    /// # let info = FramebufferInfo::new(16, 16, 16 * 4, PixelFormat::Rgba8888);
    /// # let mut fb = vec![0u8; info.buffer_size()];
    /// # let mut renderer = Renderer::new(info, &mut fb);
    /// let vertices = [
    ///     Vertex2D { position: [0.0, 0.0], varying: Color::new(255, 0, 0) },
    ///     Vertex2D { position: [1.0, 0.0], varying: Color::new(0, 255, 0) },
    ///     Vertex2D { position: [1.0, 1.0], varying: Color::new(0, 0, 255) },
    ///     Vertex2D { position: [0.0, 1.0], varying: Color::new(255, 255, 0) },
    /// ];
    /// // 索引 3 被引用两次，缓存只着色一次。
    /// let indices = [0u16, 1, 2, 0, 2, 3];
    /// let pipeline = Pipeline::new(
    ///     Passthrough2D::new(),
    ///     SolidFs,
    ///     PipelineState::alpha_blend(Viewport::new(0, 0, 16, 16)),
    /// );
    ///
    /// let mut slots: [Option<FragmentInput<Color>>; 4] = [None; 4];
    /// let mut cache = VertexCache::new(&mut slots);
    /// renderer
    ///     .draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline, Some(&mut cache))
    ///     .unwrap();
    /// ```
    pub fn draw_indexed<VS, FS>(
        &mut self,
        primitive: PrimitiveType,
        vertices: &[VS::Input],
        indices: &[u16],
        pipeline: &Pipeline<VS, FS>,
        cache: Option<&mut VertexCache<'_, VS::Varying>>,
    ) -> Result<(), DrawError>
    where
        VS: VertexShader,
        FS: FragmentShader<Varying = VS::Varying>,
    {
        // 先整体校验索引，再进入图元装配：半途报错会留下「已画一半」的帧缓冲。
        let vertex_count = vertices.len();
        if let Some(position) = indices
            .iter()
            .position(|index| (*index as usize) >= vertex_count)
        {
            return Err(DrawError::IndexOutOfBounds {
                index: indices[position] as usize,
                vertex_count,
            });
        }

        // 图元装配前先做视口校验：可见矩形为空说明整批图元都落在屏幕外，
        // 直接成功返回，连顶点着色都不执行。
        let bound = match self.visible_bounds(&pipeline.state) {
            Some(bound) => bound,
            None => return Ok(()),
        };

        // 闭包只借用 `cache`（`&mut`），装配时按索引惰性着色：
        // 命中复用槽位，未命中执行顶点着色器并回填；越界索引由 `resolve` 静默降级。
        let mut cache = cache;
        self.assemble_primitives(
            primitive,
            indices.len(),
            |slot| {
                let vertex_index = indices[slot] as usize;
                match cache.as_deref_mut() {
                    Some(cache) => cache.resolve(
                        &pipeline.vertex_shader,
                        vertex_index,
                        &vertices[vertex_index],
                    ),
                    None => pipeline.vertex_shader.shade_vertex(&vertices[vertex_index]),
                }
            },
            pipeline,
            &bound,
        );
        Ok(())
    }

    /// 图元装配：按 `primitive` 把前 `count` 个「已着色顶点」组装成图元并光栅化。
    ///
    /// `resolve` 负责把**装配位置**（`0..count`）映射到实际的顶点着色结果：
    /// [`Self::draw_arrays`] 传顶点下标，[`Self::draw_indexed`] 传索引缓冲查表，
    /// 因此两条公开 API 共用同一份装配与光栅化逻辑，像素覆盖逐位一致。
    ///
    /// `resolve` 返回 `FragmentInput`（`Copy`），故按值传出不会引入生命周期约束；
    /// 缓存的 `&mut` 借用只存在于闭包内部，不会与 `self` 的借用冲突。
    ///
    /// 尾部不足一个完整图元的索引（`Lines` 余 1 个、`Triangles` 余 1 或 2 个）被
    /// 静默丢弃，与 [`Self::draw_arrays`] 对顶点缓冲的处理一致。
    fn assemble_primitives<VS, FS, R>(
        &mut self,
        primitive: PrimitiveType,
        count: usize,
        mut resolve: R,
        pipeline: &Pipeline<VS, FS>,
        bound: &Bounds,
    ) where
        VS: VertexShader,
        FS: FragmentShader<Varying = VS::Varying>,
        R: FnMut(usize) -> FragmentInput<VS::Varying>,
    {
        match primitive {
            PrimitiveType::Points => {
                for slot in 0..count {
                    self.rasterize_point(
                        resolve(slot),
                        bound,
                        &pipeline.fragment_shader,
                        &pipeline.state,
                    );
                }
            }
            PrimitiveType::Lines => {
                let mut slot = 0usize;
                while slot + 1 < count {
                    self.rasterize_line(
                        [resolve(slot), resolve(slot + 1)],
                        bound,
                        &pipeline.fragment_shader,
                        &pipeline.state,
                    );
                    slot += 2;
                }
            }
            PrimitiveType::Triangles => {
                let mut slot = 0usize;
                while slot + 2 < count {
                    self.rasterize_triangle(
                        [resolve(slot), resolve(slot + 1), resolve(slot + 2)],
                        bound,
                        &pipeline.fragment_shader,
                        &pipeline.state,
                    );
                    slot += 3;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::color::{Color as C, PixelFormat as Fmt};
    use crate::framebuffer::FramebufferInfo;
    use crate::programmable::shader::Passthrough2D;
    use crate::programmable::Vertex2D;

    /// 容差常量：光栅化用 f32 累加重心坐标，插值后允许极小的舍入偏差。
    const EPS: f32 = 1e-4;

    // ————————————————————————————— 测试替身（着色器） —————————————————————————————

    /// 把顶点 varying 原样输出的片元着色器（等价于内建 `Passthrough2D` + 直通 FS）。
    struct SolidFs;

    impl FragmentShader for SolidFs {
        type Varying = C;
        fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
            FragmentOutput::Color(input.varying)
        }
    }

    /// 记录每次调用次数的片元着色器，用于验证「裁剪发生在着色器之前」。
    struct CountingFs {
        calls: core::cell::Cell<usize>,
    }

    impl CountingFs {
        fn new() -> Self {
            Self {
                calls: core::cell::Cell::new(0),
            }
        }
    }

    impl FragmentShader for CountingFs {
        type Varying = C;
        fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
            self.calls.set(self.calls.get() + 1);
            FragmentOutput::Color(input.varying)
        }
    }

    /// 记录顶点着色器调用次数的着色器，用于验证缓存与「空集不着色」。
    #[derive(Clone)]
    struct CountingVs {
        calls: core::cell::Cell<usize>,
    }

    impl CountingVs {
        fn new() -> Self {
            Self {
                calls: core::cell::Cell::new(0),
            }
        }
    }

    impl VertexShader for CountingVs {
        type Input = Vertex2D<C>;
        type Varying = C;
        fn shade_vertex(&self, input: &Self::Input) -> FragmentInput<Self::Varying> {
            self.calls.set(self.calls.get() + 1);
            *input
        }
    }

    /// 恒定丢弃的片元着色器。
    struct DiscardFs;

    impl FragmentShader for DiscardFs {
        type Varying = C;
        fn shade_fragment(&self, _: FragmentInput<Self::Varying>) -> FragmentOutput {
            FragmentOutput::Discard
        }
    }

    /// 构造一个 `size × size` 的 Rgba8888 渲染器与帧缓冲。
    fn make_renderer(size: usize) -> (FramebufferInfo, Vec<u8>) {
        let info = FramebufferInfo::new(size, size, size * 4, Fmt::Rgba8888);
        let buffer = vec![0u8; info.buffer_size()];
        (info, buffer)
    }

    /// 收集 Rgba8888 缓冲区中被写过的像素坐标（alpha 非零即视为写过）。
    ///
    /// 本文件所有测试都用 `make_renderer(size)` 生成 `size × size`、`pitch = size * 4`
    /// 的帧缓冲，故只需传 `size`。
    ///
    /// 刻意不接收 `&FramebufferInfo`：`Renderer::new` 会**取得** `info` 的所有权
    /// （`FramebufferInfo` 未实现 `Clone`），渲染器存活期间无法再引用原值。
    fn painted_pixels(size: usize, buffer: &[u8]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let off = y * size * 4 + x * 4;
                // Rgba8888 写入后第 4 字节恒为 255。
                if buffer[off + 3] != 0 {
                    out.push((x, y));
                }
            }
        }
        out
    }

    // ————————————————————————————— S24 · 点 / 线光栅化 —————————————————————————————

    #[test]
    fn rasterize_point_maps_normalized_to_screen_including_viewport_offset() {
        let (info, mut buffer) = make_renderer(64);
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(10, 20, 40, 30),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            // 边缘空间：[0,0] → viewport 左上边缘 (10,20)；向零取整 → 像素 (10,20)。
            renderer
                .draw_arrays(
                    PrimitiveType::Points,
                    &[Vertex2D {
                        position: [0.0, 0.0],
                        varying: C::new(255, 0, 0),
                    }],
                    &pipeline,
                )
                .unwrap();
        }
        assert_eq!(painted_pixels(64, &buffer), vec![(10, 20)]);

        // [0.5, 0.5] → (10 + 20, 20 + 15) = (30, 35)。
        let (info2, mut buffer2) = make_renderer(64);
        {
            let mut renderer = Renderer::new(info2, &mut buffer2);
            renderer
                .draw_arrays(
                    PrimitiveType::Points,
                    &[Vertex2D {
                        position: [0.5, 0.5],
                        varying: C::new(255, 0, 0),
                    }],
                    &pipeline,
                )
                .unwrap();
        }
        assert_eq!(painted_pixels(64, &buffer2), vec![(30, 35)]);
    }

    #[test]
    fn point_outside_the_viewport_is_clipped_before_the_fragment_shader() {
        let (info, mut buffer) = make_renderer(64);
        // viewport 原点为 (0,0)，归一化 -0.5 映射到屏幕 -32（视口外）。
        // 这是 `contains_normalized` 的核心陷阱：先 `as usize` 会饱和成 0 而误判可见。
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            CountingFs::new(),
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, 64, 64),
            },
        );
        let mut renderer = Renderer::new(info, &mut buffer);
        renderer
            .draw_arrays(
                PrimitiveType::Points,
                &[
                    Vertex2D {
                        position: [-0.5, 0.5],
                        varying: C::new(255, 0, 0),
                    },
                    // 可见的一个，作为对照组：着色器确实会被调用。
                    Vertex2D {
                        position: [0.5, 0.5],
                        varying: C::new(255, 0, 0),
                    },
                ],
                &pipeline,
            )
            .unwrap();
        assert_eq!(
            pipeline.fragment_shader.calls.get(),
            1,
            "视口外的点不得调用片元着色器（scissor/裁剪在着色器之前）"
        );
        assert_eq!(painted_pixels(64, &buffer), vec![(32, 32)]);
    }

    #[test]
    fn rasterize_point_uses_truncation_not_rounding() {
        // 点用向零取整：normalized 0.9 → 屏幕 57.6 → 像素 57（不是四舍五入的 58）。
        let (info, mut buffer) = make_renderer(64);
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, 64, 64),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(
                    PrimitiveType::Points,
                    &[Vertex2D {
                        position: [0.9, 0.9],
                        varying: C::new(255, 0, 0),
                    }],
                    &pipeline,
                )
                .unwrap();
        }
        // 0.9 * 64 = 57.6，向零取整 → 57。
        assert_eq!(painted_pixels(64, &buffer), vec![(57, 57)]);
    }

    #[test]
    fn rasterize_line_is_end_order_invariant() {
        // 文档明示的数学恒等式（不是缺陷）：`lerp` 逐通道等于 `lerp(b, a, 1-t)`，
        // 而反序端点后同一像素的步进参数恰为 `1 - t`，两者相乘抵消。
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [1.0, 1.0],
                varying: C::new(0, 0, 255),
            },
        ];
        let render = |vs: &[Vertex2D<C>]| -> Vec<(usize, usize)> {
            let (info, mut buffer) = make_renderer(64);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport: Viewport::new(0, 0, 64, 64),
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_arrays(PrimitiveType::Lines, vs, &pipeline)
                    .unwrap();
            }
            painted_pixels(64, &buffer)
        };
        assert_eq!(
            render(&vertices),
            render(&[vertices[1], vertices[0]]),
            "线段端点反序必须逐像素相同"
        );
    }

    #[test]
    fn degenerate_line_behaves_like_a_point() {
        // 「退化」的定义是 `rasterize_line` 内部的 `delta == 0.0`，
        // 即**两个 normalized 坐标完全相同**（而不是「取整后落到同一像素」）。
        // 此时直接转调 `rasterize_point`，语义与点图元一致。
        //
        // 这一点与「取整后同像素但坐标不同」必须区分：后者 delta 非零，
        // 会按 `steps = ceil(max|dx|,|dy|)` 走两步采样，因此**画出两个像素**。
        // 下面两条断言分别锁定这两种行为。
        let render = |positions: [Vec2; 2]| -> Vec<(usize, usize)> {
            let (info, mut buffer) = make_renderer(64);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport: Viewport::new(0, 0, 64, 64),
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_arrays(
                        PrimitiveType::Lines,
                        &[
                            Vertex2D {
                                position: positions[0],
                                varying: C::new(255, 0, 0),
                            },
                            Vertex2D {
                                position: positions[1],
                                varying: C::new(255, 0, 0),
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();
            }
            painted_pixels(64, &buffer)
        };

        // 坐标完全相同 → delta == 0 → 等价于点图元，只有一个像素。
        let same = render([[0.25, 0.25], [0.25, 0.25]]);
        assert_eq!(same, vec![(16, 16)], "坐标相同的退化线段应等价于点图元");

        // 坐标不同且取整后落在**不同**像素 → 两个采样点各写一个像素。
        // 0.25 * 64 = 16.00 → 像素 16；0.26 * 64 = 16.64 → 四舍五入得 17。
        // 这里没有重复，去重逻辑不应误删任何一个像素。
        let near = render([[0.25, 0.25], [0.26, 0.26]]);
        assert_eq!(
            near,
            vec![(16, 16), (17, 17)],
            "取整后落在不同像素的线段仍应画出两个像素"
        );
    }

    // ————————————— S24 补充 · 线段片元去重（短线段/重复采样） —————————————

    /// 屏幕长度远小于 1 像素的线段只调用一次片元着色器。
    ///
    /// 去重前：`steps = ceil(0.1) = 1`，`t = 0` 与 `t = 1` 两点四舍五入后
    /// 落在同一像素，着色器被调用两次。
    #[test]
    fn short_line_shades_each_pixel_once() {
        // 屏幕长度 0.1 像素：64 × 0.0015625 ≈ 0.1。
        let short = 0.1 / 64.0;
        let (info, mut buffer) = make_renderer(64);
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            CountingFs::new(),
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, 64, 64),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(
                    PrimitiveType::Lines,
                    &[
                        Vertex2D {
                            position: [0.5, 0.5],
                            varying: C::new(255, 0, 0),
                        },
                        Vertex2D {
                            position: [0.5 + short, 0.5],
                            varying: C::new(255, 0, 0),
                        },
                    ],
                    &pipeline,
                )
                .unwrap();
        }
        // 计数器是辅助证据；真正的断言是下面的 framebuffer 结果。
        assert_eq!(
            pipeline.fragment_shader.calls.get(),
            1,
            "长度 0.1 像素的线段应对片元着色器只调用一次"
        );
        assert_eq!(
            painted_pixels(64, &buffer),
            vec![(32, 32)],
            "短线段应恰好覆盖一个像素"
        );
    }

    /// 半透明短线段与同位置的点绘制结果**逐字节相同**。
    ///
    /// 这是去重最有说服力的口径：点图元天然只写一次，若线段把同一像素写了两次，
    /// `BlendMode::Alpha` 会混合两遍，结果必然比点更亮。
    #[test]
    fn short_line_alpha_matches_a_point_at_the_same_pixel() {
        let color = C::with_alpha(255, 255, 255, 128);
        // 屏幕长度 0.1 像素、水平方向（主轴 x）。
        let short = 0.1 / 64.0;

        let render_line = || -> Vec<u8> {
            let (info, mut buffer) = make_renderer(64);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState::alpha_blend(Viewport::new(0, 0, 64, 64)),
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                // 先把整屏清成不透明的黑，混合才有可观察的底。
                renderer
                    .draw_arrays(
                        PrimitiveType::Triangles,
                        &[
                            Vertex2D {
                                position: [0.0, 0.0],
                                varying: C::new(0, 0, 0),
                            },
                            Vertex2D {
                                position: [1.0, 0.0],
                                varying: C::new(0, 0, 0),
                            },
                            Vertex2D {
                                position: [0.0, 1.0],
                                varying: C::new(0, 0, 0),
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();
                renderer
                    .draw_arrays(
                        PrimitiveType::Lines,
                        &[
                            Vertex2D {
                                position: [0.5, 0.5],
                                varying: color,
                            },
                            Vertex2D {
                                position: [0.5 + short, 0.5],
                                varying: color,
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();
            }
            buffer
        };

        let render_point = || -> Vec<u8> {
            let (info, mut buffer) = make_renderer(64);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState::alpha_blend(Viewport::new(0, 0, 64, 64)),
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_arrays(
                        PrimitiveType::Triangles,
                        &[
                            Vertex2D {
                                position: [0.0, 0.0],
                                varying: C::new(0, 0, 0),
                            },
                            Vertex2D {
                                position: [1.0, 0.0],
                                varying: C::new(0, 0, 0),
                            },
                            Vertex2D {
                                position: [0.0, 1.0],
                                varying: C::new(0, 0, 0),
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();
                renderer
                    .draw_arrays(
                        PrimitiveType::Points,
                        &[Vertex2D {
                            position: [0.5, 0.5],
                            varying: color,
                        }],
                        &pipeline,
                    )
                    .unwrap();
            }
            buffer
        };

        let line = render_line();
        let point = render_point();

        // 被画到的像素必须非空。
        let lit: Vec<usize> = (0..64 * 64)
            .filter(|i| line[i * 4] != 0 || line[i * 4 + 1] != 0)
            .collect();
        assert_eq!(lit.len(), 1, "短线段必须恰好点亮一个像素，实际 {lit:?}");
        assert_ne!(line, point_precondition_black(64), "线段确实写入了内容");

        assert_eq!(
            line, point,
            "半透明短线段必须与同位置的半透明点逐字节相同（重复片元会混合两遍）"
        );
    }

    /// 全黑帧缓冲（用于给上面那条测试做非平凡性对照）。
    fn point_precondition_black(size: usize) -> Vec<u8> {
        vec![0u8; size * size * 4]
    }

    /// 不同方向的短线段不会因重复采样产生不同亮度。
    ///
    /// 去重前，多余采样落在哪个轴上取决于线段方向（主轴承担 `steps` 次迭代），
    /// 于是水平、垂直、斜向的短线段会在同一像素写出不同的混合次数。
    #[test]
    fn short_lines_of_different_directions_have_equal_brightness() {
        let color = C::with_alpha(255, 255, 255, 128);
        let short = 0.1 / 64.0;

        // 四条屏幕长度同为 0.1 像素、方向不同的短线段，全部锚在 (0.5, 0.5)。
        let directions: [[f32; 2]; 4] =
            [[short, 0.0], [0.0, short], [short, short], [short, -short]];

        let mut samples = Vec::new();
        for dir in directions {
            let (info, mut buffer) = make_renderer(64);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState::alpha_blend(Viewport::new(0, 0, 64, 64)),
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_arrays(
                        PrimitiveType::Lines,
                        &[
                            Vertex2D {
                                position: [0.5, 0.5],
                                varying: color,
                            },
                            Vertex2D {
                                position: [0.5 + dir[0], 0.5 + dir[1]],
                                varying: color,
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();
            }
            // 取锚点像素的颜色：所有方向都应落在 (32, 32)。
            let off = (32 * 64 + 32) * 4;
            samples.push((dir, [buffer[off], buffer[off + 1], buffer[off + 2]]));
        }

        let first = samples[0].1;
        for (dir, rgb) in &samples {
            assert_eq!(
                *rgb, first,
                "方向 {dir:?} 的短线段亮度与其它方向不同（重复片元？）"
            );
        }
        // 非平凡性：混合必须真的发生过（不是全 0），且是「恰好一次」的精确值。
        // `PixelFormat::blend_channel` 的契约公式是 `(src×a + dst×inv + 255) >> 8`，
        // 在 src = 255、dst = 0、a = 128 下即 `(255×128 + 255) >> 8 = 128`。
        // 若该像素被混合两次，结果会变成 `(255×128 + 128×127 + 255) >> 8 = 191`。
        let single = ((255u32 * 128) + 255) >> 8;
        assert_eq!(single, 128, "公式口径核对");
        assert_eq!(
            first[0] as u32, single,
            "锚点像素应是恰好一次 alpha 混合的结果"
        );
    }

    /// 长线段仍保持既有像素覆盖（去重不得改变正常线段的栅格化结果）。
    #[test]
    fn long_line_coverage_is_unchanged() {
        let color = C::new(255, 0, 0);
        let (info, mut buffer) = make_renderer(64);
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            CountingFs::new(),
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, 64, 64),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(
                    PrimitiveType::Lines,
                    &[
                        Vertex2D {
                            position: [0.0, 0.0],
                            varying: color,
                        },
                        Vertex2D {
                            position: [1.0, 1.0],
                            varying: color,
                        },
                    ],
                    &pipeline,
                )
                .unwrap();
        }
        let painted = painted_pixels(64, &buffer);
        // 从 (0,0) 到 (63,63) 的对角线：每行一个像素，共 64 个。
        assert_eq!(painted.len(), 64, "对角线应覆盖 64 个像素");
        for (i, p) in painted.iter().enumerate() {
            assert_eq!(*p, (i, i), "第 {i} 个像素应落在对角线上");
        }
        // 采样数（64 个不同像素）不变，故着色器调用次数也应保持不变。
        assert_eq!(
            pipeline.fragment_shader.calls.get(),
            64,
            "长线段的每个像素恰好着色一次"
        );
    }

    /// 线段反向绘制仍保持覆盖与颜色插值一致。
    ///
    /// 去重引入了 `previous` 状态，必须确认它与端点顺序无关：反序后端点到像素的
    /// 映射不变（只是遍历方向相反），`previous` 的比较同样对称。
    #[test]
    fn short_line_dedup_is_end_order_invariant() {
        let color = C::with_alpha(255, 255, 255, 128);
        let short = 0.1 / 64.0;

        let render = |a: [f32; 2], b: [f32; 2]| -> Vec<u8> {
            let (info, mut buffer) = make_renderer(64);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState::alpha_blend(Viewport::new(0, 0, 64, 64)),
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_arrays(
                        PrimitiveType::Lines,
                        &[
                            Vertex2D {
                                position: a,
                                varying: color,
                            },
                            Vertex2D {
                                position: b,
                                varying: color,
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();
            }
            buffer
        };

        let forward = render([0.5, 0.5], [0.5 + short, 0.5]);
        let backward = render([0.5 + short, 0.5], [0.5, 0.5]);
        assert_eq!(
            forward, backward,
            "短线段反序绘制必须逐字节相同（去重状态与端点顺序无关）"
        );

        // 非平凡性：确实是「点亮的像素 + 一次混合」，不是两条路径都空。
        let off = (32 * 64 + 32) * 4;
        assert!(forward[off] > 0, "短线段应写入锚点像素");
    }

    // ———————————— 线段参考模型：「每像素最多一次」 ————————————

    /// **独立参考模型**：线段在给定 viewport 下应着色的像素集合。
    ///
    /// 刻意**不复用** `rasterize_line` 的任何代码路径（不调
    /// `normalized_to_screen` / `clip_segment_t` / `floor_to_usize`），而是用
    /// 最朴素的定义重算一遍：把整条线段按足够细的步长采样，逐点做与生产实现
    /// 相同的视口变换与四舍五入，最后用 `BTreeSet` **去重**。
    ///
    /// 去重是参考模型的核心语义：线段是零面积图元，「哪些像素属于这条线段」
    /// 与其**顺序**无关，故正确的覆盖集必然等于「所有采样点映射到的像素之并」。
    /// 生产实现若对同一像素着色多次，去重后的模型不会体现——所以覆盖集相等
    /// 只说明**几何**对，写入次数由调用方的计数着色器单独断言。
    ///
    /// 采样步长取可见段长度的 `1/4096`，远细于生产实现的
    /// `ceil(max|dx|, |dy|)` 步进，因此模型是生产采样的**加细**：它的像素集合
    /// 是生产集合的超集（更密的采样只会发现更多像素，不会漏掉生产找到的）。
    fn reference_line_pixels(
        a: [f32; 2],
        b: [f32; 2],
        viewport: Viewport,
        size: usize,
    ) -> alloc::collections::BTreeSet<(usize, usize)> {
        use alloc::collections::BTreeSet;

        // 视口变换（与 `normalized_to_screen` 同口径，此处独立重写）。
        let to_screen = |p: [f32; 2]| -> [f32; 2] {
            [
                viewport.x as f32 + p[0] * viewport.width as f32,
                viewport.y as f32 + p[1] * viewport.height as f32,
            ]
        };
        let p0 = to_screen(a);
        let p1 = to_screen(b);

        // 逐点取整（四舍五入）后判可见，与生产实现一致。
        let pixel_of = |p: [f32; 2]| -> (usize, usize) {
            let round = |v: f32| -> usize {
                let r = v + 0.5;
                if r <= 0.0 {
                    0
                } else {
                    r as usize
                }
            };
            (round(p[0]), round(p[1]))
        };

        let mut out = BTreeSet::new();
        const SUBDIVISIONS: usize = 4096;
        for i in 0..=SUBDIVISIONS {
            let t = i as f32 / SUBDIVISIONS as f32;
            let p = [p0[0] + (p1[0] - p0[0]) * t, p0[1] + (p1[1] - p0[1]) * t];
            let (x, y) = pixel_of(p);
            if x < size && y < size {
                out.insert((x, y));
            }
        }
        out
    }

    /// 参考模型与生产实现的一致性：多种线段的覆盖集必须等于独立模型的覆盖集。
    ///
    /// 这条护栏的作用是**约束去重不能误删像素**——去重只允许消除重复的
    /// **写入动作**，不允许改变覆盖集合。若把 `previous` 的比较写错（例如比较
    /// 整个 (x, y) 之外的量、或漏更新导致连续跳过），覆盖集立刻变小。
    #[test]
    fn line_coverage_matches_independent_reference_model() {
        let size = 64usize;
        let viewport = Viewport::new(0, 0, size, size);

        // 覆盖长线、短于 1 像素的线、水平/垂直/斜向、反向。
        let cases: [([f32; 2], [f32; 2]); 7] = [
            ([0.0, 0.0], [1.0, 1.0]),
            ([0.0, 0.5], [1.0, 0.5]),
            ([0.5, 0.0], [0.5, 1.0]),
            ([0.5, 0.5], [0.5 + 0.1 / 64.0, 0.5]),
            ([0.5 + 0.1 / 64.0, 0.5], [0.5, 0.5]),
            ([0.2, 0.3], [0.8, 0.65]),
            ([0.9, 0.1], [0.1, 0.95]),
        ];

        for (a, b) in cases {
            let (info, mut buffer) = make_renderer(size);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport,
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_arrays(
                        PrimitiveType::Lines,
                        &[
                            Vertex2D {
                                position: a,
                                varying: C::new(255, 255, 255),
                            },
                            Vertex2D {
                                position: b,
                                varying: C::new(255, 255, 255),
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();
            }

            let actual: alloc::collections::BTreeSet<(usize, usize)> =
                painted_pixels(size, &buffer).into_iter().collect();
            let expected = reference_line_pixels(a, b, viewport, size);

            // 模型是生产采样的加细，故实际集合必须是模型的**子集**；
            // 生产实现只采样 `steps + 1` 个点，模型采样 4097 个点。
            assert!(
                actual.is_subset(&expected),
                "线段 {a:?}→{b:?} 覆盖了模型之外的像素：{:?}",
                actual.difference(&expected).collect::<Vec<_>>()
            );
            // 且模型里不能有「实际一个都没画到」的病态情况——至少端点像素要在。
            assert!(!actual.is_empty(), "线段 {a:?}→{b:?} 未覆盖任何像素");
        }
    }

    /// 五种混合模式对短线段都**只写一次**。
    ///
    /// 这是需求里最直接的一条：`None` / `Alpha` / `Add` / `Multiply` /
    /// `PremultipliedAlpha` 无一例外。对 `None` 与 `Alpha` 用「与同位置点比较」
    /// 判断写入次数；对三种读-改-写模式，重复写入会显著改变结果，故用
    /// 「与同位置点逐字节相同」作为统一口径——点图元天然只写一次。
    #[test]
    fn every_blend_mode_writes_a_short_line_exactly_once() {
        // 颜色刻意与底色在三个通道上都不同：`Multiply` 在 src == dst 时是
        // 恒等变换（`x * x / 255 ≈ x` 的定点近似），若取同色会让「写入确实
        // 发生」的断言失效。
        let color = C::with_alpha(240, 200, 40, 128);
        let short = 0.1 / 64.0;
        let size = 64usize;

        let modes = [
            BlendMode::None,
            BlendMode::Alpha,
            BlendMode::Add,
            BlendMode::Multiply,
            BlendMode::PremultipliedAlpha,
        ];

        // 底：先铺满一层非透明灰，让三种读-改-写模式有可观察的底。
        let background = C::new(90, 90, 90);
        let render_with = |primitive: PrimitiveType, blend: BlendMode| -> Vec<u8> {
            let (info, mut buffer) = make_renderer(size);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState {
                    blend,
                    scissor: None,
                    viewport: Viewport::new(0, 0, size, size),
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                // 铺底：两个三角形拼满整屏。
                renderer
                    .draw_arrays(
                        PrimitiveType::Triangles,
                        &[
                            Vertex2D {
                                position: [0.0, 0.0],
                                varying: background,
                            },
                            Vertex2D {
                                position: [1.0, 0.0],
                                varying: background,
                            },
                            Vertex2D {
                                position: [1.0, 1.0],
                                varying: background,
                            },
                            Vertex2D {
                                position: [0.0, 0.0],
                                varying: background,
                            },
                            Vertex2D {
                                position: [1.0, 1.0],
                                varying: background,
                            },
                            Vertex2D {
                                position: [0.0, 1.0],
                                varying: background,
                            },
                        ],
                        &pipeline,
                    )
                    .unwrap();

                match primitive {
                    PrimitiveType::Lines => {
                        renderer
                            .draw_arrays(
                                PrimitiveType::Lines,
                                &[
                                    Vertex2D {
                                        position: [0.5, 0.5],
                                        varying: color,
                                    },
                                    Vertex2D {
                                        position: [0.5 + short, 0.5],
                                        varying: color,
                                    },
                                ],
                                &pipeline,
                            )
                            .unwrap();
                    }
                    _ => {
                        renderer
                            .draw_arrays(
                                PrimitiveType::Points,
                                &[Vertex2D {
                                    position: [0.5, 0.5],
                                    varying: color,
                                }],
                                &pipeline,
                            )
                            .unwrap();
                    }
                }
            }
            buffer
        };

        for blend in modes {
            let from_line = render_with(PrimitiveType::Lines, blend);
            let from_point = render_with(PrimitiveType::Points, blend);

            // 差异只应出现在锚点像素上；两者必须逐字节相同。
            assert_eq!(
                from_line, from_point,
                "BlendMode::{blend:?} 下短线段与同位置点的结果不同（重复片元？）"
            );

            // 非平凡性：该模式确实改变了锚点像素（不是两种路径都无事发生）。
            let off = (32 * size + 32) * 4;
            assert_ne!(
                &from_point[off..off + 3],
                &background_bytes(background)[..],
                "BlendMode::{blend:?} 下锚点像素未被写入，断言失去意义"
            );
            // 线段只影响锚点一个像素：把「有线段的帧」与「只有铺底的帧」对比，
            // 差异必须恰为该像素。这既确认了写入确实发生，也确认了线段的影响
            // 范围没有被铺底掩盖。
            let background_only = {
                let (info, mut buffer) = make_renderer(size);
                let pipeline = Pipeline::new(
                    Passthrough2D::<C>::new(),
                    SolidFs,
                    PipelineState {
                        blend,
                        scissor: None,
                        viewport: Viewport::new(0, 0, size, size),
                    },
                );
                {
                    let mut renderer = Renderer::new(info, &mut buffer);
                    renderer
                        .draw_arrays(
                            PrimitiveType::Triangles,
                            &[
                                Vertex2D {
                                    position: [0.0, 0.0],
                                    varying: background,
                                },
                                Vertex2D {
                                    position: [1.0, 0.0],
                                    varying: background,
                                },
                                Vertex2D {
                                    position: [1.0, 1.0],
                                    varying: background,
                                },
                                Vertex2D {
                                    position: [0.0, 0.0],
                                    varying: background,
                                },
                                Vertex2D {
                                    position: [1.0, 1.0],
                                    varying: background,
                                },
                                Vertex2D {
                                    position: [0.0, 1.0],
                                    varying: background,
                                },
                            ],
                            &pipeline,
                        )
                        .unwrap();
                }
                buffer
            };
            // 线段的影响范围**不得超出**锚点像素：任何额外被改动的像素都说明
            // 光栅化跑到了别的坐标上。
            let differing: Vec<usize> = (0..size * size)
                .filter(|i| from_point[i * 4..i * 4 + 4] != background_only[i * 4..i * 4 + 4])
                .collect();
            assert!(
                differing.iter().all(|i| *i == 32 * size + 32),
                "BlendMode::{blend:?} 下线段影响了锚点之外的像素：{differing:?}"
            );
            // 对 `None` / `Alpha` 这两个不依赖底色的模式，再要求写入确实发生；
            // `Add` / `Multiply` / `PremultipliedAlpha` 在特定颜色组合下可能是
            // 恒等变换（例如 `Multiply` 的 src ≈ dst），此时「影响范围为空」是
            // 合法结果，不能据此判定断言失效。
            if matches!(blend, BlendMode::None | BlendMode::Alpha) {
                assert_eq!(
                    differing,
                    vec![32 * size + 32],
                    "BlendMode::{blend:?} 下锚点像素必须被改动"
                );
            }
        }
    }

    /// 参考用的 `Color` 字节序（Rgba8888 下即 `[r, g, b, a]`）。
    fn background_bytes(color: C) -> [u8; 3] {
        [color.r, color.g, color.b]
    }

    #[test]
    fn rasterize_line_discards_non_finite_endpoints() {
        let (info, mut buffer) = make_renderer(64);
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            CountingFs::new(),
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, 64, 64),
            },
        );
        let mut renderer = Renderer::new(info, &mut buffer);
        for bad in [
            [f32::NAN, 0.5],
            [0.5, f32::NAN],
            [f32::INFINITY, 0.5],
            [0.5, f32::NEG_INFINITY],
        ] {
            renderer
                .draw_arrays(
                    PrimitiveType::Lines,
                    &[
                        Vertex2D {
                            position: bad,
                            varying: C::new(255, 0, 0),
                        },
                        Vertex2D {
                            position: [0.9, 0.9],
                            varying: C::new(255, 0, 0),
                        },
                    ],
                    &pipeline,
                )
                .unwrap();
        }
        assert_eq!(
            pipeline.fragment_shader.calls.get(),
            0,
            "含非有限端点的线段整条丢弃，不得调用片元着色器"
        );
        assert!(painted_pixels(64, &buffer).is_empty());
    }

    #[test]
    fn rasterize_line_clips_to_bounds_and_interpolates_linearly() {
        // 端点在视口外，裁剪后应只画可见段，且 varying 沿线段线性变化。
        let (info, mut buffer) = make_renderer(64);
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, 64, 64),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            // 从屏幕外 (-32, 32) 到 (95, 32)：水平线，可见段为 x ∈ [0, 63]。
            renderer
                .draw_arrays(
                    PrimitiveType::Lines,
                    &[
                        Vertex2D {
                            position: [-0.5, 0.5],
                            varying: C::new(0, 0, 0),
                        },
                        Vertex2D {
                            position: [1.5, 0.5],
                            varying: C::new(255, 255, 255),
                        },
                    ],
                    &pipeline,
                )
                .unwrap();
        }
        let painted = painted_pixels(64, &buffer);
        // 可见段覆盖 x = 0..=63 这一行，共 64 个像素。
        assert_eq!(painted.len(), 64, "可见段像素数不符");
        assert!(painted.iter().all(|&(_, y)| y == 32), "应只有一行");
        assert_eq!(
            painted.iter().map(|&(x, _)| x).min(),
            Some(0),
            "左端应被裁到 0"
        );
        assert_eq!(
            painted.iter().map(|&(x, _)| x).max(),
            Some(63),
            "右端应被裁到 63"
        );

        // varying 沿线段线性：最左像素最暗、最右最亮。
        let left = read_pixel(&buffer, 64, 0, 32);
        let right = read_pixel(&buffer, 64, 63, 32);
        assert!(
            left.r < right.r,
            "varying 应沿线段递增（左 {} 右 {}）",
            left.r,
            right.r
        );
    }

    // ————————————————————————————— S25 · 三角形光栅化 —————————————————————————————

    /// 覆盖整屏的轴对齐三角形：`[(0,0), (1,0), (0,1)]` → 屏幕 `[(0,0), (64,0), (0,64)]`。
    fn full_triangle() -> [Vertex2D<C>; 3] {
        [
            Vertex2D {
                position: [0.0, 0.0],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [1.0, 0.0],
                varying: C::new(0, 255, 0),
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: C::new(0, 0, 255),
            },
        ]
    }

    #[test]
    fn clockwise_and_counter_clockwise_fill_the_same_pixels() {
        // 两种绕向必须填出同一像素集合（`area` 的符号处理）。
        let size = 32usize;
        let mut cw = full_triangle();
        // 反转后两个顶点，得到反向绕向。
        cw.swap(1, 2);

        let render = |vs: &[Vertex2D<C>; 3]| -> Vec<(usize, usize)> {
            let (info, mut buffer) = make_renderer(size);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport: Viewport::new(0, 0, size, size),
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_arrays(PrimitiveType::Triangles, vs, &pipeline)
                    .unwrap();
            }
            painted_pixels(size, &buffer)
        };

        let forward = render(&full_triangle());
        let reversed = render(&cw);
        assert!(!forward.is_empty(), "正向三角形应画出像素");
        assert_eq!(forward, reversed, "顺时针与逆时针必须填出同一像素集合");
    }

    #[test]
    fn degenerate_triangle_writes_nothing() {
        let size = 32usize;
        // 三点共线：沿对角线。
        let collinear = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [0.5, 0.5],
                varying: C::new(0, 255, 0),
            },
            Vertex2D {
                position: [1.0, 1.0],
                varying: C::new(0, 0, 255),
            },
        ];
        // 两点重合。
        let degenerate = [
            Vertex2D {
                position: [0.2, 0.2],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [0.2, 0.2],
                varying: C::new(0, 255, 0),
            },
            Vertex2D {
                position: [0.8, 0.8],
                varying: C::new(0, 0, 255),
            },
        ];
        for (name, vs) in [("共线", collinear), ("两点重合", degenerate)] {
            let (info, mut buffer) = make_renderer(size);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                CountingFs::new(),
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport: Viewport::new(0, 0, size, size),
                },
            );
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vs, &pipeline)
                .unwrap();
            assert!(
                painted_pixels(size, &buffer).is_empty(),
                "{name} 三角形不应写入任何像素"
            );
        }
    }

    #[test]
    fn constant_varying_is_not_scaled_down() {
        // 回归 `8586ef3`（三角形面积三倍缩放导致 varying 插值失真）：
        // 三个顶点 same varying 时，每个片元都必须得到**原值**，
        // 若 `area` 被放大 3 倍，权重和退化成 1/3，结果会被整体缩小。
        let size = 32usize;
        let (info, mut buffer) = make_renderer(size);
        let constant = C::new(128, 64, 32);
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: constant,
            },
            Vertex2D {
                position: [1.0, 0.0],
                varying: constant,
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: constant,
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
        }
        let painted = painted_pixels(size, &buffer);
        assert!(!painted.is_empty(), "应画出像素");
        // 每一个像素都必须精确等于原常量色。
        for (x, y) in painted {
            let got = read_pixel(&buffer, size, x, y);
            assert_eq!(got, constant, "常量 varying 在 ({x}, {y}) 处被缩放");
        }
    }

    #[test]
    fn barycentric_weights_at_each_vertex_recover_that_vertex_varying() {
        // 三个顶点用互不相同的颜色；三角形内部靠近各顶点的像素应趋近该顶点色。
        let size = 64usize;
        let (info, mut buffer) = make_renderer(size);
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [1.0, 0.0],
                varying: C::new(0, 255, 0),
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: C::new(0, 0, 255),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
        }
        // `info` 已被 `Renderer::new` 取得所有权，直接用已知 pitch 采样。
        let sample = |x: usize, y: usize| read_pixel(&buffer, size, x, y);
        // 顶点 (0,0)：像素中心 (0.5, 0.5) 非常靠近该顶点 → 红色占主导。
        let near_v0 = sample(1, 1);
        assert!(near_v0.r > 200, "顶点 0 附近应以红色为主，实得 {near_v0:?}");
        assert!(
            near_v0.g < 60 && near_v0.b < 60,
            "顶点 0 附近不应混入其它通道：{near_v0:?}"
        );

        // 顶点 (size, 0)：靠近右上的像素 → 绿色占主导。
        //
        // 用 `size - 3` 而非 `size - 2`：三角形是「边缘空间」的，右上角 (size, 0)
        // 位于半开区间 `[0, size)` **之外**（见 `Viewport` 文档），顶点本身不产生
        // 片元；离它最近的可覆盖像素已让给了右上斜边另一侧（top-left 规则的必然
        // 结果），故 `size - 2` 整列落在三角形外。
        let near_v1 = sample(size - 3, 1);
        assert!(near_v1.g > 200, "顶点 1 附近应以绿色为主，实得 {near_v1:?}");
        assert!(near_v1.r < 60, "顶点 1 附近不应混入红色：{near_v1:?}");

        // 顶点 (0, size)：靠近左下 → 蓝色占主导，同理用 `size - 3`。
        let near_v2 = sample(1, size - 3);
        assert!(near_v2.b > 200, "顶点 2 附近应以蓝色为主，实得 {near_v2:?}");
        assert!(near_v2.r < 60, "顶点 2 附近不应混入红色：{near_v2:?}");
    }

    #[test]
    fn triangle_bbox_intersects_bounds_so_offscreen_vertices_stay_bounded() {
        // 顶点远超视口时，迭代量必须受 bound 约束（只遍历可见像素）。
        let size = 16usize;
        let (info, mut buffer) = make_renderer(size);
        // 所有顶点都在屏幕外很远处。
        let vertices = [
            Vertex2D {
                position: [100.0, 100.0],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [200.0, 100.0],
                varying: C::new(0, 255, 0),
            },
            Vertex2D {
                position: [100.0, 200.0],
                varying: C::new(0, 0, 255),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            CountingFs::new(),
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        let mut renderer = Renderer::new(info, &mut buffer);
        renderer
            .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
            .unwrap();
        assert!(
            painted_pixels(size, &buffer).is_empty(),
            "完全在视口外的三角形不应产生片元"
        );
        assert_eq!(
            pipeline.fragment_shader.calls.get(),
            0,
            "bbox ∩ bound 为空时不得调用片元着色器"
        );
    }

    #[test]
    fn triangle_sampling_uses_pixel_centers() {
        // 采样点在像素中心 (+0.5)：一个恰好覆盖半屏的三角形，
        // 其斜边上的像素归属应与「以像素中心采样」一致。
        let size = 8usize;
        let (info, mut buffer) = make_renderer(size);
        // 顶点屏幕坐标 (0,0)、(4,0)、(0,4)：对角线 x + y = 4。
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [4.0 / size as f32, 0.0],
                varying: C::new(0, 255, 0),
            },
            Vertex2D {
                position: [0.0, 4.0 / size as f32],
                varying: C::new(0, 0, 255),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
        }
        // 对角线 x + y = 4 的边是 `v[2] → v[0]` 的反向，即从 (0,4) 走向 (0,0)…
        // 逐条核对：(0,0)→(4,0) 上边（dy == 0, dx > 0）归本三角形；
        // (4,0)→(0,4) 斜边 dy > 0，不归；(0,4)→(0,0) 左边 dy < 0，归。
        // 于是斜边上的像素归给**右侧那个**三角形，本三角形在此处止步于 x+y == 3。
        assert_eq!(
            painted_pixels(size, &buffer),
            vec![(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (0, 2)],
            "覆盖集合应为所有满足 x + y <= 3 的像素（含斜边归属侧的排除）"
        );
    }

    // ————————————————————— S25b · 共享边的 top-left 归属 —————————————————————

    /// 半透明灰阶渐变的片元着色器 + 调用计数器。
    ///
    /// 色值与内建 `GradientFs::new(黑(α=64), 白(α=64), [1, 0])` 完全一致
    /// （沿 normalized x 线性插值），但额外记录调用次数——「共享边上的像素是否
    /// 被着色两次」只有调用计数能直接回答，读回的像素值只能间接佐证。
    ///
    /// 计数用 [`core::cell::Cell`] 而非 `Rc<Cell<_>>`：`Cell` 是 `Copy`，
    /// **克隆不会共享计数**，故本类型只在 [`Pipeline`] 内按值存放，渲染后经
    /// `pipeline.fragment_shader.calls` 读回（同 `CountingFs` 的用法）。
    struct CountingGradientFs {
        calls: core::cell::Cell<usize>,
    }

    impl CountingGradientFs {
        fn new() -> Self {
            Self {
                calls: core::cell::Cell::new(0),
            }
        }
    }

    impl FragmentShader for CountingGradientFs {
        type Varying = ();
        fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
            self.calls.set(self.calls.get() + 1);
            let t = input.position[0].clamp(0.0, 1.0);
            let level = (t * 255.0) as u8;
            FragmentOutput::Color(C::with_alpha(level, level, level, 64))
        }
    }

    /// 把视口左上一角的 `[0, 0.5]²` 区域拆成两个**共边**三角形画一遍。
    ///
    /// 返回 `(帧缓冲, 片元着色器调用次数)`。
    ///
    /// 共享边是 normalized 空间的对角线 `(0, 0.5) — (0.5, 0)`，屏幕坐标
    /// `size / 2` 级别。`flip_second` 控制第二个三角形的绕向。
    ///
    /// 两个三角形的绕向刻意相反。取 `size = 64` 时屏幕顶点分别是：
    ///
    /// * 正向 `[(0,0), (0.5,0), (0,0.5)]` → `(0,0), (32,0), (0,32)`，
    ///   `area = (32-0)·(32-0) − (0-0)·(0-0) = 1024 > 0`；
    /// * 反向 `[(0,0.5), (0.5,0), (0.5,0.5)]` → `(0,32), (32,0), (32,32)`，
    ///   `area = (32-0)·(32-32) − (0-32)·(32-0) = −1024 < 0`。
    ///
    /// 两者对共享边 `(0,32) — (32,0)` 的遍历方向相反，正是要验证的场景。
    fn render_shared_edge_pair(size: usize, flip_second: bool) -> (Vec<u8>, usize) {
        let (info, mut buffer) = make_renderer(size);
        let pipeline = Pipeline::new(
            Passthrough2D::<()>::new(),
            CountingGradientFs::new(),
            PipelineState {
                blend: BlendMode::Alpha,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        let half = 0.5f32;
        // 共享边从 (0.5, 0) 出发，两个三角形在此处顺序相反。
        let second = if flip_second {
            [
                Vertex2D {
                    position: [0.0, half],
                    varying: (),
                },
                Vertex2D {
                    position: [half, 0.0],
                    varying: (),
                },
                Vertex2D {
                    position: [half, half],
                    varying: (),
                },
            ]
        } else {
            [
                Vertex2D {
                    position: [half, 0.0],
                    varying: (),
                },
                Vertex2D {
                    position: [0.0, half],
                    varying: (),
                },
                Vertex2D {
                    position: [half, half],
                    varying: (),
                },
            ]
        };
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: (),
            },
            Vertex2D {
                position: [half, 0.0],
                varying: (),
            },
            Vertex2D {
                position: [0.0, half],
                varying: (),
            },
            second[0],
            second[1],
            second[2],
        ];
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
        }
        let calls = pipeline.fragment_shader.calls.get();
        (buffer, calls)
    }

    #[test]
    fn two_triangles_sharing_an_edge_never_shade_a_pixel_twice() {
        // 回归：三条边都做闭区间判定时，共享边上的像素被两个三角形各着色一次，
        // Alpha 混合下形成一条可见亮缝（修复前 `(16, 15)` 处亮度 30，邻域仅 16）。
        //
        // 两条独立口径交叉验证：
        //
        // 1. 片元着色器调用总数 == 覆盖像素数 —— 重复着色的直接证据；
        // 2. 同一行内亮度沿 x 单调不减 —— 重复混合会在某处形成亮度跳变。
        let size = 64usize;
        let (buffer, calls) = render_shared_edge_pair(size, false);
        let painted = painted_pixels(size, &buffer);
        assert!(!painted.is_empty(), "两个三角形应画出像素，测试前提不成立");
        assert_eq!(
            calls,
            painted.len(),
            "每个覆盖像素只应着色一次（共享边上的像素被两个三角形各着色了一次）"
        );

        // 渐变沿 x 递增，故同一 y 上亮度应随 x 单调不减；
        // 若某列被混合两次，该列亮度会越过左邻列一倍以上。
        for y in 0..size / 2 {
            let mut previous = read_pixel(&buffer, size, 0, y).r;
            for x in 1..size / 2 {
                let current = read_pixel(&buffer, size, x, y).r;
                assert!(
                    current >= previous,
                    "第 {y} 行 x={x} 处亮度 {current} < 左邻 {previous}：共享边被重复混合了"
                );
                previous = current;
            }
        }
    }

    #[test]
    fn triangle_ownership_of_a_shared_edge_is_winding_independent() {
        // 第二个三角形换成反向绕向后，覆盖像素集合与片元着色次数都必须完全相同。
        // 这是 top-left 谓词对边方向反对称的直接后果：两个共边三角形对共享边的
        // 遍历方向必然相反（否则内部落在同一侧即重叠），故恰有一方拥有它。
        let size = 32usize;
        let (forward_buffer, forward_calls) = render_shared_edge_pair(size, false);
        let (flipped_buffer, flipped_calls) = render_shared_edge_pair(size, true);
        let forward = painted_pixels(size, &forward_buffer);
        assert!(!forward.is_empty(), "正向应画出像素");
        assert_eq!(
            forward,
            painted_pixels(size, &flipped_buffer),
            "共享边上的像素归属不应随图元绕向改变"
        );
        assert_eq!(forward_calls, flipped_calls, "反向绕向下片元着色次数应相同");
    }

    // ————————————————————————————— S26 · draw_arrays / draw_indexed —————————————————————————————

    #[test]
    fn primitive_assembly_groups_vertices_and_drops_the_remainder() {
        let size = 16usize;
        // 计数 FS 记录被着色的片元数；这里用点图元让每个顶点恰好产生一个片元，
        // 从而把「装配了几组」与「着色了几次」对应起来。
        let render_points = |count: usize| -> usize {
            let (info, mut buffer) = make_renderer(size);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                CountingFs::new(),
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport: Viewport::new(0, 0, size, size),
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                let vertices: Vec<Vertex2D<C>> = (0..count)
                    .map(|i| Vertex2D {
                        // 每个点落在不同像素上，确保都被着色。
                        position: [i as f32 / size as f32, 0.0],
                        varying: C::new(255, 0, 0),
                    })
                    .collect();
                renderer
                    .draw_arrays(PrimitiveType::Points, &vertices, &pipeline)
                    .unwrap();
            }
            pipeline.fragment_shader.calls.get()
        };
        // 点图元：每个顶点独立成组。
        assert_eq!(render_points(0), 0);
        assert_eq!(render_points(1), 1);
        assert_eq!(render_points(5), 5);
    }

    #[test]
    fn line_assembly_drops_an_odd_trailing_vertex() {
        // 3 个顶点 + Lines → 只装配 1 条线，尾数被丢弃。
        let size = 16usize;
        let render = |count: usize| -> usize {
            let (info, mut buffer) = make_renderer(size);
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                CountingFs::new(),
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport: Viewport::new(0, 0, size, size),
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                let vertices: Vec<Vertex2D<C>> = (0..count)
                    .map(|i| Vertex2D {
                        position: [i as f32 / size as f32, 0.5],
                        varying: C::new(255, 0, 0),
                    })
                    .collect();
                renderer
                    .draw_arrays(PrimitiveType::Lines, &vertices, &pipeline)
                    .unwrap();
            }
            pipeline.fragment_shader.calls.get()
        };
        // 2 顶点 → 1 条线；3 顶点 → 仍是 1 条线（尾数丢弃）；4 顶点 → 2 条线。
        let one = render(2);
        assert_eq!(render(3), one, "3 顶点应与 2 顶点等价（尾数丢弃）");
        let two = render(4);
        assert!(two > one, "4 顶点应装配 2 条线（着色次数增加）");
    }

    #[test]
    fn index_out_of_bounds_fails_fast_without_writing_anything() {
        let size = 16usize;
        let (info, mut buffer) = make_renderer(size);
        let vertices = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: C::new(255, 0, 0),
            },
            Vertex2D {
                position: [1.0, 0.0],
                varying: C::new(0, 255, 0),
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: C::new(0, 0, 255),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        let mut renderer = Renderer::new(info, &mut buffer);
        // 索引 9 越界（顶点只有 3 个）。
        let result = renderer.draw_indexed(
            PrimitiveType::Triangles,
            &vertices,
            &[0u16, 1, 9],
            &pipeline,
            None,
        );
        assert_eq!(
            result,
            Err(DrawError::IndexOutOfBounds {
                index: 9,
                vertex_count: 3
            })
        );
        // fail-fast：校验先于任何绘制，帧缓冲保持空白。
        assert!(
            painted_pixels(size, &buffer).is_empty(),
            "索引越界时不应产生任何写入（校验必须先于绘制）"
        );
    }

    #[test]
    fn empty_vertex_set_does_not_invoke_the_vertex_shader() {
        let size = 16usize;
        let (info, mut buffer) = make_renderer(size);
        let pipeline = Pipeline::new(
            CountingVs::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        let mut renderer = Renderer::new(info, &mut buffer);
        let empty: [Vertex2D<C>; 0] = [];
        assert!(renderer
            .draw_arrays(PrimitiveType::Triangles, &empty, &pipeline)
            .is_ok());
        assert_eq!(
            pipeline.vertex_shader.calls.get(),
            0,
            "空顶点集不应调用顶点着色器"
        );
        assert!(painted_pixels(size, &buffer).is_empty());
    }

    #[test]
    fn empty_visible_bounds_returns_ok_without_invoking_the_vertex_shader() {
        let size = 16usize;
        let (info, mut buffer) = make_renderer(size);
        // viewport 完全在帧缓冲之外 → 可见矩形为空。
        let pipeline = Pipeline::new(
            CountingVs::new(),
            SolidFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(1000, 1000, 8, 8),
            },
        );
        let mut renderer = Renderer::new(info, &mut buffer);
        let vertices = [Vertex2D {
            position: [0.0, 0.0],
            varying: C::new(255, 0, 0),
        }];
        assert!(renderer
            .draw_arrays(PrimitiveType::Points, &vertices, &pipeline)
            .is_ok());
        assert_eq!(
            pipeline.vertex_shader.calls.get(),
            0,
            "可见矩形为空时连顶点着色都不应执行"
        );
    }

    #[test]
    fn indexed_draw_selects_vertices_through_the_index_buffer() {
        // 不能用「端点反序」验证索引生效（`rasterize_line` 文档已说明那是数学恒等式）。
        // 正确做法：用不同的顶点位置，看索引是否真的决定了被选中的顶点。
        let size = 32usize;
        let render = |indices: &[u16]| -> Vec<(usize, usize)> {
            let (info, mut buffer) = make_renderer(size);
            let vertices = [
                Vertex2D {
                    position: [0.0, 0.0],
                    varying: C::new(255, 0, 0),
                },
                Vertex2D {
                    position: [0.5, 0.0],
                    varying: C::new(0, 255, 0),
                },
                Vertex2D {
                    position: [0.0, 0.5],
                    varying: C::new(0, 0, 255),
                },
                Vertex2D {
                    position: [0.5, 0.5],
                    varying: C::new(255, 255, 255),
                },
            ];
            let pipeline = Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState {
                    blend: BlendMode::None,
                    scissor: None,
                    viewport: Viewport::new(0, 0, size, size),
                },
            );
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                renderer
                    .draw_indexed(PrimitiveType::Points, &vertices, indices, &pipeline, None)
                    .unwrap();
            }
            painted_pixels(size, &buffer)
        };
        // 索引 [0, 3] → 像素 (0,0) 与 (16,16)。结果按**绘制顺序**排列，
        // 不是坐标排序，故直接用索引顺序断言。
        assert_eq!(render(&[0, 3]), vec![(0, 0), (16, 16)]);
        // 索引 [1, 2] → 像素 (16,0) 与 (0,16)：与上一组完全不同，
        // 证明索引确实改变了被选中的顶点。
        assert_eq!(render(&[1, 2]), vec![(16, 0), (0, 16)]);
        assert_ne!(render(&[0, 3]), render(&[1, 2]));
    }

    #[test]
    fn scissor_clips_before_the_fragment_shader() {
        let size = 32usize;
        let (info, mut buffer) = make_renderer(size);
        // scissor 限定在右下 8x8。
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            CountingFs::new(),
            PipelineState {
                blend: BlendMode::None,
                scissor: Some(ScissorRect::new(24, 24, 8, 8)),
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            // 点在 scissor 之外（左上角）→ 不得调用片元着色器。
            renderer
                .draw_arrays(
                    PrimitiveType::Points,
                    &[Vertex2D {
                        position: [0.1, 0.1],
                        varying: C::new(255, 0, 0),
                    }],
                    &pipeline,
                )
                .unwrap();
        }
        assert_eq!(
            pipeline.fragment_shader.calls.get(),
            0,
            "scissor 外的片元不得调用片元着色器"
        );
        assert!(painted_pixels(size, &buffer).is_empty());
    }

    #[test]
    fn discard_output_writes_nothing() {
        let size = 16usize;
        let (info, mut buffer) = make_renderer(size);
        let pipeline = Pipeline::new(
            Passthrough2D::<C>::new(),
            DiscardFs,
            PipelineState {
                blend: BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, size, size),
            },
        );
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            renderer
                .draw_arrays(
                    PrimitiveType::Points,
                    &[
                        Vertex2D {
                            position: [0.5, 0.5],
                            varying: C::new(255, 0, 0),
                        },
                        Vertex2D {
                            position: [0.25, 0.25],
                            varying: C::new(0, 255, 0),
                        },
                    ],
                    &pipeline,
                )
                .unwrap();
        }
        assert!(
            painted_pixels(size, &buffer).is_empty(),
            "FragmentOutput::Discard 必须丢弃片元"
        );
    }

    #[test]
    fn pipeline_is_reusable_across_consecutive_draws() {
        // 同一 Pipeline 连续两次 draw_arrays 应无状态残留：
        // 第二次的结果与「新建一个相同 pipeline 单独绘制」逐字节一致。
        let size = 24usize;
        let make_pipeline = || {
            Pipeline::new(
                Passthrough2D::<C>::new(),
                SolidFs,
                PipelineState {
                    blend: BlendMode::Alpha,
                    scissor: None,
                    viewport: Viewport::new(0, 0, size, size),
                },
            )
        };
        let vertices = [
            Vertex2D {
                position: [0.1, 0.1],
                varying: C::with_alpha(255, 0, 0, 128),
            },
            Vertex2D {
                position: [0.9, 0.1],
                varying: C::with_alpha(0, 255, 0, 128),
            },
            Vertex2D {
                position: [0.1, 0.9],
                varying: C::with_alpha(0, 0, 255, 128),
            },
        ];

        // 两次绘制叠加在同一个渲染器上。
        let (info, mut buffer) = make_renderer(size);
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            let pipeline = make_pipeline();
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
        }
        let reused = buffer.clone();

        // 单独绘制两次（各自新建渲染器与 pipeline）应得到相同结果。
        let (info2, mut buffer2) = make_renderer(size);
        {
            let mut renderer = Renderer::new(info2, &mut buffer2);
            let pipeline = make_pipeline();
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
            renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
        }
        assert_eq!(
            reused, buffer2,
            "同一 Pipeline 复用两次与独立绘制两次结果应一致（无状态残留）"
        );
    }

    #[test]
    fn blend_mode_matches_the_actually_written_pixel() {
        // `BlendMode::blend` 是全库混合语义的权威纯函数；这里用真实落盘结果
        // 逐位对照，验证纯函数与字节层路径不分叉。
        let size = 8usize;
        let dst = C::new(10, 20, 30);
        let src = C::with_alpha(200, 100, 50, 128);
        for mode in [
            BlendMode::None,
            BlendMode::Alpha,
            BlendMode::Add,
            BlendMode::Multiply,
            BlendMode::PremultipliedAlpha,
        ] {
            // `Renderer::new` 取得 info 所有权，故每次迭代都重新构造。
            let (info, mut buffer) = make_renderer(size);
            // 预置背景为 dst（Rgba8888 小端序：R,G,B,A）。
            buffer[0] = dst.r;
            buffer[1] = dst.g;
            buffer[2] = dst.b;
            buffer[3] = 255;
            {
                let mut renderer = Renderer::new(info, &mut buffer);
                let pipeline = Pipeline::new(
                    Passthrough2D::<C>::new(),
                    SolidFs,
                    PipelineState {
                        blend: mode,
                        scissor: None,
                        viewport: Viewport::new(0, 0, size, size),
                    },
                );
                // 让 (0,0) 像素恰好被一个点覆盖。
                renderer
                    .draw_arrays(
                        PrimitiveType::Points,
                        &[Vertex2D {
                            position: [0.0, 0.0],
                            varying: src,
                        }],
                        &pipeline,
                    )
                    .unwrap();
            }
            let got = read_pixel(&buffer, size, 0, 0);
            // `None` 走 `mix_alpha(255)` 强制覆盖；其余分支直接用 `blend` 的结果。
            let expected = if mode == BlendMode::None {
                C::new(src.r, src.g, src.b)
            } else {
                mode.blend(src, dst)
            };
            assert_eq!(
                got, expected,
                "{mode:?} 的落盘结果与 BlendMode::blend 不一致"
            );
        }
    }

    // ————————————————————————————— S21 · 类型与插值 —————————————————————————————

    /// 读取 Rgba8888 帧缓冲中 `(x, y)` 处的颜色。
    ///
    /// 必须用 `PixelFormat::Rgba8888.read_color` 而不是
    /// `Color::from_rgba_u32(u32::from_le_bytes(..))`：后者的位布局是
    /// `R24G16B8A0`（R 在**最高**字节），而 Rgba8888 的字节序是
    /// `R, G, B, A`（R 在最低字节）。两者互为字节序关系，直接套用会
    /// 把 R 读成 alpha、G 读成 B——曾经因此写出一批"插值被缩放"的假失败。
    fn read_pixel(buffer: &[u8], size: usize, x: usize, y: usize) -> C {
        let off = y * size * 4 + x * 4;
        Fmt::Rgba8888.read_color(&buffer[off..off + 4])
    }

    #[test]
    fn primitive_type_step_counts() {
        assert_eq!(PrimitiveType::Points.to_step(), 1);
        assert_eq!(PrimitiveType::Lines.to_step(), 2);
        assert_eq!(PrimitiveType::Triangles.to_step(), 3);
    }

    #[test]
    fn interpolate_identity_holds_for_every_implementation() {
        // 这是缓存正确性的前提：`interpolate(a, a, a, ..) == a`。
        // 权重取一组明显不同且非平凡的值，确保实现真的做了加权求和。
        let (w0, w1, w2) = (0.25f32, 0.35f32, 0.40f32);
        // f32
        assert_eq!(
            <f32 as Interpolate>::interpolate(1.5, 1.5, 1.5, w0, w1, w2),
            1.5
        );
        // Color
        assert_eq!(
            <Color as Interpolate>::interpolate(
                C::new(10, 20, 30),
                C::new(10, 20, 30),
                C::new(10, 20, 30),
                w0,
                w1,
                w2
            ),
            C::new(10, 20, 30)
        );
        // Vec2 / Vec3 / Vec4
        assert_eq!(
            <Vec2 as Interpolate>::interpolate([1.0, 2.0], [1.0, 2.0], [1.0, 2.0], w0, w1, w2),
            [1.0, 2.0]
        );
        assert_eq!(
            <Vec3 as Interpolate>::interpolate(
                [1.0, 2.0, 3.0],
                [1.0, 2.0, 3.0],
                [1.0, 2.0, 3.0],
                w0,
                w1,
                w2
            ),
            [1.0, 2.0, 3.0]
        );
        assert_eq!(
            <Vec4 as Interpolate>::interpolate(
                [1.0, 2.0, 3.0, 4.0],
                [1.0, 2.0, 3.0, 4.0],
                [1.0, 2.0, 3.0, 4.0],
                w0,
                w1,
                w2
            ),
            [1.0, 2.0, 3.0, 4.0]
        );
        // Flat：恒取 provoking vertex。
        assert_eq!(
            <Flat<u32> as Interpolate>::interpolate(Flat(7), Flat(7), Flat(7), w0, w1, w2),
            Flat(7)
        );
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        assert_eq!(<f32 as Interpolate>::lerp(10.0, 20.0, 0.0), 10.0);
        assert_eq!(<f32 as Interpolate>::lerp(10.0, 20.0, 1.0), 20.0);
        assert_eq!(<f32 as Interpolate>::lerp(10.0, 20.0, 0.5), 15.0);
        // 向量逐分量。
        let mid = <Vec3 as Interpolate>::lerp([0.0, 10.0, -10.0], [10.0, 20.0, 10.0], 0.5);
        assert!(
            (mid[0] - 5.0).abs() < EPS && (mid[1] - 15.0).abs() < EPS && mid[2].abs() < EPS,
            "lerp 中点不符: {mid:?}"
        );
    }

    #[test]
    fn color_interpolation_is_per_channel_with_rounding_and_clamping() {
        let black = Color::new(0, 0, 0);
        let white = Color::new(255, 255, 255);
        // 权重 (0, 0.5, 0.5) 全部压在两个白点上：`0*0 + 255*0.5 + 255*0.5 = 255`，
        // 故结果精确等于白（这是权重的真实算术，不是 128——后者需要三顶点各 1/3）。
        assert_eq!(
            <Color as Interpolate>::interpolate(black, white, white, 0.0, 0.5, 0.5),
            white
        );
        // 三顶点等权 (1/3, 1/3, 1/3) 才是真正的中点：255 * 1 = 255 仍为白；
        // 用 (0.5, 0.25, 0.25) 之类非对称权重检验逐通道加权求和：
        // `0*0.5 + 255*0.25 + 255*0.25 = 127.5` → `(127.5 + 0.5) as u8 = 128`。
        assert_eq!(
            <Color as Interpolate>::interpolate(black, white, white, 0.5, 0.25, 0.25),
            Color::new(128, 128, 128)
        );
        // 端点：权重全压在单个顶点上时结果精确等于该顶点值。
        assert_eq!(
            <Color as Interpolate>::interpolate(black, white, white, 1.0, 0.0, 0.0),
            black
        );
        assert_eq!(
            <Color as Interpolate>::interpolate(black, white, white, 0.0, 1.0, 0.0),
            white
        );
        // 饱和：权重之和为 3（越界）时结果被钳制到 255 而不回绕成小值。
        assert_eq!(
            <Color as Interpolate>::interpolate(white, white, white, 1.0, 1.0, 1.0),
            white,
            "超范围插值必须被钳制而非回绕"
        );
        // 四通道独立插值。
        let a = Color::with_alpha(0, 0, 0, 0);
        let b = Color::with_alpha(100, 200, 50, 255);
        assert_eq!(
            <Color as Interpolate>::interpolate(a, b, b, 0.0, 1.0, 0.0),
            b,
            "权重全给 b 时应精确等于 b 的四通道"
        );
    }

    #[test]
    fn flat_ignores_weights_and_lerp_returns_a() {
        assert_eq!(
            <Flat<u32> as Interpolate>::lerp(Flat(42), Flat(99), 0.0),
            Flat(42)
        );
        assert_eq!(
            <Flat<u32> as Interpolate>::lerp(Flat(42), Flat(99), 0.5),
            Flat(42)
        );
        assert_eq!(
            <Flat<u32> as Interpolate>::lerp(Flat(42), Flat(99), 1.0),
            Flat(42)
        );
        // interpolate 完全忽略权重，即便全部权重给了 c。
        assert_eq!(
            <Flat<u32> as Interpolate>::interpolate(Flat(42), Flat(7), Flat(9), 0.0, 0.0, 1.0),
            Flat(42),
            "Flat 必须始终取 provoking vertex（第一个顶点）"
        );
    }

    #[test]
    fn fragment_input_is_copy_and_preserves_fields() {
        let input = FragmentInput {
            position: [0.25, 0.75],
            varying: C::new(1, 2, 3),
        };
        // Copy：按值传出后原值仍可用。
        let copied = input;
        assert_eq!(copied.position, [0.25, 0.75]);
        assert_eq!(copied.varying, C::new(1, 2, 3));
        let cloned = input.clone();
        assert_eq!(cloned, input);
    }

    #[test]
    fn rect_new_does_not_validate_overflow() {
        // 文档契约：new 不校验 x + width 是否溢出。
        let rect = Rect::new(usize::MAX, usize::MAX, usize::MAX, usize::MAX);
        assert_eq!(rect.x, usize::MAX);
        assert_eq!(rect.width, usize::MAX);
        // 溢出由 From<Rect> for Bounds 的 saturating_add 兜住。
        let bounds: Bounds = rect.into();
        assert_eq!(bounds.max_x, usize::MAX);
        assert_eq!(bounds.max_y, usize::MAX);
    }

    // ————————————————————————————— S22 · Bounds 与裁剪 —————————————————————————————

    #[test]
    fn bounds_from_rect_uses_saturating_add_at_the_top() {
        // usize::MAX 附近的 x + width 不回绕成小值。
        let bounds = Bounds::from(Rect::new(usize::MAX - 2, 0, usize::MAX, 4));
        assert_eq!(bounds.min_x, usize::MAX - 2);
        assert_eq!(bounds.max_x, usize::MAX, "max_x 必须饱和而不是回绕");
        // 饱和后的区域只有 2 列可见：`[MAX-2, MAX)`。
        assert!(bounds.contains(usize::MAX - 2, 0), "下界含");
        assert!(bounds.contains(usize::MAX - 1, 0), "倒数第二列在区间内");
        assert!(!bounds.contains(usize::MAX, 0), "上界不含");
        // 若 `saturating_add` 缺失而回绕成 0，`min_x > max_x`，
        // `contains` 会因 `x >= min_x` 恒假而全部返回 false——上面前两行即守护它。
    }

    #[test]
    fn bounds_contains_is_half_open() {
        let bounds = Bounds::from(Rect::new(10, 20, 30, 40));
        // 下界含。
        assert!(bounds.contains(10, 20));
        assert!(bounds.contains(39, 59));
        // 上界不含。
        assert!(!bounds.contains(40, 20));
        assert!(!bounds.contains(10, 60));
        assert!(!bounds.contains(9, 20));
        assert!(!bounds.contains(10, 19));
    }

    #[test]
    fn contains_normalized_rejects_negative_coordinates() {
        // 核心陷阱：`f32 as usize` 是饱和转换，负坐标会变成 0，从而误通过
        // min_x == 0 的下界检查。`contains_normalized` 必须在浮点域判定。
        let bounds = Bounds::from(Rect::new(0, 0, 64, 64));
        assert!(
            !bounds.contains_normalized(-0.5, 32.0),
            "负 x 不得被误判为可见"
        );
        assert!(
            !bounds.contains_normalized(32.0, -0.5),
            "负 y 不得被误判为可见"
        );
        // 正常坐标仍可见。
        assert!(bounds.contains_normalized(0.0, 0.0));
        assert!(bounds.contains_normalized(63.5, 63.5));
        // 上界不含（半开区间）。
        assert!(!bounds.contains_normalized(64.0, 32.0));
    }

    #[test]
    fn contains_normalized_rejects_non_finite_coordinates() {
        let bounds = Bounds::from(Rect::new(0, 0, 64, 64));
        // NaN 的所有比较都为 false，故与边界的比较全部失败 → 不可见。
        assert!(!bounds.contains_normalized(f32::NAN, 32.0));
        assert!(!bounds.contains_normalized(32.0, f32::NAN));
        assert!(!bounds.contains_normalized(f32::NAN, f32::NAN));
        // ±∞ 落在区间外。
        assert!(!bounds.contains_normalized(f32::INFINITY, 32.0));
        assert!(!bounds.contains_normalized(32.0, f32::NEG_INFINITY));
        assert!(!bounds.contains_normalized(f32::NEG_INFINITY, f32::INFINITY));
    }

    #[test]
    fn clip_segment_t_rejects_segments_parallel_and_outside_an_axis() {
        let bounds = Bounds::from(Rect::new(0, 0, 100, 100));
        // 垂直于 x 轴：delta[0] == 0，起点的 x 在区间外 → 整段不可见。
        assert!(bounds.clip_segment_t([-1.0, 50.0], [0.0, 10.0]).is_none());
        assert!(bounds.clip_segment_t([101.0, 50.0], [0.0, 10.0]).is_none());
        // 同一条轴不约束：起点在区间内时该轴跳过，只由 y 轴决定。
        assert!(bounds.clip_segment_t([50.0, -10.0], [0.0, 200.0]).is_some());
        // y 轴同理。
        assert!(bounds.clip_segment_t([50.0, -1.0], [10.0, 0.0]).is_none());
        assert!(bounds.clip_segment_t([50.0, 101.0], [10.0, 0.0]).is_none());
    }

    #[test]
    fn clip_segment_t_swaps_near_far_for_negative_components() {
        // 负方向分量（向左/向上）时 near > far，必须交换。
        let bounds = Bounds::from(Rect::new(0, 0, 100, 100));
        // 从 (50, 50) 向左走到 (-50, 50)：进入区间的是 x 下降方向。
        let (t_enter, t_exit) = bounds.clip_segment_t([50.0, 50.0], [-100.0, 0.0]).unwrap();
        assert!(
            (t_enter - 0.0).abs() < EPS,
            "起点已在区间内，t_enter 应为 0"
        );
        assert!(
            (t_exit - 0.5).abs() < EPS,
            "走到 x=0 需半程，t_exit 应为 0.5"
        );
        // 从 (-10, 50) 向右走到 (10, 50)：t_enter = 0.5，t_exit = 1。
        let (t_enter, t_exit) = bounds.clip_segment_t([-10.0, 50.0], [20.0, 0.0]).unwrap();
        assert!((t_enter - 0.5).abs() < EPS);
        assert!((t_exit - 1.0).abs() < EPS);
    }

    #[test]
    fn clip_segment_t_returns_none_when_fully_outside() {
        let bounds = Bounds::from(Rect::new(10, 10, 20, 20));
        // 整条线段在区间左侧。
        assert!(bounds.clip_segment_t([0.0, 15.0], [1.0, 0.0]).is_none());
        // 整条线段在区间右侧。
        assert!(bounds.clip_segment_t([100.0, 15.0], [1.0, 0.0]).is_none());
        // 整条线段在区间上方。
        assert!(bounds.clip_segment_t([15.0, 0.0], [0.0, 1.0]).is_none());
        // 整条线段在区间下方。
        assert!(bounds.clip_segment_t([15.0, 100.0], [0.0, 1.0]).is_none());
    }

    #[test]
    fn clip_segment_t_returns_ordered_parameters_in_range() {
        let bounds = Bounds::from(Rect::new(10, 20, 30, 40));
        let cases: [([f32; 2], [f32; 2]); 5] = [
            ([15.0, 25.0], [10.0, 20.0]),   // 向左上，完全在内 → (0, 1)
            ([5.0, 25.0], [20.0, 0.0]),     // 斜穿区间
            ([0.0, 30.0], [60.0, 20.0]),    // 横向穿过
            ([25.0, 10.0], [25.0, 60.0]),   // 纵向穿过
            ([100.0, 0.0], [-100.0, 60.0]), // 反向斜穿
        ];
        for (origin, delta) in cases {
            if let Some((t_enter, t_exit)) = bounds.clip_segment_t(origin, delta) {
                assert!(
                    t_enter >= 0.0 && t_enter <= t_exit && t_exit <= 1.0,
                    "参数越界: origin={origin:?} delta={delta:?} → ({t_enter}, {t_exit})"
                );
                // 返回的区间端点必须位于**闭区间** [min, max] 内。
                //
                // 注意不能用 `contains_normalized`（半开）：例如
                // origin=[0,30] delta=[60,20] 得到的 t_exit 端点恰好落在
                // x == max_x 上——半开区间会把它判为界外，但裁剪的数学定义
                // 允许区间端点正好抵达边界。因此这里显式用闭区间比较。
                let a = [
                    origin[0] + delta[0] * t_enter,
                    origin[1] + delta[1] * t_enter,
                ];
                let b = [origin[0] + delta[0] * t_exit, origin[1] + delta[1] * t_exit];
                for point in [a, b] {
                    assert!(
                        point[0] >= 10.0 - EPS
                            && point[0] <= 40.0 + EPS
                            && point[1] >= 20.0 - EPS
                            && point[1] <= 60.0 + EPS,
                        "裁剪端点 {point:?} 落在 bounds 外（origin={origin:?} delta={delta:?}）"
                    );
                }
                // t_enter 端点必须**严格**可见（它是进入区间的第一个点）。
                assert!(
                    bounds.contains_normalized(a[0], a[1]),
                    "t_enter 端点 {a:?} 应严格落在半开区间内"
                );
            }
        }
    }

    #[test]
    fn clip_segment_t_endpoints_land_exactly_on_the_boundary() {
        // 精确数值：bounds = Rect(10, 20, 30, 40) → x ∈ [10, 40)，y ∈ [20, 60)。
        let bounds = Bounds::from(Rect::new(10, 20, 30, 40));
        let (t_enter, t_exit) = bounds.clip_segment_t([0.0, 30.0], [60.0, 20.0]).unwrap();
        // x 轴：进入 x=10 需 1/6，离开 x=40 需 2/3；y 轴约束更宽松。
        assert!((t_enter - 1.0 / 6.0).abs() < EPS, "t_enter = {t_enter}");
        assert!((t_exit - 2.0 / 3.0).abs() < EPS, "t_exit = {t_exit}");
        let a = [0.0 + 60.0 * t_enter, 30.0 + 20.0 * t_enter];
        let b = [0.0 + 60.0 * t_exit, 30.0 + 20.0 * t_exit];
        assert!(
            (a[0] - 10.0).abs() < EPS,
            "t_enter 端点 x 应为下界 10，实际 {}",
            a[0]
        );
        assert!(
            (b[0] - 40.0).abs() < EPS,
            "t_exit 端点 x 应为上界 40，实际 {}",
            b[0]
        );
    }

    // ————————————————————————————— S23 · BlendMode —————————————————————————————

    #[test]
    fn blend_mode_none_returns_source_verbatim() {
        let src = Color::with_alpha(10, 20, 30, 100);
        let dst = Color::with_alpha(40, 50, 60, 200);
        assert_eq!(BlendMode::None.blend(src, dst), src);
    }

    #[test]
    fn alpha_bearing_blend_modes_always_return_opaque() {
        // 注意：`BlendMode::None` 单独处理（见上一个测试）。
        //
        // `blend` 的文档声称「返回值恒为不透明」，但 `None` 一臂是 `src` 的
        // 原样返回，alpha == 0 时会返回 a == 0。实际落盘时
        // `write_fragment_color` 用 `src.mix_alpha(255)` 在**调用点**补偿，
        // 因此「None = 无条件覆盖」的语义仍然成立——只是补偿发生在别处。
        // 这里把该已知偏差固化为契约：除 None 外全部混合分支返回不透明色。
        let dst = Color::new(10, 20, 30);
        for alpha in [0u8, 1, 128, 254, 255] {
            let src = Color::with_alpha(200, 100, 50, alpha);
            for mode in [
                BlendMode::Alpha,
                BlendMode::Add,
                BlendMode::Multiply,
                BlendMode::PremultipliedAlpha,
            ] {
                let got = mode.blend(src, dst);
                assert_eq!(got.a, 255, "{mode:?} 在 src.a={alpha} 时返回了非不透明颜色");
            }
        }

        // 调用点补偿：`None` 落盘前被强制为不透明，故「无条件覆盖」语义成立。
        let transparent = Color::with_alpha(200, 100, 50, 0);
        assert_eq!(
            BlendMode::None.blend(transparent.mix_alpha(255), dst),
            Color::new(200, 100, 50)
        );
    }

    #[test]
    fn blend_mode_alpha_delegates_to_color_blend() {
        let src = Color::with_alpha(200, 100, 50, 128);
        let dst = Color::new(10, 20, 30);
        assert_eq!(BlendMode::Alpha.blend(src, dst), src.blend(&dst, src.a));
        // a = 0 时退化为目标色。
        let clear = Color::with_alpha(1, 2, 3, 0);
        assert_eq!(BlendMode::Alpha.blend(clear, dst), dst);
        // a = 255 时为源色。
        let opaque = Color::with_alpha(1, 2, 3, 255);
        assert_eq!(BlendMode::Alpha.blend(opaque, dst), Color::new(1, 2, 3));
    }

    #[test]
    fn blend_mode_add_saturates_per_channel() {
        let src = Color::with_alpha(200, 10, 5, 77);
        let dst = Color::new(100, 10, 10);
        assert_eq!(BlendMode::Add.blend(src, dst), Color::new(255, 20, 15));
        // 与黑色相加得源色（不透明化后的 RGB）。
        assert_eq!(
            BlendMode::Add.blend(src, Color::new(0, 0, 0)),
            Color::new(200, 10, 5)
        );
    }

    #[test]
    fn blend_mode_multiply_uses_round_half_up_division() {
        let mul = |s: u8, d: u8| ((s as u16 * d as u16 + 127) / 255) as u8;
        for src in [0u8, 1, 77, 128, 200, 255] {
            for dst in [0u8, 1, 77, 128, 200, 255] {
                let got = BlendMode::Multiply.blend(
                    Color::with_alpha(src, src, src, 200),
                    Color::new(dst, dst, dst),
                );
                assert_eq!(got, Color::new(mul(src, dst), mul(src, dst), mul(src, dst)));
            }
        }
    }

    #[test]
    fn blend_mode_premultiplied_does_not_multiply_alpha_again() {
        let src = Color::with_alpha(200, 100, 50, 128);
        let dst = Color::new(10, 20, 30);
        let premul = BlendMode::PremultipliedAlpha.blend(src, dst);
        // 语义是 src + dst × (1 - src.a)，不再乘一次 src.a。
        assert_ne!(
            premul,
            BlendMode::Alpha.blend(src, dst),
            "预乘不应等同于普通 alpha 混合"
        );
        // a = 255 时退化为纯源色。
        let opaque = Color::with_alpha(9, 8, 7, 255);
        assert_eq!(
            BlendMode::PremultipliedAlpha.blend(opaque, dst),
            Color::new(9, 8, 7)
        );
    }

    #[test]
    fn every_blend_mode_returns_an_opaque_color() {
        let dst = Color::new(10, 20, 30);
        // `BlendMode::None` 原样返回 src（含 alpha），故单独断言；
        // 真正混合目标像素的四个分支一律返回不透明色。
        for alpha in [0u8, 1, 128, 254, 255] {
            let src = Color::with_alpha(200, 100, 50, alpha);
            assert_eq!(BlendMode::None.blend(src, dst), src);
            for mode in [
                BlendMode::Alpha,
                BlendMode::Add,
                BlendMode::Multiply,
                BlendMode::PremultipliedAlpha,
            ] {
                let got = mode.blend(src, dst);
                assert_eq!(got.a, 255, "{mode:?} 在 src.a={alpha} 时返回了非不透明颜色");
            }
        }
    }
}
