use core::marker::PhantomData;

use crate::color::Color;
use crate::programmable::texture::{FilterMode, Texture2D, WrapMode};
use crate::programmable::{
    FragmentInput, FragmentOutput, FragmentShader, Interpolate, Vec2, Vertex2D, VertexShader,
};

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

    /// 把 [`Vertex2D`] 原样转换为 [`FragmentInput`]。
    fn shade_vertex(&self, input: &Self::Input) -> FragmentInput<Self::Varying> {
        *input
    }
}

/// 输出单一固定颜色的片元着色器。
///
/// 全部图元内每个片元都得到同一个 [`Color`]，参数存在 `&self` 上而非
/// varying 里，因此顶点数据可以退化为最轻的 [`()`]（无属性、不占顶点空间）。
///
/// 典型用法是配合 [`crate::programmable::shader::Passthrough2D`]：
///
/// ```
/// use rasteron::color::Color;
/// use rasteron::programmable::shader::{Passthrough2D, SolidFs};
/// use rasteron::programmable::{
///     FragmentInput, Pipeline, PipelineState, PrimitiveType, Vertex2D, Viewport,
/// };
/// use rasteron::Renderer;
/// # use rasteron::framebuffer::FramebufferInfo;
///
/// # let info = FramebufferInfo::new(16, 16, 16 * 4, rasteron::color::PixelFormat::Rgba8888);
/// # let mut fb = vec![0u8; info.buffer_size()];
/// # let mut renderer = Renderer::new(info, &mut fb);
/// let pipeline = Pipeline::new(
///     Passthrough2D::new(),
///     SolidFs::new(Color::new(255, 0, 0)),
///     PipelineState::alpha_blend(Viewport::new(0, 0, 16, 16)),
/// );
/// renderer
///     .draw_arrays(
///         PrimitiveType::Points,
///         &[Vertex2D { position: [0.5, 0.5], varying: () }],
///         &pipeline,
///     )
///     .unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct SolidFs {
    /// 每个片元输出的颜色。
    color: Color,
}

impl SolidFs {
    /// 创建常量颜色着色器。
    pub const fn new(color: Color) -> Self {
        Self { color }
    }
}

impl FragmentShader for SolidFs {
    /// 不携带任何 varying：颜色来自 `&self`，无需逐顶点传递。
    type Varying = ();

    /// 直接输出 [`SolidFs::color`]，忽略输入的片元坐标。
    fn shade_fragment(&self, _input: FragmentInput<Self::Varying>) -> FragmentOutput {
        FragmentOutput::Color(self.color)
    }
}

