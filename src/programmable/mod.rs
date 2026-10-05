use crate::utils::{ceil_to_usize, floor_to_usize, normalized_to_screen};
use crate::{Renderer, color::Color, error::DrawError};
pub mod cache;
pub mod shader;

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
        for i in 0..=steps {
            let t = i as f32 / denom;
            let (x, y) = (
                floor_to_usize(a[0] + d[0] * t + 0.5),
                floor_to_usize(a[1] + d[1] * t + 0.5),
            );
            if !bound.contains(x, y) {
                continue;
            }

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

        // 面积为零 = 三点共线，无内部像素。顺带避免权重除零。
        let area = edge(v[1], v[2], v[0]) + edge(v[2], v[0], v[1]) + edge(v[0], v[1], v[2]);
        if area == 0.0 {
            return;
        }

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

        for y in y0..y1 {
            for x in x0..x1 {
                // 采样点取像素中心，否则覆盖判定会整体偏移半像素
                let p = [x as f32 + 0.5, y as f32 + 0.5];

                let w0 = edge(v[1], v[2], p);
                let w1 = edge(v[2], v[0], p);
                let w2 = edge(v[0], v[1], p);
                // 三条边全同号即在内部。用 area 的符号统一两方向，
                // 顺时针的三角形不会只填出半个。
                if (w0 * area < 0.0) || (w1 * area < 0.0) || (w2 * area < 0.0) {
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
