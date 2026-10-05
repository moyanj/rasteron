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
//! `draw_indexed` 的顶点着色结果缓存需要 `alloc`，因此本模块会链接全局分配器。
//!
//! # 坐标与数据流
//!
//! 每个图元依次经过四个阶段，各阶段的坐标空间如下：
//!
//! 1. **顶点着色**：`VS::Input` → [`ProgramVertex`]，位置为 `[0, 1]` 的
//!    normalized viewport 坐标。
//! 2. **图元裁剪**：在 normalized 空间内对该图元做半平面裁剪，保证送往
//!    光栅化的顶点都落在 `[0, 1]²` 内。
//! 3. **视口变换**：`ndc_to_screen` 把 normalized 坐标映射为 viewport 内的
//!    浮点像素坐标。
//! 4. **光栅化 + 片元着色**：计算片元重心权重，插值 varying 后调用片元着色器，
//!    并把结果按 [`BlendMode`] 写入帧缓冲。
//!
//! viewport 之外的部分由 `viewport_intersection` 求交裁剪，[`ScissorRect`]
//! 则作为第二层矩形裁剪叠加在其上。
extern crate alloc;

use crate::color::{Color, decode_565_bytes, encode_565_bytes, pack_565, unpack_565};
use crate::core::Renderer;
use crate::utils::floor_to_usize;
use core::marker::PhantomData;

/// 三角形裁剪后可能产生的最大顶点数。
///
/// 四个半平面各最多切一刀，3 个顶点的凸多边形最多增长到 3 + 4 = 7 个顶点，
/// 取 8 作为安全上界。
const MAX_CLIP_VERTICES: usize = 8;

/// 扇形三角化后可能产生的最大三角形数。
///
/// `n` 个顶点扇形三角化产生 `n - 2` 个三角形，故上界为 `MAX_CLIP_VERTICES - 2`。
const MAX_CLIP_TRIANGLES: usize = MAX_CLIP_VERTICES - 2;

/// 光栅化原语类型。
///
/// 图元的组装方式（每个图元消耗多少顶点）由光栅化阶段决定，`draw_arrays`
/// 与 `draw_indexed` 都按此规则从顶点流中切分：
///
/// | 原语 | 每个图元顶点数 | 输入顶点不足时的处理 |
/// |------|----------------|----------------------|
/// | `Points` | 1 | 逐个顶点绘制 |
/// | `Lines` | 2 | 末尾落单的顶点被忽略 |
/// | `Triangles` | 3 | 末尾不足 3 个的顶点被忽略 |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveType {
    /// 独立点图元，每个顶点产生一个片元。
    Points,
    /// 独立线段图元，每 2 个顶点组成一条线段（不共享端点）。
    Lines,
    /// 独立三角形图元，每 3 个顶点组成一个三角形（不共享边）。
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
///
/// `varying` 是该顶点的插值属性，会在光栅化时按重心权重在三个顶点间插值，
/// 因此其类型必须实现 [`Interpolate`]。
///
/// # 格式
///
/// `[0, 1]` 是 normalized viewport 坐标而非 OpenGL 的 `[-1, 1]` clip space；
/// 后者被映射到 viewport 的那一步由 `ndc_to_screen` 在此约定下完成。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgramVertex<V> {
    /// normalized viewport 坐标，`[0, 1]²` 为可见区域。
    pub position: [f32; 2],
    /// 顶点属性，将在光栅化时插值。
    pub varying: V,
}

/// 片元着色器输入。
///
/// - `varying` 是插值后的片元属性
/// - `position` 是当前片元中心对应的屏幕像素坐标
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FragmentInput<V> {
    /// 由三角形三个顶点插值而来的片元属性；点/线图元则由端点线性插值。
    pub varying: V,
    /// 片元中心的屏幕坐标，形如 `[x + 0.5, y + 0.5]`。
    ///
    /// 取值以像素为单位且相对 **viewport 原点**，因此当 viewport 不位于
    /// `(0, 0)` 时该值并不等于帧缓冲内的绝对像素坐标。
    pub position: [f32; 2],
}

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

/// 单元类型的插值实现：不携带任何属性，插值即恒等。
///
/// 供片元着色器只关心片元坐标、不需要任何 varying 的场景使用。
impl Interpolate for () {
    fn interpolate(_: Self, _: Self, _: Self, _: f32, _: f32, _: f32) -> Self {}
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
impl Interpolate for [f32; 2] {
    fn interpolate(a: Self, b: Self, c: Self, w0: f32, w1: f32, w2: f32) -> Self {
        [
            a[0] * w0 + b[0] * w1 + c[0] * w2,
            a[1] * w0 + b[1] * w1 + c[1] * w2,
        ]
    }
}

/// 四维向量的插值实现：四个分量各自独立插值。
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

/// 顶点着色器。
///
/// 把用户顶点数据 `Input` 变换为 normalized viewport 坐标下的
/// [`ProgramVertex`]，并产出需要插值的 varying 属性。
pub trait VertexShader {
    /// 输入顶点类型，由 `draw_arrays` / `draw_indexed` 的顶点切片提供。
    type Input;
    /// 输出到光栅化阶段的 varying 类型，必须可插值。
    type Varying: Interpolate;

    /// 对单个顶点执行着色。
    ///
    /// 该方法应当无副作用且结果稳定：`draw_indexed` 会按顶点索引缓存结果，
    /// 同一索引可能被多个图元复用。
    fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying>;
}

/// 片元着色器。
///
/// 接收插值后的 varying 与片元屏幕坐标，输出颜色或丢弃该片元。
pub trait FragmentShader {
    /// 输入的 varying 类型，必须与顶点着色器的 [`VertexShader::Varying`] 一致。
    type Varying: Copy;

    /// 对单个片元执行着色。
    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput;
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

/// 视口：把 `[0, 1]` 的 normalized 坐标映射到的屏幕矩形。
pub type Viewport = Rect;
/// 裁剪矩形：片元允许写入的屏幕区域，超出部分既不写着色器也不写帧缓冲。
pub type ScissorRect = Rect;

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

/// 纹理采样环绕模式：决定 `[0, 1]` 之外的纹理坐标如何处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    /// 钳制到边界，坐标超出 `[0, 1]` 时取样边缘 texel。
    Clamp,
    /// 平铺重复，坐标按 `[0, 1]` 取小数部分循环。
    Repeat,
}

/// 纹理采样过滤模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// 最近邻采样：取坐标最近的一个 texel，速度快但放大时有块状锯齿。
    Nearest,
    /// 双线性采样：对相邻 4 个 texel 做二维线性插值，放大更平滑。
    Bilinear,
}

/// 采样状态：环绕模式与过滤模式的组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler2D {
    /// U 轴（水平）环绕模式。
    pub wrap_u: WrapMode,
    /// V 轴（垂直）环绕模式。
    pub wrap_v: WrapMode,
    /// 过滤模式。
    pub filter: FilterMode,
}