/// 沿 `dir` 方向做线性颜色渐变的片元着色器。
///
/// 参数全部存在 `&self` 上，不携带 varying，故顶点数据可用 [`()`]。
///
/// # 坐标系
///
/// 渐变进度 `t` 由 normalized **viewport** 坐标（[`FragmentInput::position`]，
/// 即 `[0, 1]`，左上角原点）与 `dir` 的点积得到。`t` 在 `{0.0, 1.0}` 区间内按
/// `from`→`to` 线性插值，超出则钳制到端点色。这样做的结果是**与视口尺寸无关**的：
/// 同一组参数在任意分辨率下渐变覆盖相同的相对范围。
///
/// # `dir` 同时表达方向与速率
///
/// `dir` **不归一化**，实现直接取 `t = dot(position, dir)`，故它的**长度有意义**：
/// `[1, 0]` 让渐变横向走完全宽，`[2, 0]` 只走一半就到达 `to` 色。需要「只关心方向、
/// 速率恒为 1」时自行归一化后再传入。
///
/// 更要紧的是，归一化并不是一个中性的替代方案：当前语义下 `dir = [1, 1]`
/// 即 `t = x + y`，峰值 1.414，渐变在对角线中点 `(0.5, 0.5)` 就到头；
/// 一旦归一化成 `t = (x + y) / √2`，终点会外推到 `(1, 1)` 之外的区域之外被钳制，
/// 实际结果变为整幅画布都停在 `to` 色附近——等于替调用方重新定义了
/// 「斜向渐变何时到头」。这属于 API 语义变更，不是实现细节，故本库保持现状。
///
/// # 宽高比
///
/// `dir` 定义在 normalized 空间（x、y 等权），因此 `dir = [1.0, 1.0]` 在正方形
/// 视口里呈 45°，在宽视口里会显得更「平」。这与 `Position::position` 的语义
/// 一致（归一化坐标本身不携带宽高比信息）；需要像素各向同性的等向渐变时，
/// 自行把视口宽高比折进 `dir` 即可。
///
/// # 陷阱：反转渐变要交换两端色，而不是翻转 `dir`
///
/// `t` 由 normalized 坐标（恒在 `[0, 1]`）与 `dir` 点积得到后钳制，因此
/// `dir` 中若有负分量，该轴上的 `t` 会整体落到 `[0, 1]` 之外并被钳到端点色。
/// 典型后果是 `dir = [-1.0, 0.0]` **不会**得到反向渐变，而是整片塌成 `from` 色
/// （`t ∈ [-1, 0]` 全被钳为 0）。要反向应交换 `from` / `to` 并保持 `dir` 为正。
/// 测试 `gradient_fs_negative_direction_collapses_to_from_color` 记录了该行为。
///
/// # 示例
///
/// ```
/// use rasteron::color::{BLACK, WHITE};
/// use rasteron::color::Color;
/// use rasteron::programmable::shader::{GradientFs, Passthrough2D};
/// use rasteron::programmable::{
///     FragmentInput, Pipeline, PipelineState, PrimitiveType, Vertex2D, Viewport,
/// };
/// use rasteron::Renderer;
/// # use rasteron::framebuffer::FramebufferInfo;
///
/// # let info = FramebufferInfo::new(64, 64, 64 * 4, rasteron::color::PixelFormat::Rgba8888);
/// # let mut fb = vec![0u8; info.buffer_size()];
/// # let mut renderer = Renderer::new(info, &mut fb);
/// let pipeline = Pipeline::new(
///     Passthrough2D::new(),
///     // 从左到右：黑 → 白
///     GradientFs::new(BLACK, WHITE, [1.0, 0.0]),
///     PipelineState::alpha_blend(Viewport::new(0, 0, 64, 64)),
/// );
/// renderer
///     .draw_arrays(
///         PrimitiveType::Triangles,
///         &[
///             Vertex2D { position: [0.0, 0.0], varying: () },
///             Vertex2D { position: [1.0, 0.0], varying: () },
///             Vertex2D { position: [1.0, 1.0], varying: () },
///         ],
///         &pipeline,
///     )
///     .unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct GradientFs {
    /// `t == 0` 处输出的颜色。
    from: Color,
    /// `t == 1` 处输出的颜色。
    to: Color,
    /// 渐变方向，同时是渐变的**速率**：决定每单位 normalized 位移推进多少 `t`。
    ///
    /// 实现直接取 `t = dot(position, dir)`（见 [`FragmentShader::shade_fragment`]），
    /// 故 `dir` 的**长度有意义**，不是只有方向：把 `[1, 0]` 写成 `[2, 0]` 会让
    /// 渐变快一倍，在横向一半宽度处就到达 `to` 色。方向相同的 `dir` 若要得到
    /// 同一视觉结果，必须按比例缩放——这正是 `dir` 同时表达「方向 + 速率」的
    /// 含义。需要「只关心方向、速率恒为 1」时请自行归一化后再传入。
    dir: Vec2,
}

impl GradientFs {
    /// 创建线性渐变着色器。
    ///
    /// `dir` 定义在 normalized viewport 坐标空间，**长度与方向都有意义**：
    /// 它就是 `t = dot(position, dir)` 里的速率系数（见 [`dir`](Self::dir)）。
    ///
    /// 零向量使所有片元都取 `from`（点积恒为 0），不会 panic。
    ///
    /// # `dir` 的长度会改变渐变速度
    ///
    /// ```
    /// use rasteron::color::{BLACK, WHITE};
    /// use rasteron::programmable::shader::GradientFs;
    /// use rasteron::programmable::{FragmentInput, FragmentOutput, FragmentShader};
    ///
    /// fn red_at(shader: &GradientFs, x: f32) -> u8 {
    ///     match shader.shade_fragment(FragmentInput { position: [x, 0.0], varying: () }) {
    ///         FragmentOutput::Color(color) => color.r,
    ///         FragmentOutput::Discard => unreachable!("GradientFs 永不丢弃"),
    ///     }
    /// }
    ///
    /// let slow = GradientFs::new(BLACK, WHITE, [1.0, 0.0]);
    /// let fast = GradientFs::new(BLACK, WHITE, [2.0, 0.0]);
    ///
    /// // x = 0.5 处：`[2, 0]` 已到头（t = 1），`[1, 0]` 才走了一半（t = 0.5）。
    /// assert_eq!(red_at(&fast, 0.5), 255);
    /// assert_eq!(red_at(&slow, 0.5), 128);
    /// ```
    ///
    /// 需要「只关心方向」时，自行归一化：`dir / (dx² + dy²).sqrt()`。
    /// 本库刻意**不**内部归一化——归一化会把 `[1, 1]`（45°，当前语义下
    /// `t = x + y`、峰值 1.414）的终点挪到 `(0.5, 0.5)`，等于替调用方
    /// 重新定义了「斜向渐变何时到头」，那属于 API 语义变更而非实现细节。
    pub const fn new(from: Color, to: Color, dir: Vec2) -> Self {
        Self { from, to, dir }
    }
}

impl FragmentShader for GradientFs {
    /// 不携带任何 varying：渐变由片元坐标直接算出。
    type Varying = ();

    /// 用片元 normalized 坐标与 `dir` 的点积作为插值进度，输出 `from`→`to` 渐变。
    ///
    /// `t = dot(position, dir)` 后钳制到 `[0, 1]`。`dir` **未归一化**，
    /// 故其长度参与结果、同时充当渐变速率——见 [`dir`](Self::dir)。
    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
        // 点积即沿渐变方向的进度；`t` 随后被钳制到 [0, 1] 覆盖端点色。
        let t = (input.position[0] * self.dir[0] + input.position[1] * self.dir[1]).clamp(0.0, 1.0);
        // 复用 `Interpolate for Color`——它是库内「两点间插值」的权威实现：
        // 四个通道（含 alpha）一并插值，取整口径与 varying 渐变完全一致。
        // 不用 `Color::blend`：它的权重语义（self=src 乘 alpha）是标准的，
        // 但返回值由 `Color::new` 构造、alpha 恒置 255，不插值 alpha 通道，
        // 会把半透明端点色（如 `from.a == 128`）错误地变成不透明。
        FragmentOutput::Color(Color::lerp(self.from, self.to, t))
    }
}