impl Sampler2D {
    /// 最近邻 + 双轴钳制的采样状态，适合像素画与 UI 贴图。
    pub const fn nearest_clamp() -> Self {
        Self {
            wrap_u: WrapMode::Clamp,
            wrap_v: WrapMode::Clamp,
            filter: FilterMode::Nearest,
        }
    }

    /// 双线性 + 双轴钳制的采样状态，适合连续色调图像的平滑缩放。
    pub const fn bilinear_clamp() -> Self {
        Self {
            wrap_u: WrapMode::Clamp,
            wrap_v: WrapMode::Clamp,
            filter: FilterMode::Bilinear,
        }
    }
}

impl Default for Sampler2D {
    /// 默认使用 [`Sampler2D::nearest_clamp`]。
    fn default() -> Self {
        Self::nearest_clamp()
    }
}

/// `Texture2D` 构造错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureError {
    /// `width × height` 溢出 `usize`。
    DimensionOverflow,
    /// texel 切片长度与 `width × height` 不符。
    LengthMismatch {
        /// 按 `width × height` 期望的 texel 数量。
        expected: usize,
        /// 实际传入的 texel 数量。
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
    /// 创建纹理，参数非法时恐慌。
    ///
    /// # 恐慌
    ///
    /// * `width × height` 溢出 `usize` → 维度溢出；
    /// * `texels.len() != width × height` → 长度不匹配。
    ///
    /// 需要在运行时处理这些错误时，请改用 [`Texture2D::try_new`]。
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

    /// 创建纹理的校验版本，参数非法时返回 [`TextureError`]。
    ///
    /// # 错误
    ///
    /// * [`TextureError::DimensionOverflow`] — `width × height` 溢出 `usize`；
    /// * [`TextureError::LengthMismatch`] — `texels` 长度不等于 `width × height`。
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

    /// 纹理宽度（texel 数）。
    pub const fn width(&self) -> usize {
        self.width
    }

    /// 纹理高度（texel 数）。
    pub const fn height(&self) -> usize {
        self.height
    }

    /// 以最近邻 + 双轴钳制采样，等价于 `sample(uv, Sampler2D::nearest_clamp())`。
    pub fn sample_nearest(&self, uv: [f32; 2]) -> Color {
        self.sample(uv, Sampler2D::nearest_clamp())
    }

    /// 以双线性 + 双轴钳制采样，等价于 `sample(uv, Sampler2D::bilinear_clamp())`。
    pub fn sample_bilinear(&self, uv: [f32; 2]) -> Color {
        self.sample(uv, Sampler2D::bilinear_clamp())
    }

    /// 按给定采样状态采样纹理。
    ///
    /// 空纹理（`width` 或 `height` 为 0）返回全透明黑，避免越界取样。
    pub fn sample(&self, uv: [f32; 2], sampler: Sampler2D) -> Color {
        if self.width == 0 || self.height == 0 {
            return Color::with_alpha(0, 0, 0, 0);
        }

        match sampler.filter {
            FilterMode::Nearest => {
                self.sample_nearest_with_wrap(uv, sampler.wrap_u, sampler.wrap_v)
            }
            FilterMode::Bilinear => {
                self.sample_bilinear_with_wrap(uv, sampler.wrap_u, sampler.wrap_v)
            }
        }
    }

    /// 最近邻采样：先把 `uv` 按环绕模式归一化，再四舍五入到最近的 texel。
    ///
    /// 乘 `width - 1` 而非 `width`，使 `u = 1.0` 恰好落在最后一个 texel 上，
    /// 保证 `[0, 1]` 完整覆盖整张纹理。
    fn sample_nearest_with_wrap(&self, uv: [f32; 2], wrap_u: WrapMode, wrap_v: WrapMode) -> Color {
        let u = self.wrap_coord(uv[0], wrap_u);
        let v = self.wrap_coord(uv[1], wrap_v);
        let x = (u * (self.width.saturating_sub(1) as f32) + 0.5) as usize;
        let y = (v * (self.height.saturating_sub(1) as f32) + 0.5) as usize;
        self.texel(x, y)
    }

    /// 双线性采样：对相邻 4 个 texel 先水平后垂直做两次线性插值。
    ///
    /// 不足 1 个 texel 的间距（`x1` / `y1` 越过边界）由 [`Self::wrap_texel_index`]
    /// 按环绕模式处理，因此 clamp 与 repeat 在边缘处表现不同。
    fn sample_bilinear_with_wrap(&self, uv: [f32; 2], wrap_u: WrapMode, wrap_v: WrapMode) -> Color {
        let u = self.wrap_coord(uv[0], wrap_u) * self.width.saturating_sub(1) as f32;
        let v = self.wrap_coord(uv[1], wrap_v) * self.height.saturating_sub(1) as f32;
        let x0 = floor_to_usize(u);
        let y0 = floor_to_usize(v);
        let x1 = self.wrap_texel_index(x0.saturating_add(1), self.width, wrap_u);
        let y1 = self.wrap_texel_index(y0.saturating_add(1), self.height, wrap_v);
        // 小数部分即插值权重，取值范围为 [0, 1)
        let tx = u - x0 as f32;
        let ty = v - y0 as f32;

        let c00 = self.texel(x0, y0);
        let c10 = self.texel(x1, y0);
        let c01 = self.texel(x0, y1);
        let c11 = self.texel(x1, y1);

        /// 单通道线性插值：`t = 0` 取 `a`，`t = 1` 取 `b`。
        fn blend_channel(a: u8, b: u8, t: f32) -> f32 {
            a as f32 * (1.0 - t) + b as f32 * t
        }

        /// 逐通道线性插值，返回中间结果以保留 `f32` 精度。
        fn color_lerp(a: Color, b: Color, t: f32) -> [f32; 4] {
            [
                blend_channel(a.r, b.r, t),
                blend_channel(a.g, b.g, t),
                blend_channel(a.b, b.b, t),
                blend_channel(a.a, b.a, t),
            ]
        }

        // 先在 U 方向插值得到上下两条边的颜色，再在 V 方向插值得到最终结果。
        let top = color_lerp(c00, c10, tx);
        let bottom = color_lerp(c01, c11, tx);
        Color::with_alpha(
            (top[0] * (1.0 - ty) + bottom[0] * ty + 0.5).clamp(0.0, 255.0) as u8,
            (top[1] * (1.0 - ty) + bottom[1] * ty + 0.5).clamp(0.0, 255.0) as u8,
            (top[2] * (1.0 - ty) + bottom[2] * ty + 0.5).clamp(0.0, 255.0) as u8,
            (top[3] * (1.0 - ty) + bottom[3] * ty + 0.5).clamp(0.0, 255.0) as u8,
        )
    }

    /// 读取指定 texel。
    ///
    /// # 调用方保证
    ///
    /// `x < width` 且 `y < height`；越界会引发 panic（索引越界）。
    #[inline(always)]
    fn texel(&self, x: usize, y: usize) -> Color {
        self.texels[y * self.width + x]
    }

    /// 把纹理坐标按环绕模式归一化到 `[0, 1]`。
    #[inline(always)]
    fn wrap_coord(&self, coord: f32, mode: WrapMode) -> f32 {
        match mode {
            WrapMode::Clamp => coord.clamp(0.0, 1.0),
            WrapMode::Repeat => Self::repeat_unit(coord),
        }
    }

    /// 把 texel 索引按环绕模式收敛到合法范围 `[0, extent)`。
    ///
    /// 用于双线性采样中 `x0 + 1` / `y0 + 1` 可能越过最后一个 texel 的情况。
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

    /// 取 `value` 的小数部分，结果恒在 `[0, 1)`，即 repeat 环绕的核心。
    ///
    /// 对负值的处理与 `f32::fract` 不同：`fract` 返回负的小数部分
    /// （如 `-0.25f32.fract() == -0.25`），而平铺纹理要求坐标映射回
    /// `[0, 1)`（`-0.25 → 0.75`），故对负值单独补 1。
    #[inline(always)]
    fn repeat_unit(value: f32) -> f32 {
        if value >= 0.0 {
            value - value as usize as f32
        } else {
            let integer = value as i32;
            let frac = value - integer as f32;
            // 整数倍时 frac 为 0，此时不应再 +1
            if frac == 0.0 { 0.0 } else { frac + 1.0 }
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
#[derive(Debug, Clone, Copy)]
pub struct Pipeline<VS, FS> {
    /// 顶点着色器实例。
    pub vertex_shader: VS,
    /// 片元着色器实例。
    pub fragment_shader: FS,
    /// 混合、裁剪与视口状态。
    pub state: PipelineState,
}

impl<VS, FS> Pipeline<VS, FS> {
    /// 组装管线。
    pub const fn new(vertex_shader: VS, fragment_shader: FS, state: PipelineState) -> Self {
        Self {
            vertex_shader,
            fragment_shader,
            state,
        }
    }
}

/// 用于把顶点属性直接作为 varying 传给片元阶段的便捷顶点。
///
/// 与 [`ProgramVertex`] 字段完全一致，配合 [`Passthrough2D`] 使用时可以省去
/// 手写一个纯透传的顶点着色器；也可通过 `From` 转换回 [`ProgramVertex`]。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex2D<V> {
    /// normalized viewport 坐标。
    pub position: [f32; 2],
    /// 直接作为 varying 插值到片元阶段的顶点属性。
    pub varying: V,
}

impl<V: Copy> From<Vertex2D<V>> for ProgramVertex<V> {
    /// 字段一一对应地转换为 [`ProgramVertex`]。
    fn from(value: Vertex2D<V>) -> Self {
        Self {
            position: value.position,
            varying: value.varying,
        }
    }
}

/// 直接透传 [`Vertex2D`] 的内建顶点着色器。
///
/// 不做任何坐标变换，仅把顶点属性原样作为 varying 输出。
#[derive(Debug, Clone, Copy, Default)]
pub struct Passthrough2D<V> {
    /// 仅用于在类型上绑定 varying 类型 `V`，无运行时数据。
    _marker: PhantomData<V>,
}

impl<V> Passthrough2D<V> {
    /// 创建透传顶点着色器。
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

    /// 把 [`Vertex2D`] 原样转换为 [`ProgramVertex`]。
    fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying> {
        (*input).into()
    }
}

/// 裁剪后的线段：两个端点都保证落在 normalized 的 `[0, 1]²` 内。
#[derive(Clone, Copy)]
struct ClippedLine<V> {
    /// 裁剪后的起点。
    start: ProgramVertex<V>,
    /// 裁剪后的终点。
    end: ProgramVertex<V>,
}

/// 裁剪并三角化后的单个三角形，顶点均落在 normalized 的 `[0, 1]²` 内。
#[derive(Clone, Copy)]
struct ClippedTriangle<V> {
    /// 三角形的三个顶点，顺序与原始图元一致。
    vertices: [ProgramVertex<V>; 3],
}

/// 可编程管线对 [`Renderer`] 的扩展。
///
/// 固定功能的 2D 绘图 API（`draw_line` / `fill_triangle` 等）与这里的方法
/// 操作的是同一份帧缓冲与同一套格式缓存，可以混用。
impl<'a> Renderer<'a> {
    /// 以帧缓冲尺寸构造覆盖整块画布的 viewport。
    #[inline(always)]
    fn framebuffer_viewport(&self) -> Viewport {
        Viewport::new(0, 0, self.info().width, self.info().height)
    }

    /// 把 normalized viewport 坐标 `[0, 1]²` 映射为屏幕浮点像素坐标。
    ///
    /// 映射使用 `vw - 1.0` / `vh - 1.0` 作为跨度，使 `0.0` 落在 viewport 第一个
    /// 像素、`1.0` 恰好落在最后一个像素，从而完整覆盖 viewport 而不会越过右/下边界。
    /// viewport 宽或高为 0 时按 1 处理，避免除零式的退化。
    #[inline(always)]
    fn ndc_to_screen(position: [f32; 2], viewport: Viewport) -> [f32; 2] {
        let vx = viewport.x as f32;
        let vy = viewport.y as f32;
        let vw = viewport.width.max(1) as f32;
        let vh = viewport.height.max(1) as f32;
        [vx + position[0] * (vw - 1.0), vy + position[1] * (vh - 1.0)]
    }

    /// 二维叉积形式的边函数，符号表示点 `p` 位于有向边 `a → b` 的哪一侧。
    ///
    /// 三角形光栅化用它一次得到三个重心权重：正规化后即为插值系数。
    /// 返回值为 `(b - a)` 与 `(p - a)` 的叉积，正负取决于顶点绕序。
    #[inline(always)]
    fn edge_function(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
        (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
    }

    /// 向上取整后转为 `usize`；非正值返回 `0`。
    ///
    /// 不能直接用 `f32 as usize`：后者向零取整，对 `2.0` 会得到 2 而非上取整
    /// 应有的 3，对 `-1.0` 还会因负数转无符号而得到 0（依赖 Rust 的饱和转换）。
    /// 这里显式区分整数值与非整数值，用于三角形包围盒的右/下边界。
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

    /// 四舍五入后转为 `usize`，用于线段光栅化中把浮点采样点落到像素中心。
    #[inline(always)]
    fn round_to_usize(value: f32) -> usize {
        floor_to_usize(value + 0.5)
    }

    /// `(min_x, min_y, max_x, max_y)` 矩形与 [`ScissorRect`] 求交。
    /// 若结果为 `None`，表示无可见区域。
    ///
    /// 区间为左闭右开：`min` 含、`max` 不含，与后续 `for x in min_x..=max_x` 的
    /// 闭区间循环之间通过调用方的 `- 1` 衔接。
    #[inline(always)]
    fn rect_intersection(
        (min_x, min_y, max_x, max_y): (usize, usize, usize, usize),
        rect: ScissorRect,
    ) -> Option<(usize, usize, usize, usize)> {
        let min_x = min_x.max(rect.x);
        let min_y = min_y.max(rect.y);
        let max_x = max_x.min(rect.x.saturating_add(rect.width));
        let max_y = max_y.min(rect.y.saturating_add(rect.height));
        if min_x >= max_x || min_y >= max_y {
            None
        } else {
            Some((min_x, min_y, max_x, max_y))
        }
    }

    /// 求帧缓冲可见区域与给定 viewport 的交集。
    ///
    /// 返回 `None` 表示该 viewport 完全落在帧缓冲之外，整个图元可被直接跳过——
    /// 这是所有光栅化入口的第一道裁剪。
    #[inline(always)]
    fn viewport_intersection(&self, viewport: Viewport) -> Option<(usize, usize, usize, usize)> {
        let fb = self.framebuffer_viewport();
        Self::rect_intersection(
            (
                fb.x,
                fb.y,
                fb.x.saturating_add(fb.width),
                fb.y.saturating_add(fb.height),
            ),
            ScissorRect {
                x: viewport.x,
                y: viewport.y,
                width: viewport.width,
                height: viewport.height,
            },
        )
    }

    /// 在已求得 viewport ∩ 帧缓冲的边界上再叠加 scissor 裁剪。
    ///
    /// scissor 为 `None` 时原样返回，不引入额外开销。
    #[inline(always)]
    fn clip_bounds_with_scissor(
        bounds: (usize, usize, usize, usize),
        scissor: Option<ScissorRect>,
    ) -> Option<(usize, usize, usize, usize)> {
        match scissor {
            Some(rect) => Self::rect_intersection(bounds, rect),
            None => Some(bounds),
        }
    }

    /// 直接将颜色写入缓冲区，不进行任何混合。
    ///
    /// 使用缓存格式属性代替 `color.to_bytes(format)` 的枚举匹配。
    /// 对 565 格式，颜色先量化打包再按该格式的存储字节序落盘。
    #[inline(always)]
    fn write_color_raw_at_offset(&mut self, offset: usize, color: &Color) {
        let bpp = self.fmt_bpp;
        let is_rgb_order = self.fmt_rgb_order;
        let stores_alpha = self.fmt_stores_color_alpha;
        // 必须在 `buffer_mut()` 之前读入局部变量，否则与缓冲区的可变借用冲突。
        let bytes_swapped = self.fmt_bytes_swapped;
        let target = self.buffer_mut();
        if bpp == 2 {
            // 565 没有第 3、4 字节，必须先于下方的 dst[1]/dst[2] 写入返回。
            let bytes = encode_565_bytes(pack_565(color), bytes_swapped);
            target[offset..offset + 2].copy_from_slice(&bytes);
            return;
        }
        let dst = &mut target[offset..offset + bpp];
        if is_rgb_order {
            dst[0] = color.r;
            dst[1] = color.g;
            dst[2] = color.b;
        } else {
            dst[0] = color.b;
            dst[1] = color.g;
            dst[2] = color.r;
        }
        if bpp == 4 {
            dst[3] = if stores_alpha { color.a } else { 0 };
        }
    }

    #[inline(always)]
    fn read_color_at_offset(&self, offset: usize) -> Color {
        let bpp = self.fmt_bpp;
        let is_rgb_order = self.fmt_rgb_order;
        let stores_alpha = self.fmt_stores_color_alpha;
        let src = &self.buffer()[offset..offset + bpp];
        if bpp == 2 {
            // 565 没有第 3、4 字节，必须先于下方的 src[3] 读取返回。
            return unpack_565(decode_565_bytes(src, self.fmt_bytes_swapped));
        }
        if is_rgb_order {
            Color::with_alpha(
                src[0],
                src[1],
                src[2],
                if stores_alpha { src[3] } else { 255 },
            )
        } else {
            Color::with_alpha(
                src[2],
                src[1],
                src[0],
                if stores_alpha { src[3] } else { 255 },
            )
        }
    }

    /// 计算片元颜色与目标像素颜色的混合结果（不写入）。
    ///
    /// 这里的运算均在 `Color` 层面进行、结果随后由 `write_color_raw_at_offset`
    /// 按像素格式落盘，与固定功能路径的 `PixelFormat::write_color`（在字节层面
    /// 按格式分支混合）是两条独立实现，但都遵循同一套混合语义。
    #[inline(always)]
    fn blend_pixel(src: Color, dst: Color, blend: BlendMode) -> Color {
        /// 加法混合的单通道实现，饱和到 `255` 防止回绕。
        fn add_channel(src: u8, dst: u8) -> u8 {
            src.saturating_add(dst)
        }
        /// 乘法混合的单通道实现：`src × dst / 255`，`+127` 用于四舍五入。
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
                // 片元颜色已被视为预乘，故只需按 (1 - src.a) 衰减目标后叠加。
                let inv_alpha = 255 - src.a;
                /// 预乘混合的单通道实现：`src + dst × (1 - src.a)`。
                fn blend_channel(src: u8, dst: u8, inv_alpha: u16) -> u8 {
                    src.saturating_add(((dst as u16 * inv_alpha + 127) / 255) as u8)
                }
                Color::with_alpha(
                    blend_channel(src.r, dst.r, inv_alpha as u16),
                    blend_channel(src.g, dst.g, inv_alpha as u16),
                    blend_channel(src.b, dst.b, inv_alpha as u16),
                    blend_channel(src.a, dst.a, inv_alpha as u16),
                )
            }
        }
    }

    /// 把片元颜色按混合模式写入 `(x, y)`。
    ///
    /// # 调用方保证
    ///
    /// `(x, y)` 必须已通过 viewport 与 scissor 裁剪，落在帧缓冲范围内。
    #[inline(always)]
    fn write_fragment_color(&mut self, x: usize, y: usize, color: &Color, blend: BlendMode) {
        let offset = self.pixel_offset(x, y);
        match blend {
            // 完全不透明：跳过读-改-写，直接写。`Color::mix` 在 a == 255 时
            // 恒等于 src，因此这一路径与混合路径的像素结果逐位一致，只是省掉了
            // 一次目标读取 + 3 次乘法除法。GUI/字体等大覆盖面片基本全走这里。
            BlendMode::None => self.write_color_raw_at_offset(offset, color),
            BlendMode::Alpha if color.a == 255 => self.write_color_raw_at_offset(offset, color),
            // 完全透明：`Color::mix` 在 a == 0 时恒等于 dst，即写入是空操作。
            // 直接跳过可以避免一次无用的读-改-写（虚线/镂空模板场景收益明显）。
            BlendMode::Alpha if color.a == 0 => {}
            _ => {
                let dst = self.read_color_at_offset(offset);
                self.write_color_raw_at_offset(offset, &Self::blend_pixel(*color, dst, blend));
            }
        }
    }

    /// 计算 normalized 坐标相对 `[0, 1]²` 裁剪区域的区域码（Cohen–Sutherland 式）。
    ///
    /// 返回 `0` 表示顶点在区域内；非 `0` 时其置位对应越界的边。
    /// 注意这里的 `TOP` / `BOTTOM` 对应 `y < 0` 与 `y > 1`，与 `[0, 1]`
    /// 坐标系下「上边界为 0」的直觉一致（与 `utils::clip_line` 中面向
    /// `[0, width) × [0, height)` 像素坐标的编码含义不同，切勿混用）。
    #[inline(always)]
    fn clip_position_code(position: [f32; 2]) -> u8 {
        /// `x < 0`，越过左边界。
        const LEFT: u8 = 1;
        /// `x > 1`，越过右边界。
        const RIGHT: u8 = 2;
        /// `y < 0`，越过上边界。
        const TOP: u8 = 4;
        /// `y > 1`，越过下边界。
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

    /// 在 `a` 与 `b` 之间按参数 `t` 线性插值出一个新顶点。
    ///
    /// 位置分量直接线性插值，varying 走 [`Interpolate::lerp`]，
    /// 因此 [`Flat`] 这类不影响线性插值的类型会保持 `a` 的值。
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

    /// 对线段做 `[0, 1]²` 裁剪（Liang–Barsky 参数区间法）。
    ///
    /// 用四组半平面约束（左右上下）不断收窄参数区间 `[t0, t1]`，
    /// 最终区间为空说明线段完全不可见，返回 `None`。
    ///
    /// # 参数法相对逐点半平面裁剪的优势
    ///
    /// 线段裁剪在参数域内完成，最多 4 次约束即得出结论，无需构造新顶点数组；
    /// 裁剪后的端点通过 [`Self::interpolate_vertex`] 插值得到，varying 会按
    /// 参数 `t` 正确重分布，不会出现「先裁剪位置再单独裁剪属性」导致的不一致。
    fn clip_line_segment<V: Interpolate + Copy>(
        vertices: [ProgramVertex<V>; 2],
    ) -> Option<ClippedLine<V>> {
        let p0 = vertices[0].position;
        let p1 = vertices[1].position;
        let dx = p1[0] - p0[0];
        let dy = p1[1] - p0[1];
        // 待收窄的参数区间：t=0 对应 p0，t=1 对应 p1
        let mut t0 = 0.0f32;
        let mut t1 = 1.0f32;

        // 施加一条半平面约束，收窄 `[t0, t1]`。
        //
        // `p` 为参数方向上的分量、`q` 为当前位置到边界的距离；返回 `false`
        // 表示线段已被完全排除在该半平面之外，可立即放弃。
        let clip_test = |p: f32, q: f32, t0: &mut f32, t1: &mut f32| -> bool {
            // 与边界平行：此时是否可见只取决于 q 的符号
            if p.abs() <= f32::EPSILON {
                return q >= 0.0;
            }

            let r = q / p;
            if p < 0.0 {
                // 朝区间外走：用 r 抬高下界 t0
                if r > *t1 {
                    return false;
                }
                if r > *t0 {
                    *t0 = r;
                }
            } else {
                // 朝区间内走：用 r 压低上界 t1
                if r < *t0 {
                    return false;
                }
                if r < *t1 {
                    *t1 = r;
                }
            }
            true
        };

        // 四组半平面：x >= 0、x <= 1、y >= 0、y <= 1
        if !clip_test(-dx, p0[0], &mut t0, &mut t1)
            || !clip_test(dx, 1.0 - p0[0], &mut t0, &mut t1)
            || !clip_test(-dy, p0[1], &mut t0, &mut t1)
            || !clip_test(dy, 1.0 - p0[1], &mut t0, &mut t1)
            || t0 > t1
        {
            return None;
        }

        Some(ClippedLine {
            start: Self::interpolate_vertex(vertices[0], vertices[1], t0),
            end: Self::interpolate_vertex(vertices[0], vertices[1], t1),
        })
    }

    /// 构造片元着色器输入，`position` 取自像素中心。
    #[inline(always)]
    fn fragment_input<V>(varying: V, x: usize, y: usize) -> FragmentInput<V> {
        FragmentInput {
            varying,
            position: [x as f32 + 0.5, y as f32 + 0.5],
        }
    }

    /// 对三角形做四个半平面的 Sutherland–Hodgman 裁剪，并把结果三角化。
    ///
    /// 返回顶点扇形三角化的结果：`(顶点数组, 三角形个数)`。裁剪一个凸多边形
    /// 最多产生 `MAX_VERTICES - 2` 个三角形，因此定长数组足以容纳全部输出，
    /// **无需堆分配**——这对内核/裸机场景很关键（原实现每三角形一次 `Vec` 分配）。
    ///
    /// # 算法
    ///
    /// 依次用 `x >= 0`、`x <= 1`、`y >= 0`、`y <= 1` 四条边裁剪输入多边形。
    /// 每条边遍历当前多边形所有边，按「起点/终点是否在内侧」的四种组合决定
    /// 输出：全在内则保留终点；一进一出则先输出交点再输出终点；全在外则无输出。
    /// 凸多边形被半平面裁剪后仍是凸多边形，故四轮之后依然可用扇形三角化。
    ///
    /// 裁剪产生的交点由 [`Self::clip_triangle_intersection`] 插值得到，
    /// varying 随参数 `t` 一起重分布，不会出现属性错位。
    ///
    /// 返回 `None` 表示三角形完全不可见（某轮裁剪后顶点少于 3 个）。
    fn clip_triangle<V: Interpolate + Copy>(
        vertices: [ProgramVertex<V>; 3],
    ) -> Option<([ClippedTriangle<V>; MAX_CLIP_TRIANGLES], usize)> {
        const MAX_VERTICES: usize = MAX_CLIP_VERTICES;

        // input / output 为交替使用的固定缓冲，每轮裁剪后 input 变为本轮输出。
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
            // 逐边遍历时以「上一条边的终点」为 prev，形成闭合环路
            let mut prev = input[input_len - 1];
            let mut prev_inside = Self::clip_triangle_inside(prev.position, edge);

            for index in 0..input_len {
                let curr = input[index];
                let curr_inside = Self::clip_triangle_inside(curr.position, edge);

                if curr_inside {
                    // 由外入内：先补上交点，再输出当前顶点
                    if !prev_inside {
                        output[output_len] = Self::clip_triangle_intersection(prev, curr, edge);
                        output_len += 1;
                    }
                    output[output_len] = curr;
                    output_len += 1;
                } else if prev_inside {
                    // 由内出外：只输出交点
                    output[output_len] = Self::clip_triangle_intersection(prev, curr, edge);
                    output_len += 1;
                }
                // 其余两种情况（两侧都在外）不产生输出

                prev = curr;
                prev_inside = curr_inside;
            }

            input[..output_len].copy_from_slice(&output[..output_len]);
            input_len = output_len;
        }

        // 退化情况：裁剪后不足一个三角形
        if input_len < 3 {
            return None;
        }

        // 扇形三角化：固定首顶点，与相邻两点组成三角形，共 input_len - 2 个
        let mut triangles = [ClippedTriangle {
            vertices: [vertices[0]; 3],
        }; MAX_CLIP_TRIANGLES];
        let mut count = 0usize;
        for index in 1..input_len - 1 {
            triangles[count] = ClippedTriangle {
                vertices: [input[0], input[index], input[index + 1]],
            };
            count += 1;
        }
        Some((triangles, count))
    }

    /// 判断 normalized 坐标点位于第 `edge` 条裁剪边的哪一侧。
    ///
    /// `edge` 依次为左（`x >= 0`）、右（`x <= 1`）、上（`y >= 0`）、下（`y <= 1`），
    /// 与 [`Self::clip_triangle`] 的四轮循环顺序对应。
    #[inline(always)]
    fn clip_triangle_inside(position: [f32; 2], edge: usize) -> bool {
        match edge {
            0 => position[0] >= 0.0,
            1 => position[0] <= 1.0,
            2 => position[1] >= 0.0,
            _ => position[1] <= 1.0,
        }
    }

    /// 求线段 `a → b` 与第 `edge` 条裁剪边的交点，并插值出该点的属性。
    ///
    /// # 调用方保证
    ///
    /// `a` 与 `b` 分居该边两侧，故分母不为 0。若两者在边方向上坐标相同
    /// （退化线段），分母为 0 会得到 `NaN`/`inf`，此时应交由调用方避免。
    fn clip_triangle_intersection<V: Interpolate + Copy>(
        a: ProgramVertex<V>,
        b: ProgramVertex<V>,
        edge: usize,
    ) -> ProgramVertex<V> {
        // 交点参数 t 由「边界值 - a 的该轴坐标」比上「b - a 的该轴增量」得到
        let t = match edge {
            0 => (0.0 - a.position[0]) / (b.position[0] - a.position[0]),
            1 => (1.0 - a.position[0]) / (b.position[0] - a.position[0]),
            2 => (0.0 - a.position[1]) / (b.position[1] - a.position[1]),
            _ => (1.0 - a.position[1]) / (b.position[1] - a.position[1]),
        };
        Self::interpolate_vertex(a, b, t)
    }

    /// 光栅化单个点图元：把顶点映射到屏幕后着色一个片元。
    ///
    /// 顶点完全落在 `[0, 1]²` 之外时整点丢弃（不做裁剪）。
    /// 注意此处用 `as usize`（向零取整）而非四舍五入，故点的落位与
    /// 线段/三角形路径的取整方式不同。
    #[inline(always)]
    fn shade_vertex_point<V, FS>(
        &mut self,
        vertex: ProgramVertex<V>,
        pipeline: &Pipeline<impl VertexShader<Input = impl Copy, Varying = V>, FS>,
    ) where
        V: Copy,
        FS: FragmentShader<Varying = V>,
    {
        let Some((clip_min_x, clip_min_y, clip_max_x, clip_max_y)) = self
            .viewport_intersection(pipeline.state.viewport)
            .and_then(|bounds| Self::clip_bounds_with_scissor(bounds, pipeline.state.scissor))
        else {
            return;
        };
        // 顶点越界直接丢弃（点图元无「部分可见」的概念）
        if Self::clip_position_code(vertex.position) != 0 {
            return;
        }

        let point = Self::ndc_to_screen(vertex.position, pipeline.state.viewport);
        let x = point[0] as usize;
        let y = point[1] as usize;
        // 屏幕坐标可能因 viewport 与 scissor 的组合而落在裁剪区之外
        if x < clip_min_x || x >= clip_max_x || y < clip_min_y || y >= clip_max_y {
            return;
        }

        if let FragmentOutput::Color(color) = pipeline
            .fragment_shader
            .shade_fragment(Self::fragment_input(vertex.varying, x, y))
        {
            self.write_fragment_color(x, y, &color, pipeline.state.blend);
        }
    }

    /// 光栅化线段图元。
    ///
    /// 先做参数域裁剪得到可见区间，再按屏幕坐标的 DDA 步进采样：
    /// 步数取 `max(|dx|, |dy|)`，保证相邻采样点之间最多跨越一个像素、
    /// 不会出现断点。varying 沿线段用同一参数 `t` 线性插值。
    ///
    /// 两端点重合（`steps == 0`）时退化为一个点，交由
    /// [`Self::shade_vertex_point`] 处理。
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
        let Some((clip_min_x, clip_min_y, clip_max_x, clip_max_y)) = self
            .viewport_intersection(pipeline.state.viewport)
            .and_then(|bounds| Self::clip_bounds_with_scissor(bounds, pipeline.state.scissor))
        else {
            return;
        };

        let p0 = Self::ndc_to_screen(clipped.start.position, pipeline.state.viewport);
        let p1 = Self::ndc_to_screen(clipped.end.position, pipeline.state.viewport);
        let dx = p1[0] - p0[0];
        let dy = p1[1] - p0[1];
        let steps = Self::ceil_to_usize(dx.abs().max(dy.abs()));
        if steps == 0 {
            // 退化线段：两端点映射到同一像素，按点图元处理
            self.shade_vertex_point(clipped.start, pipeline);
            return;
        }

        // 闭区间采样，共 steps + 1 个片元，两端点都会被着色
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let px = Self::round_to_usize(p0[0] + dx * t);
            let py = Self::round_to_usize(p0[1] + dy * t);
            // 裁剪区外的采样点跳过，但不终止循环——线段可能穿出再穿入
            if px < clip_min_x || px >= clip_max_x || py < clip_min_y || py >= clip_max_y {
                continue;
            }

            let varying = V::lerp(clipped.start.varying, clipped.end.varying, t);
            if let FragmentOutput::Color(color) = pipeline
                .fragment_shader
                .shade_fragment(Self::fragment_input(varying, px, py))
            {
                self.write_fragment_color(px, py, &color, pipeline.state.blend);
            }
        }
    }

    /// 光栅化三角形图元。
    ///
    /// 先做 Sutherland–Hodgman 裁剪与扇形三角化，再对每个子三角形用
    /// 边函数（重心坐标）扫描填充。
    ///
    /// # 算法
    ///
    /// 1. 算三个边函数值即重心权重 `w0/w1/w2`，三者同号时片元在三角形内。
    /// 2. 用 `sign` 归一化绕序，使顺时针与逆时针三角形都能正确填充。
    /// 3. 权重对 `x`、`y` 都是线性的，故可用增量递推：内层每次 `+ w?_dx`，
    ///    换行时对行首值 `+ w?_dy`，避免逐像素重算叉积。
    /// 4. 除以 `inv_area` 归一化后交给 [`Interpolate::interpolate`] 插值 varying。
    ///
    /// 包围盒按子三角形的屏幕范围与裁剪区求交得到，因此屏幕外的部分
    /// 不会被扫描，代价与可见面积而非viewport面积成正比。
    /// 采样点取像素中心 `(x + 0.5, y + 0.5)`，与 `fragment_input` 汇报的坐标一致。
    fn rasterize_triangle<V, FS>(
        &mut self,
        vertices: [ProgramVertex<V>; 3],
        pipeline: &Pipeline<impl VertexShader<Input = impl Copy, Varying = V>, FS>,
    ) where
        V: Interpolate + Copy,
        FS: FragmentShader<Varying = V>,
    {
        let Some((clipped_tris, clipped_count)) = Self::clip_triangle(vertices) else {
            return;
        };
        let Some((clip_min_x, clip_min_y, clip_max_x, clip_max_y)) = self
            .viewport_intersection(pipeline.state.viewport)
            .and_then(|bounds| Self::clip_bounds_with_scissor(bounds, pipeline.state.scissor))
        else {
            return;
        };

        for clipped in &clipped_tris[..clipped_count] {
            let vertices = clipped.vertices;
            let p0 = Self::ndc_to_screen(vertices[0].position, pipeline.state.viewport);
            let p1 = Self::ndc_to_screen(vertices[1].position, pipeline.state.viewport);
            let p2 = Self::ndc_to_screen(vertices[2].position, pipeline.state.viewport);

            // 包围盒 = 顶点屏幕范围 ∩ 裁剪区；clip_max 为开区间，故减 1 转闭区间
            let min_x = floor_to_usize(p0[0].min(p1[0]).min(p2[0]).max(clip_min_x as f32));
            let min_y = floor_to_usize(p0[1].min(p1[1]).min(p2[1]).max(clip_min_y as f32));
            let max_x =
                Self::ceil_to_usize(p0[0].max(p1[0]).max(p2[0]).min((clip_max_x - 1) as f32));
            let max_y =
                Self::ceil_to_usize(p0[1].max(p1[1]).max(p2[1]).min((clip_max_y - 1) as f32));
            if min_x > max_x || min_y > max_y {
                continue;
            }

            // 有向面积的两倍；绝对值即归一化因子
            let area = Self::edge_function(p0, p1, p2);
            if area.abs() <= f32::EPSILON {
                // 退化三角形（三点共线或重合），无边可填
                continue;
            }

            // 以面积为负判断绕序，使两种绕序都能得到「内部权重 >= 0」的判据
            let sign = if area < 0.0 { -1.0 } else { 1.0 };
            let inv_area = 1.0 / area.abs();
            // 包围盒左上角像素中心作为递推起点
            let start_sample = [min_x as f32 + 0.5, min_y as f32 + 0.5];
            let mut row_w0 = Self::edge_function(p1, p2, start_sample) * sign;
            let mut row_w1 = Self::edge_function(p2, p0, start_sample) * sign;
            let mut row_w2 = Self::edge_function(p0, p1, start_sample) * sign;
            // 权重对 x 的增量（每列一次加法即可推进）
            let w0_dx = (p2[1] - p1[1]) * sign;
            let w1_dx = (p0[1] - p2[1]) * sign;
            let w2_dx = (p1[1] - p0[1]) * sign;
            // 权重对 y 的增量（换行时用于更新行首值）
            let w0_dy = (p1[0] - p2[0]) * sign;
            let w1_dy = (p2[0] - p0[0]) * sign;
            let w2_dy = (p0[0] - p1[0]) * sign;

            for y in min_y..=max_y {
                let mut w0 = row_w0;
                let mut w1 = row_w1;
                let mut w2 = row_w2;
                for x in min_x..=max_x {
                    // 任一权重为负表示在三角形外，跳过该片元
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        w0 += w0_dx;
                        w1 += w1_dx;
                        w2 += w2_dx;
                        continue;
                    }

                    // 权重归一化后作为重心坐标插值 varying
                    let varying = V::interpolate(
                        vertices[0].varying,
                        vertices[1].varying,
                        vertices[2].varying,
                        w0 * inv_area,
                        w1 * inv_area,
                        w2 * inv_area,
                    );
                    match pipeline
                        .fragment_shader
                        .shade_fragment(Self::fragment_input(varying, x, y))
                    {
                        FragmentOutput::Color(color) => {
                            self.write_fragment_color(x, y, &color, pipeline.state.blend);
                        }
                        // 片元被丢弃：不写入，但继续处理同行后续片元
                        FragmentOutput::Discard => {}
                    }
                    w0 += w0_dx;
                    w1 += w1_dx;
                    w2 += w2_dx;
                }
                // 推进到下一行的行首权重
                row_w0 += w0_dy;
                row_w1 += w1_dy;
                row_w2 += w2_dy;
            }
        }
    }

    /// 从顶点数组直接绘制，不经过索引缓冲。
    ///
    /// 顶点按 `PrimitiveType` 的粒度顺序切分，每个顶点都会调用一次顶点着色器
    /// （相邻图元不共享着色结果，需要复用请改用 [`Self::draw_indexed`]）。
    ///
    /// 末尾不足一个完整图元的顶点会被静默忽略：`Lines` 忽略最后 1 个，
    /// `Triangles` 忽略最后 1~2 个。
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

    /// 通过索引缓冲绘制，共享顶点可复用顶点着色结果。
    ///
    /// 顶点着色结果按顶点索引缓存（`shaded_vertices`），每个索引最多着色一次，
    /// 因此相邻三角形共享的顶点不会重复执行顶点着色器。这是本方法相较
    /// [`Self::draw_arrays`] 的主要收益，也是本模块需要 `alloc` 的原因。
    ///
    /// 索引越界（`>= vertices.len()`）时该图元被静默跳过，不会恐慌。
    ///
    /// 与 OpenGL 一致，末组不足一个完整图元的索引同样被忽略。
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
        // 惰性着色缓存：None 表示该索引尚未着色
        let mut shaded_vertices = alloc::vec![None; vertices.len()];
        // 取指定索引的着色结果，未命中时调用顶点着色器并回填缓存。
        //
        // 索引越界时返回 `None`（`get_mut` 的边界检查），由调用方跳过该图元。
        let mut shade = |vertex_index: usize| -> Option<ProgramVertex<VS::Varying>> {
            let slot = shaded_vertices.get_mut(vertex_index)?;
            Some(*slot.get_or_insert_with(|| {
                pipeline.vertex_shader.shade_vertex(&vertices[vertex_index])
            }))
        };

        match primitive {
            PrimitiveType::Points => {
                for index in indices {
                    let vertex_index = *index as usize;
                    if let Some(vertex) = shade(vertex_index) {
                        self.shade_vertex_point(vertex, pipeline);
                    }
                }
            }
            PrimitiveType::Lines => {
                let mut index = 0usize;
                while index + 1 < indices.len() {
                    let i0 = indices[index] as usize;
                    let i1 = indices[index + 1] as usize;
                    // 任一索引越界则整条线段跳过，避免半条线段被绘制
                    if let (Some(v0), Some(v1)) = (shade(i0), shade(i1)) {
                        self.rasterize_line([v0, v1], pipeline);
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
                    // 任一索引越界则整个三角形跳过
                    if let (Some(v0), Some(v1), Some(v2)) = (shade(i0), shade(i1), shade(i2)) {
                        self.rasterize_triangle([v0, v1, v2], pipeline);
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
    use crate::Renderer;
    use crate::color::PixelFormat;
    use crate::framebuffer::FramebufferInfo;
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

        fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
            FragmentOutput::Color(input.varying)
        }
    }

    struct WhiteFs;

    impl FragmentShader for WhiteFs {
        type Varying = ();

        fn shade_fragment(&self, _: FragmentInput<Self::Varying>) -> FragmentOutput {
            FragmentOutput::Color(Color::new(255, 255, 255))
        }
    }

    struct DiscardRightHalfFs;

    impl FragmentShader for DiscardRightHalfFs {
        type Varying = [f32; 2];

        fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
            if input.varying[0] > 0.5 {
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

        fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
            FragmentOutput::Color(self.texture.sample_nearest(input.varying))
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

        assert_eq!(
            renderer.get_pixel(1, 1),
            Some(Color::with_alpha(255, 0, 0, 64))
        );
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

            fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
                FragmentOutput::Color(input.varying.0)
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
    fn fragment_input_exposes_pixel_center_position() {
        struct PixelCenterFs;

        impl FragmentShader for PixelCenterFs {
            type Varying = ();

            fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
                if input.position == [1.5, 1.5] {
                    FragmentOutput::Color(Color::new(255, 255, 255))
                } else {
                    FragmentOutput::Discard
                }
            }
        }

        let mut renderer = test_renderer(4, 4);
        renderer.clear(Color::new(0, 0, 0));
        let vertices = [
            Vertex2D {
                position: [1.0 / 3.0, 1.0 / 3.0],
                varying: (),
            },
            Vertex2D {
                position: [1.0, 1.0 / 3.0],
                varying: (),
            },
            Vertex2D {
                position: [1.0 / 3.0, 1.0],
                varying: (),
            },
        ];
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            PixelCenterFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline);

        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(2, 1), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn draw_indexed_reuses_vertex_shader_results() {
        use core::cell::Cell;

        #[derive(Clone, Copy)]
        struct CountingVs<'a> {
            calls: &'a Cell<usize>,
        }

        impl VertexShader for CountingVs<'_> {
            type Input = Vertex2D<Color>;
            type Varying = Color;

            fn shade_vertex(&self, input: &Self::Input) -> ProgramVertex<Self::Varying> {
                self.calls.set(self.calls.get() + 1);
                (*input).into()
            }
        }

        let calls = Cell::new(0usize);
        let mut renderer = test_renderer(4, 4);
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
                position: [1.0, 1.0],
                varying: Color::new(0, 0, 255),
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: Color::new(255, 255, 0),
            },
        ];
        let indices = [0u16, 1, 2, 0, 2, 3];
        let pipeline = Pipeline::new(
            CountingVs { calls: &calls },
            SolidFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, 4, 4)),
        );

        renderer.draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline);

        assert_eq!(calls.get(), vertices.len());
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

    // ── Rgb565 / Bgr565 ──

    /// 565 的落盘与回读必须与公开的 [`PixelFormat`] 编解码完全一致：
    /// 量化打包后只写 2 字节，绝不触碰相邻像素。
    #[test]
    fn rgb565_raw_write_and_read_match_format_codec() {
        for format in [PixelFormat::Rgb565, PixelFormat::Bgr565] {
            let info = FramebufferInfo::new(4, 1, 8, format);
            let mut front = [0u8; 8];
            let mut renderer = Renderer::new(info, &mut front);

            let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
            renderer.write_color_raw_at_offset(2, &color);

            let expected = color.to_bytes(format);
            assert_eq!(
                &renderer.buffer()[2..4],
                &expected[..2],
                "{format:?} 落盘字节错误"
            );
            assert_eq!(
                renderer.read_color_at_offset(2),
                Color::from_bytes(format, &expected[..2]),
                "{format:?} 回读颜色错误"
            );
            // 相邻像素必须保持原样（565 只有 2 字节，多写会污染邻居）
            assert_eq!(renderer.buffer()[0..2], [0u8; 2], "{format:?} 左邻居被改写");
            assert_eq!(renderer.buffer()[4..8], [0u8; 4], "{format:?} 右邻居被改写");
        }
    }

    /// 565 帧缓冲上的可编程管线：`BlendMode::None` 的落盘必须等于参考量化打包。
    #[test]
    fn rgb565_pipeline_blend_none_matches_reference() {
        for format in [PixelFormat::Rgb565, PixelFormat::Bgr565] {
            let info = FramebufferInfo::new(4, 4, 8, format);
            let mut front = [0u8; 32];
            let mut renderer = Renderer::new(info, &mut front);

            let color = Color::new(0x12, 0x34, 0x56);
            let vertices = [
                Vertex2D {
                    position: [0.0, 0.0],
                    varying: color,
                },
                Vertex2D {
                    position: [1.0, 0.0],
                    varying: color,
                },
                Vertex2D {
                    position: [1.0, 1.0],
                    varying: color,
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

            let mut want = [0u8; 2];
            format.write_color(&mut want, &color);
            let mut written = 0usize;
            for y in 0..4 {
                for x in 0..4 {
                    let off = y * 8 + x * 2;
                    let got = &renderer.buffer()[off..off + 2];
                    if got != [0u8; 2] {
                        assert_eq!(got, &want[..], "{format:?} ({x}, {y})");
                        written += 1;
                    }
                }
            }
            assert!(written > 0, "{format:?} 三角形没有写入任何像素");
        }
    }
}