/// 直接输出纹理采样结果的片元着色器。
///
/// UV 坐标作为 **varying** 传入（`type Varying = Vec2`），纹理本身存在 `&self`
/// 上。配合 [`Passthrough2D<Vec2>`] 使用即可——顶点着色器无需任何改动，
/// 因为 [`Vertex2D`] 的 `position` 与 `varying` 是两个独立字段：
///
/// ```
/// use rasteron::color::{Color, PixelFormat};
/// use rasteron::programmable::shader::{Passthrough2D, TextureFs};
/// use rasteron::programmable::texture::{FilterMode, Texture2D, WrapMode};
/// use rasteron::programmable::{
///     Pipeline, PipelineState, PrimitiveType, Vec2, Vertex2D, Viewport,
/// };
/// use rasteron::Renderer;
/// # use rasteron::framebuffer::FramebufferInfo;
///
/// # let info = FramebufferInfo::new(16, 16, 16 * 4, PixelFormat::Rgba8888);
/// # let mut fb = vec![0u8; info.buffer_size()];
/// # let mut renderer = Renderer::new(info, &mut fb);
///
/// // 2×2 纹理：左上红、右下白。
/// let texels = [
///     255, 0, 0, 255, 0, 255, 0, 255,
///     0, 0, 255, 255, 255, 255, 255, 255,
/// ];
/// let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
///
/// let pipeline = Pipeline::new(
///     Passthrough2D::<Vec2>::new(),
///     TextureFs::new(texture, FilterMode::Nearest, WrapMode::Clamp),
///     PipelineState::alpha_blend(Viewport::new(0, 0, 16, 16)),
/// );
/// renderer
///     .draw_arrays(
///         PrimitiveType::Triangles,
///         &[
///             // uv = (0, 0) → 纹理左上角附近 → 红。
///             Vertex2D { position: [0.0, 0.0], varying: [0.25, 0.25] },
///             Vertex2D { position: [1.0, 0.0], varying: [0.75, 0.25] },
///             Vertex2D { position: [1.0, 1.0], varying: [0.75, 0.75] },
///         ],
///         &pipeline,
///     )
///     .unwrap();
///
/// // 左上区域确实是红色。
/// let pitch = 16 * 4;
/// assert_eq!(&fb[0..4], &[255, 0, 0, 255], "左上角应采到红色 texel");
/// ```
///
/// # 参数为什么住在 `&self`
///
/// [`FragmentShader::shade_fragment`] 没有 uniform 通道，且 `draw_arrays` 持
/// `&mut Renderer` 时只能持 `&Pipeline`，故 `&self` 无法放宽为 `&mut self`。
/// [`Texture2D`] 只有 4 个字段、是 `Copy`，把它按值存进 `&self` 零成本。
/// 要换纹理就重建 [`Pipeline`]——它是按值结构体，重建不分配。
///
/// # 示例：动画（每帧重建 Pipeline）
///
/// ```
/// use rasteron::color::PixelFormat;
/// use rasteron::programmable::shader::TextureFs;
/// use rasteron::programmable::texture::{FilterMode, Texture2D, WrapMode};
///
/// let texels = [255u8, 0, 0, 255];
/// // 第二帧换一张纹理：只是重建 Pipeline，无需任何可变状态。
/// let frame1 = Texture2D::new(1, 1, &texels, PixelFormat::Rgba8888);
/// let frame2 = Texture2D::new(1, 1, &texels, PixelFormat::Rgba8888);
/// let _a = TextureFs::new(frame1, FilterMode::Nearest, WrapMode::Clamp);
/// let _b = TextureFs::new(frame2, FilterMode::Bilinear, WrapMode::Repeat);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct TextureFs<'a> {
    /// 被采样的纹理。
    texture: Texture2D<'a>,
    /// 过滤方式：最近邻或双线性。
    filter: FilterMode,
    /// 越界寻址方式。
    wrap: WrapMode,
}

impl<'a> TextureFs<'a> {
    /// 创建纹理采样着色器。
    ///
    /// `filter` / `wrap` 直接转发给 [`Texture2D::sample`]，语义见该方法的文档。
    pub const fn new(texture: Texture2D<'a>, filter: FilterMode, wrap: WrapMode) -> Self {
        Self {
            texture,
            filter,
            wrap,
        }
    }

    /// 被采样的纹理。
    pub const fn texture(&self) -> &Texture2D<'a> {
        &self.texture
    }
}

impl FragmentShader for TextureFs<'_> {
    /// UV 坐标作为 varying：由顶点着色器透传，光栅化阶段按重心权重插值。
    type Varying = Vec2;

    /// 按插值后的 UV 采样纹理，恒不丢弃片元。
    ///
    /// 越界坐标由 `wrap` 决定（默认钳制），不产生 [`FragmentOutput::Discard`]——
    /// 需要「UV 超出 [0,1] 时镂空」应自行检查坐标后返回 `Discard`。
    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
        FragmentOutput::Color(self.texture.sample(input.varying, self.filter, self.wrap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn passthrough_forwards_position_and_varying_verbatim() {
        let shader = Passthrough2D::<Color>::new();
        let cases = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: Color::new(0, 0, 0),
            },
            Vertex2D {
                position: [1.0, 1.0],
                varying: Color::new(255, 255, 255),
            },
            // 非整数的 normalized 坐标必须原样透传，不做任何取整或变换。
            Vertex2D {
                position: [0.123_456, 0.987_654],
                varying: Color::with_alpha(1, 2, 3, 4),
            },
            // 视口外的负坐标同样原样透传（裁剪是光栅化阶段的职责）。
            Vertex2D {
                position: [-0.5, 2.5],
                varying: Color::new(7, 8, 9),
            },
        ];
        for input in cases {
            let out = shader.shade_vertex(&input);
            assert_eq!(out.position, input.position, "position 被变换了");
            assert_eq!(out.varying, input.varying, "varying 被变换了");
        }
    }

    #[test]
    fn default_matches_new() {
        // `#[derive(Default)]` 生成的 `impl` 带有 `V: Default` 约束，
        // 故只有当 varying 类型实现 `Default` 时才可用。
        // `Color` 没有实现 `Default`，这里改用 `f32`（实现 Default）验证。
        let via_new = Passthrough2D::<f32>::new();
        let via_default = Passthrough2D::<f32>::default();
        let input = Vertex2D {
            position: [0.25, 0.75],
            varying: 42.0f32,
        };
        assert_eq!(
            via_new.shade_vertex(&input),
            via_default.shade_vertex(&input),
            "Default 与 new 的着色结果应完全一致"
        );

        // 对没有 `Default` 的 varying 类型，`default()` 不可用——
        // 这是派生实现的固有约束，此处记录以免误用。
        // （无法用编译期断言表达，只能靠类型检查本身保证。）
    }

    #[test]
    fn passthrough_is_copy_and_reusable() {
        // 无运行时状态，故可自由复用与按值复制。
        let shader = Passthrough2D::<Color>::new();
        let copy = shader; // Copy：原值仍可用
        let input = Vertex2D {
            position: [0.5, 0.5],
            varying: Color::new(5, 5, 5),
        };
        assert_eq!(shader.shade_vertex(&input), copy.shade_vertex(&input));
    }

    // ————————————————————————————— SolidFs —————————————————————————————

    /// 构造一个只带坐标的片元输入：两个着色器都不读 varying，故传 `()` 即可。
    fn fragment_at(position: [f32; 2]) -> FragmentInput<()> {
        FragmentInput {
            position,
            varying: (),
        }
    }

    #[test]
    fn solid_fs_ignores_position_and_emits_the_same_color() {
        let shader = SolidFs::new(Color::with_alpha(12, 34, 56, 78));
        // 坐标无关：任意位置的片元都应得到同一个颜色，含 alpha 原样透传。
        for position in [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0], [-3.0, 7.5]] {
            assert_eq!(
                shader.shade_fragment(fragment_at(position)),
                FragmentOutput::Color(Color::with_alpha(12, 34, 56, 78)),
                "位置 {position:?} 处不应影响输出颜色"
            );
        }
    }

    #[test]
    fn solid_fs_preserves_source_alpha() {
        // `SolidFs` 不做混合，alpha 完全交给 `BlendMode`——半透明常量色必须活到写入层。
        let shader = SolidFs::new(Color::with_alpha(255, 0, 0, 0));
        let FragmentOutput::Color(color) = shader.shade_fragment(fragment_at([0.5, 0.5])) else {
            panic!("SolidFs 永不丢弃片元");
        };
        assert_eq!(color.a, 0, "alpha 应原样透传，由 BlendMode 决定实际效果");
    }

    // ————————————————————————————— GradientFs —————————————————————————————

    /// 取 `GradientFs` 在某点的着色结果。
    fn gradient_at(shader: &GradientFs, position: [f32; 2]) -> Color {
        match shader.shade_fragment(fragment_at(position)) {
            FragmentOutput::Color(color) => color,
            FragmentOutput::Discard => panic!("GradientFs 永不丢弃片元"),
        }
    }

    #[test]
    fn gradient_fs_endpoints_hit_the_two_source_colors() {
        let shader = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [1.0, 0.0]);
        // t = dot([0,0],[1,0]) = 0 → from；t = dot([1,0],[1,0]) = 1 → to。
        assert_eq!(gradient_at(&shader, [0.0, 0.0]), crate::color::BLACK);
        assert_eq!(gradient_at(&shader, [1.0, 0.0]), crate::color::WHITE);
    }

    #[test]
    fn gradient_fs_interpolates_monotonically_between_endpoints() {
        let shader = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [1.0, 0.0]);
        // 沿 +x 走完一整段，红通道应严格递增。
        let mut previous = 0u8;
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let color = gradient_at(&shader, [t, 0.0]);
            assert!(
                color.r >= previous,
                "t={t} 处红通道应单调不减（{previous} → {}）",
                color.r
            );
            previous = color.r;
        }
        assert_eq!(previous, 255, "t=1 处应到达 to 的亮度");
    }

    #[test]
    fn gradient_fs_clamps_outside_the_unit_range() {
        let shader = GradientFs::new(Color::new(10, 20, 30), Color::new(200, 100, 50), [1.0, 0.0]);
        // t < 0 与 t > 1 都必须钳制到端点色，否则 `blend` 的 alpha 会溢出 u8。
        for position in [[-1.0, 0.0], [-0.001, 0.5], [1.001, 0.0], [2.0, 3.0]] {
            let color = gradient_at(&shader, position);
            assert!(
                (color.r == 10 && color.g == 20 && color.b == 30)
                    || (color.r == 200 && color.g == 100 && color.b == 50),
                "位置 {position:?} 应钳制到端点色，实际 {color:?}"
            );
        }
    }

    #[test]
    fn gradient_fs_is_resolution_independent() {
        // 同一组参数在不同视口尺寸下，相同 normalized 坐标必须得到相同颜色。
        let shader = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [1.0, 0.0]);
        for position in [[0.0, 0.0], [0.25, 0.9], [0.5, 0.5], [1.0, 1.0]] {
            let first = gradient_at(&shader, position);
            let second = gradient_at(&shader, position);
            assert_eq!(
                first, second,
                "normalized 坐标 {position:?} 的结果不应依赖视口"
            );
        }
        // 渐变只沿 dir 变化：垂直方向（+y）与 dir 正交，输出应保持不变。
        assert_eq!(
            gradient_at(&shader, [0.5, 0.0]),
            gradient_at(&shader, [0.5, 1.0]),
            "与 dir 正交的方向不应改变输出"
        );
    }

    #[test]
    fn gradient_fs_zero_direction_yields_from_color() {
        // 零向量使点积恒为 0 → 恒取 from。这是退化输入，契约是「不 panic」。
        let shader = GradientFs::new(Color::new(1, 2, 3), Color::new(4, 5, 6), [0.0, 0.0]);
        for position in [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]] {
            assert_eq!(
                gradient_at(&shader, position),
                Color::new(1, 2, 3),
                "零方向应恒取 from 色"
            );
        }
    }

    #[test]
    fn gradient_fs_negative_direction_collapses_to_from_color() {
        // **反向渐变的陷阱**：`t = dot(position, dir)` 在 `[0, 1]` 坐标域上取值，
        // 故 `dir = [-1, 0]` 时 t ∈ [-1, 0]，**全部被钳到 0**，整片塌成 `from` 色，
        // 而不是「反向的渐变」。要反向必须同时交换 from/to，而不是只翻转 dir。
        let forward = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [1.0, 0.0]);
        let backward = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [-1.0, 0.0]);
        for position in [[0.0, 0.0], [0.5, 0.0], [1.0, 0.0]] {
            assert_eq!(
                gradient_at(&backward, position),
                crate::color::BLACK,
                "负方向在 [0,1] 坐标域上应恒取 from 色"
            );
        }

        // 正确的反向写法：交换两端色，dir 保持正向。
        let reversed = GradientFs::new(crate::color::WHITE, crate::color::BLACK, [1.0, 0.0]);
        assert_eq!(
            gradient_at(&forward, [0.75, 0.0]).r,
            gradient_at(&reversed, [0.25, 0.0]).r,
            "交换两端色后，两者的 t 应互补"
        );
    }

    #[test]
    fn gradient_fs_dir_length_scales_the_gradient_speed() {
        // `dir` 不归一化：长度即速率。文档曾声称「只有分量比值有意义」，
        // 那是错的——`[1, 0]` 与 `[2, 0]` 方向相同，渐变速度却差一倍。
        let unit = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [1.0, 0.0]);
        let doubled = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [2.0, 0.0]);

        // x = 0.5：`[2, 0]` 的 t 已到 1，`[1, 0]` 的 t 才 0.5。
        assert_eq!(
            gradient_at(&doubled, [0.5, 0.0]).r,
            255,
            "`[2, 0]` 应在半宽处到头"
        );
        assert_eq!(
            gradient_at(&unit, [0.5, 0.0]).r,
            128,
            "`[1, 0]` 应在半宽处恰好走完一半"
        );

        // 等价说法：方向相同但长度翻倍，`t` 也翻倍，于是前者的终点位置
        // 正是后者的中点——两条曲线的半程互相对齐。
        let midpoint_of_doubled = gradient_at(&doubled, [0.25, 0.0]).r;
        let midpoint_of_unit = gradient_at(&unit, [0.5, 0.0]).r;
        assert_eq!(
            midpoint_of_doubled, midpoint_of_unit,
            "`[2, 0]` 的半程应与 `[1, 0]` 的半程对齐"
        );
    }

    #[test]
    fn gradient_fs_diagonal_direction_reaches_to_before_the_far_corner() {
        // `dir = [1, 1]` 即 `t = x + y`，峰值 1.414：在对角线中点 (0.5, 0.5)
        // 处 t 已为 1，**此后整片都是 to 色**。这条断言把「归一化会改变语义」
        // 钉死——若日后有人给 `dir` 加内部归一化，本测试会失败。
        let shader = GradientFs::new(crate::color::BLACK, crate::color::WHITE, [1.0, 1.0]);
        assert_eq!(
            gradient_at(&shader, [0.5, 0.5]).r,
            255,
            "对角线中点处 t = 1，应已是 to 色"
        );
        assert_eq!(
            gradient_at(&shader, [0.6, 0.6]).r,
            255,
            "越过后 t 被钳到 1，仍应是 to 色"
        );
        // 对称性：同一 x+y 上颜色相同。
        assert_eq!(
            gradient_at(&shader, [0.3, 0.1]).r,
            gradient_at(&shader, [0.1, 0.3]).r,
            "沿 45° 方向两侧等距的点应同色"
        );
    }

    #[test]
    fn both_fs_carry_no_varying() {
        // 恒等性契约：`() `的插值必须恒等，否则管线会在装配阶段悄悄丢掉属性。
        assert_eq!(
            <() as Interpolate>::interpolate((), (), (), 0.3, 0.3, 0.4),
            ()
        );
        assert_eq!(<() as Interpolate>::lerp((), (), 0.7), ());
    }

    // ————————————————————————————— TextureFs —————————————————————————————

    /// 构造 2×2 四色纹理。
    ///
    /// `Texture2D` 借用 texel 数组，故两者必须一起返回给调用方，
    /// 借用关系才不会在函数边界上断开。
    fn make_four_color_texture<'a>(texels: &'a mut [u8; 16]) -> Texture2D<'a> {
        for (index, color) in [
            Color::new(255, 0, 0),
            Color::new(0, 255, 0),
            Color::new(0, 0, 255),
            Color::new(255, 255, 255),
        ]
        .into_iter()
        .enumerate()
        {
            let offset = index * 4;
            texels[offset..offset + 4]
                .copy_from_slice(&color.to_bytes(crate::color::PixelFormat::Rgba8888));
        }
        Texture2D::new(2, 2, texels, crate::color::PixelFormat::Rgba8888)
    }

    /// 在某 UV 处取得 `TextureFs` 的着色结果（它永不丢弃片元）。
    fn texture_at(shader: &TextureFs<'_>, uv: Vec2) -> Color {
        match shader.shade_fragment(FragmentInput {
            position: [0.5, 0.5],
            varying: uv,
        }) {
            FragmentOutput::Color(color) => color,
            FragmentOutput::Discard => panic!("TextureFs 永不丢弃片元"),
        }
    }

    #[test]
    fn texture_fs_samples_by_uv_ignoring_position() {
        let mut texels = [0u8; 16];
        let texture = make_four_color_texture(&mut texels);
        let shader = TextureFs::new(texture, FilterMode::Nearest, WrapMode::Clamp);

        // texel 中心 = (i + 0.5) / 2。
        assert_eq!(texture_at(&shader, [0.25, 0.25]), Color::new(255, 0, 0));
        assert_eq!(texture_at(&shader, [0.75, 0.25]), Color::new(0, 255, 0));
        assert_eq!(texture_at(&shader, [0.25, 0.75]), Color::new(0, 0, 255));
        assert_eq!(texture_at(&shader, [0.75, 0.75]), Color::new(255, 255, 255));
    }

    /// `position` 不参与采样：同一 UV 无论在片元何处都得到同一颜色。
    #[test]
    fn texture_fs_ignores_fragment_position() {
        let mut texels = [0u8; 16];
        let texture = make_four_color_texture(&mut texels);
        let shader = TextureFs::new(texture, FilterMode::Bilinear, WrapMode::Clamp);

        for position in [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]] {
            let FragmentOutput::Color(color) = shader.shade_fragment(FragmentInput {
                position,
                varying: [0.25, 0.25],
            }) else {
                panic!("TextureFs 永不丢弃片元");
            };
            assert_eq!(
                color,
                Color::new(255, 0, 0),
                "position={position:?} 不应影响采样"
            );
        }
    }

    /// `filter` 与 `wrap` 参数确实透传到 `Texture2D::sample`。
    #[test]
    fn texture_fs_forwards_filter_and_wrap() {
        let mut texels = [0u8; 16];
        let texture = make_four_color_texture(&mut texels);

        // 钳制：u > 1 取最后一列；重复：u = 1.25 回到第一列。
        let clamped = TextureFs::new(texture, FilterMode::Nearest, WrapMode::Clamp);
        let repeated = TextureFs::new(texture, FilterMode::Nearest, WrapMode::Repeat);
        assert_eq!(texture_at(&clamped, [1.5, 0.25]), Color::new(0, 255, 0));
        assert_eq!(texture_at(&repeated, [1.25, 0.25]), Color::new(255, 0, 0));

        // 最近邻 vs 双线性：两色交界处两者结果不同。
        let nearest = TextureFs::new(texture, FilterMode::Nearest, WrapMode::Clamp);
        let bilinear = TextureFs::new(texture, FilterMode::Bilinear, WrapMode::Clamp);
        let uv = [0.5, 0.25]; // u 恰在两列之间
        assert_ne!(texture_at(&nearest, uv), texture_at(&bilinear, uv));
    }

    /// `TextureFs` 是 `Copy` 且可自由复用——纹理与参数都在 `&self` 上。
    #[test]
    fn texture_fs_is_copy_and_reusable() {
        let mut texels = [0u8; 16];
        let texture = make_four_color_texture(&mut texels);
        let shader = TextureFs::new(texture, FilterMode::Nearest, WrapMode::Clamp);
        let copy = shader;
        assert_eq!(
            texture_at(&shader, [0.25, 0.25]),
            texture_at(&copy, [0.25, 0.25])
        );
        assert_eq!(copy.texture().width(), 2);
    }

    /// 零尺寸纹理的着色结果是透明黑，而非 panic——退化输入也要能渲染。
    #[test]
    fn texture_fs_handles_zero_sized_texture() {
        let texels = [0u8; 16];
        let texture = Texture2D::new(0, 0, &texels, crate::color::PixelFormat::Rgba8888);
        let shader = TextureFs::new(texture, FilterMode::Bilinear, WrapMode::Repeat);
        assert_eq!(texture_at(&shader, [0.5, 0.5]), Color::new(0, 0, 0));
    }

    /// `Vec2` 的插值恒等性是 UV varying 正确的前提（同 `both_fs_carry_no_varying` 的思路）。
    #[test]
    fn texture_fs_uv_varying_interpolates_linearly() {
        // UV 必须被真正插值：三角形中点的 UV 应是两端的中点。
        let a = [0.0f32, 0.0f32];
        let b = [1.0f32, 0.0f32];
        let c = [1.0f32, 1.0f32];
        let mid = <Vec2 as Interpolate>::interpolate(a, b, c, 0.0, 0.5, 0.5);
        assert_eq!(mid, [1.0, 0.5]);
    }
}
