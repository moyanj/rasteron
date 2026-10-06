//! 2D 纹理与采样。
//!
//! 纹理是**零分配**的：只借用调用方的 `&'a [u8]` 与一份尺寸/格式元数据，
//! 与 [`crate::core::Renderer`] 对帧缓冲的处理方式一致。
//!
//! # 坐标约定
//!
//! 采样走 **半 texel 约定**（与 OpenGL / Vulkan / D3D 一致）：
//!
//! ```text
//! x = u × width  − 0.5      y = v × height − 0.5
//! ```
//!
//! 即 texel `i` 的**中心**落在 `u = (i + 0.5) / width` 上。推论：
//!
//! - `uv = [0.0, 0.0]` 落在 texel 0 的**左边缘**（`x = -0.5`），不是中心；
//! - `uv = [1.0, 1.0]` 落在 texel `width - 1` 的**右边缘**（`x = width - 0.5`）。
//!
//! 因此 `u = 0.5 / width` 才采到 texel 0 的中心。半 texel 偏移是消除边缘接缝的
//! 必要条件：没有它，`u = 1.0` 会落到 texel `width`（越界），`u = 0.0` 与
//! `u = 1.0 / width` 会采到同一个 texel，整幅图在纹理边界处缩进半像素。
//!
//! # 陷阱：与视口变换同族的「边缘空间」决策
//!
//! 本约定与 `programmable` 模块记录的「视口变换是边缘空间、跨度乘全宽」
//! （见 [`crate::utils::normalized_to_screen`]）方向一致：`1.0` 对应的是
//! **边界线**而非最后一个像素的中心。但两者不可混用——视口映射的输入是
//! normalized 屏幕坐标，而纹理映射的输入是 varying UV，两者独立计算。
//!
//! # 示例
//!
//! ```
//! use rasteron::color::{Color, PixelFormat};
//! use rasteron::programmable::texture::{FilterMode, Texture2D, WrapMode};
//!
//! // 2×2 的 RGBA8 纹理：左上红、右上绿、左下蓝、右下白。
//! let texels = [
//!     255, 0, 0, 255, // texel (0, 0)
//!     0, 255, 0, 255, // texel (1, 0)
//!     0, 0, 255, 255, // texel (0, 1)
//!     255, 255, 255, 255, // texel (1, 1)
//! ];
//! let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
//!
//! // texel 中心的 u/v 是 (i + 0.5) / size。
//! let sample = |uv| texture.sample(uv, FilterMode::Nearest, WrapMode::Clamp);
//! assert_eq!(sample([0.25, 0.25]), Color::new(255, 0, 0));
//! assert_eq!(sample([0.75, 0.75]), Color::new(255, 255, 255));
//! ```

use crate::color::{Color, PixelFormat};
use crate::error::TextureError;
use crate::programmable::Vec2;

/// 采样坐标超出纹理范围时的寻址方式。
///
/// 作用于单轴；`u` 与 `v` 各自独立寻址。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WrapMode {
    /// 钳制到边缘：超出范围的坐标取最靠近的边界 texel。
    ///
    /// 这是 [`sample_nearest`] / [`sample_bilinear`] 的默认行为，也是精灵图
    /// （sprite sheet）这类「不该把对侧像素渗进来」的场景唯一正确的选择。
    Clamp,
    /// 重复平铺：坐标对 `size` 取模后落在 `0..size`。
    ///
    /// 负坐标同样有效——用 `rem_euclid` 而非 `%`，后者会返回负余数。
    Repeat,
}

/// 采样时的过滤方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterMode {
    /// 取最近的一个 texel，不做插值。
    ///
    /// 放大（纹理小于绘制区域）时产生马赛克，缩小到 1 texel 以下时会丢细节，
    /// 但结果永远精确落在原始 texel 值上（无插值误差）。
    Nearest,
    /// 四点双线性插值。
    ///
    /// 放大时平滑，缩小时**不做 mipmap 过滤**（库内零分配，见模块文档），
    /// 因此对高倍率缩小仍可能出现闪烁与走样。
    Bilinear,
}

/// 2D 纹理：借用调用方字节切片的只读图像。
///
/// # 字段布局
///
/// texel **紧凑排列**，无行填充：`row_stride == width × bytes_per_pixel`。
/// 与 [`crate::framebuffer::FramebufferInfo`] 的 `pitch` 不同，纹理不支持行对齐；
/// 若你的数据源是带 pitch 的帧缓冲切片，需自行逐行拷贝或改用
/// [`Texture2D::from_rows`]。
///
/// # 零尺寸纹理
///
/// `width == 0` 或 `height == 0` 的纹理**允许构造**，但任何采样恒返回
/// `Color::new(0, 0, 0)`（透明黑）。见 [`sample`]。
///
/// # 示例
///
/// ```
/// use rasteron::color::PixelFormat;
/// use rasteron::programmable::texture::Texture2D;
///
/// let texels = [0u8; 2 * 2 * 4];
/// let texture = Texture2D::try_new(2, 2, &texels, PixelFormat::Rgba8888).unwrap();
/// assert_eq!(texture.width(), 2);
/// assert_eq!(texture.height(), 2);
/// assert_eq!(texture.format(), PixelFormat::Rgba8888);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Texture2D<'a> {
    /// 可见宽度（texel 数）。
    width: usize,
    /// 可见高度（texel 数）。
    height: usize,
    /// texel 字节数据，紧凑排列。
    texels: &'a [u8],
    /// texel 的像素格式。
    format: PixelFormat,
}

impl<'a> Texture2D<'a> {
    /// 创建纹理并校验不变量。
    ///
    /// 返回 [`TextureError::LengthMismatch`] 当 `texels` 长度**小于**
    /// `width × height × bytes_per_pixel`。尾部允许多余字节（便于直接借用
    /// 带 pitch 的帧缓冲切片的整个尾部），此时只有前 `width × height × bpp`
    /// 个字节参与采样。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::PixelFormat;
    /// use rasteron::error::TextureError;
    /// use rasteron::programmable::texture::Texture2D;
    ///
    /// let texels = [0u8; 16];
    /// assert!(Texture2D::try_new(2, 2, &texels, PixelFormat::Rgba8888).is_ok());
    /// // 4 个 texel × 4 字节 = 16，给 8 字节不够。
    /// assert_eq!(
    ///     Texture2D::try_new(2, 2, &texels[..8], PixelFormat::Rgba8888).unwrap_err(),
    ///     TextureError::LengthMismatch
    /// );
    /// ```
    pub fn try_new(
        width: usize,
        height: usize,
        texels: &'a [u8],
        format: PixelFormat,
    ) -> Result<Self, TextureError> {
        // 两次 checked_mul 依次排除 `w * h` 与 `(w * h) * bpp` 的溢出。
        // 溢出即 DimensionOverflow，不能让它绕回一个小值后通过长度检查。
        let required = width
            .checked_mul(height)
            .ok_or(TextureError::DimensionOverflow)?
            .checked_mul(format.bytes_per_pixel())
            .ok_or(TextureError::DimensionOverflow)?;
        if texels.len() < required {
            return Err(TextureError::LengthMismatch);
        }
        Ok(Self {
            width,
            height,
            texels,
            format,
        })
    }

    /// 创建纹理；不变量被破坏时**恐慌**。
    ///
    /// 这是构造期断言入口，与 [`crate::framebuffer::FramebufferInfo::new`] 的
    /// 分层一致：尺寸与调用方缓冲区不匹配属于**程序错误**，应在最早处暴露，
    /// 而不是让每次采样各自降级。需要优雅处理时用 [`Texture2D::try_new`]。
    ///
    /// # 恐慌
    ///
    /// 当 `texels` 长度小于 `width × height × bytes_per_pixel`，或该乘积溢出
    /// `usize` 时恐慌。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::PixelFormat;
    /// use rasteron::programmable::texture::Texture2D;
    ///
    /// let texels = [0u8; 16];
    /// let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
    /// assert_eq!(texture.width(), 2);
    /// ```
    #[track_caller]
    pub fn new(width: usize, height: usize, texels: &'a [u8], format: PixelFormat) -> Self {
        match Self::try_new(width, height, texels, format) {
            Ok(texture) => texture,
            Err(error) => panic!("{error}"),
        }
    }

    /// 从带行填充的切片逐行拷贝出紧凑纹理。
    ///
    /// 用于纹理数据源是帧缓冲切片（`row_stride > width × bytes_per_pixel`）
    /// 的情形——`Texture2D` 本身只支持紧凑排列。
    ///
    /// `destination` 需至少 `width × height × bytes_per_pixel` 字节；多余部分不写入。
    /// 逐行拷贝用 [`copy_from_slice`]，越界长度在库内 panic（属调用方程序错误）。
    ///
    /// # 参数
    ///
    /// * `source` — 源数据，至少 `row_stride × height` 字节。
    /// * `row_stride` — 源数据每行的字节跨度（≥ `width × bytes_per_pixel`）。
    /// * `destination` — 输出缓冲区。
    ///
    /// # 恐慌
    ///
    /// `destination` 太短、`source` 太短、`row_stride < width × bytes_per_pixel`，
    /// 或下列任一乘积溢出 `usize` 时恐慌：`width × bytes_per_pixel`、
    /// `width × height × bytes_per_pixel`、`row_stride × height`。
    ///
    /// 三处乘法**都是** `checked_mul`：行填充使 `row_stride` 可以远大于
    /// `width × bytes_per_pixel`，故 `row_stride × height` 是否溢出与前两者相互独立，
    /// 少查一处就会让 release 模式回绕成小值、错误地接受过短的 `source`，
    /// 直到逐行拷贝时才越界 panic。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::{Color, PixelFormat};
    /// use rasteron::programmable::texture::{FilterMode, Texture2D, WrapMode};
    ///
    /// // 源是 1 行宽 2 texel、但按 4 texel 对齐（stride 16）的帧缓冲行。
    /// let mut source = [0u8; 16];
    /// source[0..4].copy_from_slice(&[255, 0, 0, 255]);
    /// source[4..8].copy_from_slice(&[0, 255, 0, 255]);
    /// // 第 8..16 字节是行填充，不得进入纹理。
    /// source[8..12].copy_from_slice(&[0, 0, 0, 255]);
    ///
    /// let mut compact = [0u8; 8];
    /// Texture2D::from_rows(2, 1, 16, &source, PixelFormat::Rgba8888, &mut compact);
    ///
    /// let texture = Texture2D::new(2, 1, &compact, PixelFormat::Rgba8888);
    /// let sample = |uv| texture.sample(uv, FilterMode::Nearest, WrapMode::Clamp);
    /// assert_eq!(sample([0.25, 0.5]), Color::new(255, 0, 0));
    /// assert_eq!(sample([0.75, 0.5]), Color::new(0, 255, 0));
    /// ```
    pub fn from_rows(
        width: usize,
        height: usize,
        row_stride: usize,
        source: &[u8],
        format: PixelFormat,
        destination: &mut [u8],
    ) {
        let row_bytes = width
            .checked_mul(format.bytes_per_pixel())
            .expect("width × bytes_per_pixel 溢出 usize");
        let required = row_bytes
            .checked_mul(height)
            .expect("width × height × bytes_per_pixel 溢出 usize");
        // 源跨度同理必须 checked_mul：上一行的 `required` 不含 `row_stride`
        // 的行填充，故它不溢出**不代表** `row_stride × height` 不溢出。
        // 未检查时 release 模式下回绕成小值 → 错误地接受过短的 source →
        // 逐行拷贝时才在 `source[src_start..]` 上越界 panic，且 panic 点远离
        // 出错的参数。debug 模式下的 "attempt to multiply with overflow"
        // 只是插桩期的偶然拦截，不构成契约。
        let source_required = row_stride
            .checked_mul(height)
            .expect("row_stride × height 溢出 usize");

        assert!(
            destination.len() >= required,
            "destination 长度须至少为 width × height × bytes_per_pixel"
        );
        assert!(
            row_stride >= row_bytes,
            "row_stride 须至少为 width × bytes_per_pixel"
        );
        assert!(
            source.len() >= source_required,
            "source 长度须至少为 row_stride × height"
        );

        for row in 0..height {
            // 两个乘积都已证明不溢出，这里再乘不会 panic。
            let src_start = row * row_stride;
            let dst_start = row * row_bytes;
            destination[dst_start..dst_start + row_bytes]
                .copy_from_slice(&source[src_start..src_start + row_bytes]);
        }
    }

    /// 可见宽度（texel 数）。
    pub const fn width(&self) -> usize {
        self.width
    }

    /// 可见高度（texel 数）。
    pub const fn height(&self) -> usize {
        self.height
    }

    /// 借用的 texel 字节数据。
    ///
    /// 长度可能大于 `width × height × bytes_per_pixel`（尾部多余字节被忽略）。
    pub const fn texels(&self) -> &'a [u8] {
        self.texels
    }

    /// texel 的像素格式。
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// 单轴寻址：把任意整数坐标映射到 `0..limit`。
    ///
    /// `limit == 0`（零尺寸纹理）恒返回 0 —— 采样入口已在更早处短路成透明黑，
    /// 这里只是保证本函数**自身**不会 panic 的防御。
    ///
    /// # 为什么在 `i64` 域算，而不是 `limit as i32`
    ///
    /// `as i32` 是**回绕**而非饱和：`limit = i32::MAX + 100` 会变成负数，
    /// 从而让 `Clamp` 的上界错误，或让 `Repeat` 使用错误的模数。这里统一
    /// 转到 `i64` 域，避免大纹理尺寸在寻址时窄化回绕。
    ///
    /// 改在 `i64` 域计算是精确的，理由是构造期的不变量：
    /// [`try_new`](Self::try_new) 要求 `width * height * bpp <= texels.len()`，
    /// 而切片长度以字节计、上限为 `isize::MAX`，且 `height >= 1`、`bpp >= 1`，
    /// 故 `limit <= isize::MAX`。`limit as i64` 在 32 位与 64 位平台上都精确，
    /// 不会回绕。`i` 来自 [`floor_to_i32`]，本就在 `i32` 范围内。
    #[inline(always)]
    fn wrap(i: i32, limit: usize, mode: WrapMode) -> usize {
        if limit == 0 {
            return 0;
        }
        // `limit` 受 `try_new` 的切片长度不变量约束，最多为 `isize::MAX`；
        // 在 32 位与 64 位平台上都能无损转换到 i64。避免窄化为 i32 后
        // 在大纹理上回绕成负数，进而让 Clamp/Repeat 产生错误索引或 panic。
        let limit = limit as i64;
        let i = i as i64;
        match mode {
            // `limit >= 1` 已由上面的早退保证，`limit - 1` 不会下溢。
            WrapMode::Clamp => i.clamp(0, limit - 1) as usize,
            // `rem_euclid` 而非 `%`：后者对负数返回负余数（如 -1 % 4 == -1），
            // 直接 `as usize` 会饱和到 usize::MAX 造成越界。
            WrapMode::Repeat => i.rem_euclid(limit) as usize,
        }
    }

    /// 读取 `(x, y)` 处的 texel 并解码为 [`Color`]。
    ///
    /// # 调用方保证
    ///
    /// `x < width` 且 `y < height`。两个 [`wrap`](Self::wrap) 输出都满足此条件。
    #[inline(always)]
    fn texel(&self, x: usize, y: usize) -> Color {
        let bpp = self.format.bytes_per_pixel();
        // `try_new` 保证 `texels.len() >= width * height * bpp`，而
        // `x < width, y < height` ⇒ `offset + bpp <= width * height * bpp`，
        // 故这里的切片下标恒合法，不需要 unsafe。
        let offset = (y * self.width + x) * bpp;
        self.format.read_color(&self.texels[offset..offset + bpp])
    }

    /// 按 UV 坐标采样纹理。
    ///
    /// `filter` 决定过滤方式，`wrap_mode` 决定越界寻址；两者都作用于整幅采样。
    ///
    /// # 边界行为
    ///
    /// - **零尺寸纹理**（`width == 0` 或 `height == 0`）：恒返回 `Color::new(0, 0, 0)`。
    /// - **`NaN` 坐标**：任一分量为 `NaN` 时返回 `Color::new(0, 0, 0)`。不能依赖
    ///   `NaN as usize` 的饱和转换——它恰好得到 0，于是静默采到左上角 texel，
    ///   那是一个「看起来能跑」的错答案。显式归零让契约可预期。
    /// - **无穷坐标**：`±∞` 同样返回透明黑。`∞ * width` 会溢出成 `NaN`
    ///   （`f32` 溢出产生 NaN 而非 panic），落到 `NaN` 分支。
    ///
    /// # 取整口径
    ///
    /// 双线性合成在 `f32` 域一次算完再取整，口径与 `Interpolate for Color`
    /// 完全一致（`(v + 0.5).clamp(0.0, 255.0) as u8`），故纹理采样与 varying
    /// 渐变之间不会出现可见色差。**不要**把它折成两次 `Color::lerp`——
    /// 那会累积 2~3 LSB 误差。
    ///
    /// # 陷阱：双线性不预乘 alpha
    ///
    /// 本实现对四个 texel 的 alpha **直通插值**（non-premultiplied）。在
    /// 半透明纹理上，放大后的边缘会出现暗边/亮边（alpha 光晕）。标准解法是
    /// 先预乘再插值、最后反预乘，但那会引入**第二条**取整口径，与本库
    /// 「插值口径只有一处」的约定冲突，故 v1 不做。测试
    /// `bilinear_interpolates_alpha_straight_through` 记录该行为。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::{Color, PixelFormat};
    /// use rasteron::programmable::texture::{FilterMode, Texture2D, WrapMode};
    ///
    /// let texels = [255, 0, 0, 255, 0, 0, 255, 255];
    /// let texture = Texture2D::new(2, 1, &texels, PixelFormat::Rgba8888);
    /// let sample = |uv| texture.sample(uv, FilterMode::Bilinear, WrapMode::Clamp);
    ///
    /// // texel 中心处取到精确值。
    /// assert_eq!(sample([0.25, 0.5]), Color::new(255, 0, 0));
    /// // 中点恰好是 50% 混合。
    /// assert_eq!(sample([0.5, 0.5]), Color::new(128, 0, 128));
    /// ```
    pub fn sample(&self, uv: Vec2, filter: FilterMode, wrap_mode: WrapMode) -> Color {
        // 零尺寸纹理：没有可采样的 texel，恒返回透明黑。
        // 必须在 `width - 1` 参与 `clamp` 之前短路，否则 usize 下溢成 usize::MAX。
        if self.width == 0 || self.height == 0 {
            return Color::new(0, 0, 0);
        }

        // 半 texel 约定：texel i 的中心落在 u = (i + 0.5) / width 上。
        let x = uv[0] * self.width as f32 - 0.5;
        let y = uv[1] * self.height as f32 - 0.5;

        // `is_finite` 同时排除 NaN 与 ±∞。`no_std` 下没有 `f32::floor` 等方法
        // （需 std 或 libm），故取整全部手写，见 [`floor_to_i32`]。
        if !(x.is_finite() && y.is_finite()) {
            return Color::new(0, 0, 0);
        }

        match filter {
            FilterMode::Nearest => {
                // `floor(x + 0.5)` 是「四舍五入到最近整数」；它让 u = 0.5/width
                // 恰好取到 texel 0，而不是 1。
                let ix = floor_to_i32(x + 0.5);
                let iy = floor_to_i32(y + 0.5);
                self.texel(
                    Self::wrap(ix, self.width, wrap_mode),
                    Self::wrap(iy, self.height, wrap_mode),
                )
            }
            FilterMode::Bilinear => {
                let x0 = floor_to_i32(x);
                let y0 = floor_to_i32(y);
                // 小数部分即权重。`x - floor(x)` 恒在 `[0, 1)`，不需要额外钳制。
                let fx = x - x0 as f32;
                let fy = y - y0 as f32;

                let c00 = self.texel(
                    Self::wrap(x0, self.width, wrap_mode),
                    Self::wrap(y0, self.height, wrap_mode),
                );
                let c10 = self.texel(
                    Self::wrap(x0 + 1, self.width, wrap_mode),
                    Self::wrap(y0, self.height, wrap_mode),
                );
                let c01 = self.texel(
                    Self::wrap(x0, self.width, wrap_mode),
                    Self::wrap(y0 + 1, self.height, wrap_mode),
                );
                let c11 = self.texel(
                    Self::wrap(x0 + 1, self.width, wrap_mode),
                    Self::wrap(y0 + 1, self.height, wrap_mode),
                );

                // 一次算完再取整：与 `Interpolate for Color` 的口径一致。
                let channel = |c00: u8, c10: u8, c01: u8, c11: u8| {
                    let value = c00 as f32 * (1.0 - fx) * (1.0 - fy)
                        + c10 as f32 * fx * (1.0 - fy)
                        + c01 as f32 * (1.0 - fx) * fy
                        + c11 as f32 * fx * fy;
                    (value + 0.5).clamp(0.0, 255.0) as u8
                };
                Color::with_alpha(
                    channel(c00.r, c10.r, c01.r, c11.r),
                    channel(c00.g, c10.g, c01.g, c11.g),
                    channel(c00.b, c10.b, c01.b, c11.b),
                    channel(c00.a, c10.a, c01.a, c11.a),
                )
            }
        }
    }

    /// 以钳制寻址做最近邻采样（便捷入口）。
    ///
    /// 等价于 `sample(uv, FilterMode::Nearest, WrapMode::Clamp)`。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::{Color, PixelFormat};
    /// use rasteron::programmable::texture::Texture2D;
    ///
    /// let texels = [255, 0, 0, 255, 0, 0, 255, 255];
    /// let texture = Texture2D::new(2, 1, &texels, PixelFormat::Rgba8888);
    /// assert_eq!(texture.sample_nearest([0.25, 0.5]), Color::new(255, 0, 0));
    /// ```
    #[inline(always)]
    pub fn sample_nearest(&self, uv: Vec2) -> Color {
        self.sample(uv, FilterMode::Nearest, WrapMode::Clamp)
    }

    /// 以钳制寻址做双线性采样（便捷入口）。
    ///
    /// 等价于 `sample(uv, FilterMode::Bilinear, WrapMode::Clamp)`。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::{Color, PixelFormat};
    /// use rasteron::programmable::texture::Texture2D;
    ///
    /// let texels = [255, 0, 0, 255, 0, 0, 255, 255];
    /// let texture = Texture2D::new(2, 1, &texels, PixelFormat::Rgba8888);
    /// // 中点恰好是 50% 混合。
    /// assert_eq!(texture.sample_bilinear([0.5, 0.5]).r, 128);
    /// ```
    #[inline(always)]
    pub fn sample_bilinear(&self, uv: Vec2) -> Color {
        self.sample(uv, FilterMode::Bilinear, WrapMode::Clamp)
    }

    /// 以重复寻址做最近邻采样（便捷入口）。
    ///
    /// 等价于 `sample(uv, FilterMode::Nearest, WrapMode::Repeat)`。
    #[inline(always)]
    pub fn sample_nearest_repeat(&self, uv: Vec2) -> Color {
        self.sample(uv, FilterMode::Nearest, WrapMode::Repeat)
    }

    /// 以重复寻址做双线性采样（便捷入口）。
    ///
    /// 等价于 `sample(uv, FilterMode::Bilinear, WrapMode::Repeat)`。
    #[inline(always)]
    pub fn sample_bilinear_repeat(&self, uv: Vec2) -> Color {
        self.sample(uv, FilterMode::Bilinear, WrapMode::Repeat)
    }
}

/// `no_std` 下的 `f32::floor`：向下取整到 `i32`。
///
/// # 溢出饱和
///
/// `value` 超出 `i32` 可表示范围时返回 `i32::MIN` / `i32::MAX`
/// （`as` 转换的 Rust 定义的饱和行为）。采样入口已用 `is_finite` 排除了
/// NaN 与 `±∞`，调用方应保证 `|value|` 不会溢出 `i32`；即便溢出，结果仍是
/// 一个确定值，随后被 [`Texture2D::wrap`] 钳制或取模，不会 panic。
#[inline(always)]
fn floor_to_i32(value: f32) -> i32 {
    // 整数值的快速路径：截断与向下取整结果相同，省掉分支与符号判断。
    let truncated = value as i32;
    if truncated as f32 == value {
        return truncated;
    }
    // 负的非整数：向零截断比向下取整大 1。
    if value < 0.0 {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::format;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::error::TextureError;

    /// 构造一张 4×4 纹理，`texel(x, y)` 的颜色由 `f(x, y)` 生成。
    fn checkerboard<'a>(texels: &'a mut [u8], f: impl Fn(usize, usize) -> Color) -> Texture2D<'a> {
        for y in 0..4 {
            for x in 0..4 {
                let offset = (y * 4 + x) * 4;
                let color = f(x, y);
                texels[offset..offset + 4].copy_from_slice(&color.to_bytes(PixelFormat::Rgba8888));
            }
        }
        Texture2D::new(4, 4, texels, PixelFormat::Rgba8888)
    }

    // ——————————————————————— 构造期契约 ———————————————————————

    #[test]
    fn try_new_accepts_exact_length() {
        let texels = [0u8; 4 * 4 * 4];
        assert!(Texture2D::try_new(4, 4, &texels, PixelFormat::Rgba8888).is_ok());
    }

    /// 尾部多余字节被容忍（便于直接借用帧缓冲切片的整段尾部）。
    #[test]
    fn try_new_tolerates_trailing_padding() {
        let texels = [0u8; 4 * 4 * 4 + 32];
        let texture = Texture2D::try_new(4, 4, &texels, PixelFormat::Rgba8888).unwrap();
        assert_eq!(texture.width(), 4);
        // 访问器返回原始借用，不做截断。
        assert_eq!(texture.texels().len(), 4 * 4 * 4 + 32);
    }

    #[test]
    fn try_new_rejects_short_buffer() {
        let texels = [0u8; 4 * 4 * 4];
        assert_eq!(
            Texture2D::try_new(4, 4, &texels[..15], PixelFormat::Rgba8888).unwrap_err(),
            TextureError::LengthMismatch
        );
        // 差一个字节也不行。
        assert_eq!(
            Texture2D::try_new(4, 4, &texels[..63], PixelFormat::Rgba8888).unwrap_err(),
            TextureError::LengthMismatch
        );
    }

    /// 乘法溢出必须报 `DimensionOverflow`，不能绕回小值后通过长度检查。
    #[test]
    fn try_new_reports_dimension_overflow() {
        let texels = [0u8; 4];
        assert_eq!(
            Texture2D::try_new(usize::MAX, 2, &texels, PixelFormat::Rgba8888).unwrap_err(),
            TextureError::DimensionOverflow
        );
        assert_eq!(
            Texture2D::try_new(usize::MAX, usize::MAX, &texels, PixelFormat::Rgba8888).unwrap_err(),
            TextureError::DimensionOverflow
        );
        // `w * h` 恰好不溢出但 `* bpp` 溢出的路径同样要覆盖。
        assert_eq!(
            Texture2D::try_new(1usize << 40, 1usize << 40, &texels, PixelFormat::Rgba8888)
                .unwrap_err(),
            TextureError::DimensionOverflow
        );
    }

    /// `new` 对同样的条件直接恐慌（构造期不变量 = 程序错误）。
    #[test]
    #[should_panic(expected = "texels 长度与 width × height 不匹配")]
    fn new_panics_on_length_mismatch() {
        let texels = [0u8; 16];
        let _ = Texture2D::new(2, 2, &texels[..8], PixelFormat::Rgba8888);
    }

    #[test]
    #[should_panic(expected = "width × height 溢出 usize")]
    fn new_panics_on_dimension_overflow() {
        let texels = [0u8; 4];
        let _ = Texture2D::new(usize::MAX, 2, &texels, PixelFormat::Rgba8888);
    }

    // ——————————————————————— 坐标约定 ———————————————————————

    /// 半 texel 约定：texel `i` 的中心落在 `(i + 0.5) / size`。
    /// 这是与 GL / Vulkan / D3D 一致的口径，也是消除边缘接缝的必要条件。
    #[test]
    fn texel_centers_land_at_half_texel_offsets() {
        let mut texels = [0u8; 4 * 4 * 4];
        let texture = checkerboard(&mut texels, |x, y| {
            Color::new(x as u8 * 60, y as u8 * 60, 0)
        });

        for y in 0..4usize {
            for x in 0..4usize {
                let u = (x as f32 + 0.5) / 4.0;
                let v = (y as f32 + 0.5) / 4.0;
                let expected = Color::new(x as u8 * 60, y as u8 * 60, 0);
                assert_eq!(
                    texture.sample_nearest([u, v]),
                    expected,
                    "texel ({x}, {y}) 中心应采到自身"
                );
            }
        }
    }

    /// 钳制寻址下，`u = 0` 落在 texel 0 的**左边缘**，故仍采到 texel 0；
    /// `u = 1` 落在最后一列的右边缘，同样钳到最后一列。
    /// 半 texel 偏移保证两端各得 1 texel 的完整覆盖，不缩进也不溢出。
    #[test]
    fn clamp_covers_edge_texels_without_seam() {
        let mut texels = [0u8; 4 * 4 * 4];
        let texture = checkerboard(&mut texels, |x, y| {
            Color::new(x as u8 * 60, y as u8 * 60, 0)
        });

        // 左上角与右下角。
        assert_eq!(texture.sample_nearest([0.0, 0.0]), Color::new(0, 0, 0));
        assert_eq!(texture.sample_nearest([1.0, 1.0]), Color::new(180, 180, 0));
        // u = 0 与 u = 1/width 都落在 texel 0（前者在其左边缘）。
        assert_eq!(
            texture.sample_nearest([0.0, 0.0]),
            texture.sample_nearest([0.125, 0.125])
        );
    }

    /// 半 texel 约定不能省：`u = 1.0` 若不偏移就会落到 texel `width`（越界）。
    /// 这里验证越界一侧被钳制而非 wrap 回左侧。
    #[test]
    fn u_one_clamps_to_last_texel_not_wraps() {
        let mut texels = [0u8; 4 * 4 * 4];
        let texture = checkerboard(&mut texels, |x, y| {
            Color::new(x as u8 * 60, y as u8 * 60, 0)
        });
        assert_eq!(
            texture.sample_nearest([1.5, 0.5]).r,
            180,
            "u > 1 应钳到最后一列"
        );
        assert_eq!(
            texture.sample_nearest([-0.5, 0.5]).r,
            0,
            "u < 0 应钳到第一列"
        );
    }

    // ——————————————————————— 寻址 ———————————————————————

    /// `Repeat` 的负坐标必须靠 `rem_euclid` 正确回绕——`%` 会返回负余数。
    #[test]
    fn repeat_wraps_negative_coordinates() {
        let mut texels = [0u8; 4 * 4 * 4];
        let texture = checkerboard(&mut texels, |x, y| {
            Color::new(x as u8 * 60, y as u8 * 60, 0)
        });

        // u = -0.125 → x = -1.0 → round → -1 → 3（最后一列）。
        assert_eq!(texture.sample_nearest_repeat([-0.125, 0.125]).r, 180);
        // u = 0.125 → x = 0.0 → 0，与 u = -0.875（同样 x = -4 → 0）应一致。
        assert_eq!(
            texture.sample_nearest_repeat([-0.875, 0.125]).r,
            texture.sample_nearest_repeat([0.125, 0.125]).r
        );
    }

    /// Repeat 越界时绝不 panic，且结果落在合法 texel 上。
    #[test]
    fn repeat_handles_extreme_coordinates_without_panic() {
        let mut texels = [0u8; 2 * 2 * 4];
        for y in 0..2usize {
            for x in 0..2usize {
                let offset = (y * 2 + x) * 4;
                let color = Color::new(x as u8 * 200, y as u8 * 200, 0);
                texels[offset..offset + 4].copy_from_slice(&color.to_bytes(PixelFormat::Rgba8888));
            }
        }
        let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
        // u = 0.875 → x = 1.25 → round 1 → texel 1（r = 200）。
        assert_eq!(texture.sample_nearest_repeat([0.875, 0.125]).r, 200);
        // u = 1.125 → x = 1.75 → round 2 → 2 rem_euclid 2 = 0 → texel 0。
        assert_eq!(texture.sample_nearest_repeat([1.125, 0.125]).r, 0);
        // 负侧：u = -0.125 → x = -0.75 → round -1 → -1 rem_euclid 2 = 1。
        assert_eq!(texture.sample_nearest_repeat([-0.125, 0.125]).r, 200);
    }

    #[test]
    fn wrap_is_total_over_integers() {
        // 0 尺寸：防御分支，返回 0 而非 panic。
        assert_eq!(Texture2D::wrap(5, 0, WrapMode::Clamp), 0);
        assert_eq!(Texture2D::wrap(-5, 0, WrapMode::Repeat), 0);
        // Clamp 边界。
        assert_eq!(Texture2D::wrap(-100, 4, WrapMode::Clamp), 0);
        assert_eq!(Texture2D::wrap(100, 4, WrapMode::Clamp), 3);
        // Repeat 双侧。
        assert_eq!(Texture2D::wrap(4, 4, WrapMode::Repeat), 0);
        assert_eq!(Texture2D::wrap(-4, 4, WrapMode::Repeat), 0);
        assert_eq!(Texture2D::wrap(-5, 4, WrapMode::Repeat), 3);
        assert_eq!(Texture2D::wrap(9, 4, WrapMode::Repeat), 1);
    }

    /// 回归：`limit` 曾以 `limit as i32` 窄化，而 `as` 是**回绕**不是饱和。
    ///
    /// `limit = i32::MAX + 100` 会变成 `-2147483549`：Clamp 的上界
    /// `limit - 1` 变成 `-2147483550`，`i.clamp(0, 上界)` 对任何非负 `i`
    /// 返回该负数，再 `as usize` 饱和到 `usize::MAX`——**越界索引**；
    /// Repeat 的模数同样为负，`rem_euclid` 直接 panic。
    ///
    /// 这类 `limit` 无法真的构造出纹理（`width × bpp <= texels.len()` 要求
    /// 数十 GB 内存），故直接对 `wrap` 单测其寻址语义。
    #[test]
    fn wrap_handles_limits_beyond_i32_without_wrapping() {
        let limit = i32::MAX as usize + 100;
        // 回绕值 = limit − 2^32 = 2147483747 − 4294967296 = −2147483549。
        assert_eq!(limit as i32, -2147483549, "前提：`limit as i32` 确实回绕成负数");

        // Clamp：上界是 limit - 1，必须原样保留。
        assert_eq!(Texture2D::wrap(i32::MAX, limit, WrapMode::Clamp), i32::MAX as usize);
        assert_eq!(
            Texture2D::wrap(i32::MIN, limit, WrapMode::Clamp),
            0,
            "负坐标仍钳到 0"
        );

        // Repeat：模数是 limit 本身，非负且大于任何可达坐标。
        assert_eq!(Texture2D::wrap(i32::MAX, limit, WrapMode::Repeat), i32::MAX as usize);
        assert_eq!(
            Texture2D::wrap(-1, limit, WrapMode::Repeat),
            limit - 1,
            "负一应回绕到最后一列"
        );
        assert_eq!(
            Texture2D::wrap(i32::MIN, limit, WrapMode::Repeat),
            limit.wrapping_add(i32::MIN as usize),
            "最小坐标取模后仍在 [0, limit)"
        );
    }

    // ——————————————————————— 边界输入 ———————————————————————

    /// 零尺寸纹理允许构造，且任何采样恒返回透明黑（不 panic）。
    #[test]
    fn zero_sized_texture_samples_transparent_black() {
        for (w, h) in [(0usize, 0usize), (0, 4), (4, 0)] {
            let texels = [0u8; 64];
            let texture = Texture2D::try_new(w, h, &texels, PixelFormat::Rgba8888).unwrap();
            for filter in [FilterMode::Nearest, FilterMode::Bilinear] {
                for wrap_mode in [WrapMode::Clamp, WrapMode::Repeat] {
                    assert_eq!(
                        texture.sample([0.5, 0.5], filter, wrap_mode),
                        Color::new(0, 0, 0),
                        "{w}×{h} 纹理在 {filter:?}/{wrap_mode:?} 下应返回透明黑"
                    );
                }
            }
        }
    }

    /// `NaN` 必须显式归零：`NaN as i32` 饱和到 0，会静默采到左上角 texel——
    /// 那是一个「看起来能跑」的错答案。
    #[test]
    fn nan_coordinates_return_transparent_black() {
        let mut texels = [0u8; 4 * 4 * 4];
        let texture = checkerboard(&mut texels, |_, _| Color::new(255, 0, 0));
        let nan = f32::NAN;
        for filter in [FilterMode::Nearest, FilterMode::Bilinear] {
            for wrap_mode in [WrapMode::Clamp, WrapMode::Repeat] {
                for uv in [[nan, 0.5], [0.5, nan], [nan, nan]] {
                    assert_eq!(
                        texture.sample(uv, filter, wrap_mode),
                        Color::new(0, 0, 0),
                        "uv={uv:?} 在 {filter:?}/{wrap_mode:?} 下应为透明黑而非 texel 0"
                    );
                }
            }
        }
    }

    /// `±∞` 在 `uv * size - 0.5` 后仍是有限的（0.5 量级）时正常寻址；
    /// 但 `∞ * 4 = ∞` 再减 0.5 仍是 `∞`，`is_finite` 拦截。
    #[test]
    fn infinite_coordinates_return_transparent_black() {
        let mut texels = [0u8; 4 * 4 * 4];
        let texture = checkerboard(&mut texels, |_, _| Color::new(255, 0, 0));
        let inf = f32::INFINITY;
        for uv in [
            [inf, 0.5],
            [0.5, inf],
            [-inf, -inf],
            [f32::NEG_INFINITY, 0.5],
        ] {
            assert_eq!(
                texture.sample_nearest(uv),
                Color::new(0, 0, 0),
                "uv={uv:?} 应返回透明黑"
            );
        }
    }

    // ——————————————————————— 过滤 ———————————————————————

    /// 双线性在中点恰好 50%，端点取精确值。
    #[test]
    fn bilinear_endpoints_and_midpoint() {
        let texels = [255, 0, 0, 255, 0, 0, 255, 255];
        let texture = Texture2D::new(2, 1, &texels, PixelFormat::Rgba8888);

        // texel 中心 → 精确值。
        assert_eq!(texture.sample_bilinear([0.25, 0.5]), Color::new(255, 0, 0));
        assert_eq!(texture.sample_bilinear([0.75, 0.5]), Color::new(0, 0, 255));
        // u = 0.5 → x = 0.5 → floor 0，fx = 0.5 → 恰好 50%。
        assert_eq!(texture.sample_bilinear([0.5, 0.5]), Color::new(128, 0, 128));
    }

    /// 最近邻永不产生插值误差：结果必是某个原始 texel 值。
    #[test]
    fn nearest_never_produces_interpolated_values() {
        let texels = [
            0, 0, 0, 255, 10, 10, 10, 255, 200, 200, 200, 255, 255, 255, 255, 255,
        ];
        let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
        let valid = [0u8, 10, 200, 255];
        for i in 0..=8 {
            let u = i as f32 / 8.0;
            let color = texture.sample_nearest([u, 0.375]);
            assert!(
                valid.contains(&color.r),
                "最近邻得到 {color:?}，不属于任何原始 texel 值"
            );
        }
    }

    /// 单通道线性：u 从 0.25 到 0.75，r 应从 0 单调到 255（4 级灰阶纹理）。
    #[test]
    fn bilinear_ramps_monotonically() {
        let texels = [
            0u8, 0, 0, 255, 85, 85, 85, 255, 170, 170, 170, 255, 255, 255, 255, 255,
        ];
        let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
        let mut previous = 0u8;
        for i in 0..=10 {
            let u = 0.125 + (i as f32 / 10.0) * 0.75;
            let r = texture.sample_bilinear([u, 0.125]).r;
            assert!(r >= previous, "u={u} 处 r 应单调不减（{previous} → {r}）");
            previous = r;
        }
    }

    /// 双线性必须复用 `Interpolate for Color` 的取整口径：`(v + 0.5).clamp(..) as u8`。
    ///
    /// 在**一维**上（`v` 落在 texel 0 的中心，`fy = 0`）四点权重退化为
    /// `(1-fx, fx, 0, 0)`，此时必须与 `Color::lerp(c00, c10, fx)` 逐位相同——
    /// 否则同一段渐变在纹理采样与 varying 插值两条路径上会出现可见色差。
    #[test]
    fn bilinear_matches_lerp_rounding_on_the_horizontal_axis() {
        let texels = [
            10u8, 20, 30, 255, // texel (0, 0)
            110, 120, 130, 255, // texel (1, 0)
            200, 210, 220, 255, // texel (0, 1)
            250, 240, 230, 255, // texel (1, 1)
        ];
        let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
        let c00 = Color::new(10, 20, 30);
        let c10 = Color::new(110, 120, 130);

        // v = 0.25 → y = 0.25×2 − 0.5 = 0.0 → y0 = 0，fy = 0.0，
        // 四点权重退化为 (1−fx, fx, 0, 0)。
        for (u, fx_expected) in [(0.25f32, 0.0f32), (0.75, 1.0), (0.5, 0.5)] {
            let sampled = texture.sample_bilinear([u, 0.25]);
            let expected = Color::lerp(c00, c10, fx_expected);
            assert_eq!(
                sampled, expected,
                "u={u}（fx={fx_expected}）时 bilerp 应与 lerp 逐位同口径"
            );
        }
    }

    /// 双线性在 2×2 纹理的中点是**四色平均**（四角权重各 0.25），
    /// 不是两次 `lerp`。用数值锁定该语义，防止误改成折线插值。
    #[test]
    fn bilinear_averages_four_texels_at_the_center() {
        let texels = [
            0u8, 0, 0, 255, // texel (0, 0) = 0
            100, 0, 0, 255, // texel (1, 0) = 100
            200, 0, 0, 255, // texel (0, 1) = 200
            255, 0, 0, 255, // texel (1, 1) = 255
        ];
        let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
        // u = v = 0.5 → x = y = 0.5 → 四角权重各 0.25。
        let sampled = texture.sample_bilinear([0.5, 0.5]);
        // (0 + 100 + 200 + 255) / 4 = 138.75 → +0.5 → 139.25 → 139
        assert_eq!(sampled.r, 139, "中点应为四色平均而非两点折线");
    }

    /// alpha 直通插值（non-premultiplied）——v1 刻意不做预乘，记录该行为。
    #[test]
    fn bilinear_interpolates_alpha_straight_through() {
        let texels = [0, 0, 0, 0, 255, 255, 255, 255];
        let texture = Texture2D::new(2, 1, &texels, PixelFormat::Rgba8888);
        // 中点：a 应为 0 与 255 的直接平均，而非预乘后的反解。
        let sampled = texture.sample_bilinear([0.5, 0.5]);
        assert_eq!(sampled.a, 128, "alpha 应直通插值");
        assert_eq!(sampled.r, 128, "颜色同样直通插值");
    }

    // ——————————————————————— 格式解码 ———————————————————————

    /// 所有 8 种格式都必须正确解码：BGR 字节序与 565 位域差异不可丢失。
    #[test]
    fn every_pixel_format_decodes_correctly() {
        // 源颜色选 RGB 各不相同，才能区分通道错位。
        let source = Color::with_alpha(0x12, 0x34, 0x56, 0x78);

        for format in [
            PixelFormat::Rgb24,
            PixelFormat::Bgr24,
            PixelFormat::Xrgb8888,
            PixelFormat::Argb8888,
            PixelFormat::Rgba8888,
            PixelFormat::Bgra8888,
            PixelFormat::Rgb565,
            PixelFormat::Bgr565,
        ] {
            let bpp = format.bytes_per_pixel();
            let texels = source.to_bytes(format);
            let texture = Texture2D::try_new(1, 1, &texels, format).unwrap();
            let decoded = texture.sample_nearest([0.5, 0.5]);

            // 565 会量化，往返不精确；8bpp 格式则应逐位还原。
            if bpp == 2 {
                let round_trip = Color::from_bytes(format, &texels);
                assert_eq!(decoded, round_trip, "{format:?} 的解码应与 from_bytes 一致");
            } else if format.alpha_shift().is_some() {
                assert_eq!(decoded, source, "{format:?} 应逐位还原含 alpha 的颜色");
            } else {
                assert_eq!(
                    decoded,
                    Color::new(0x12, 0x34, 0x56),
                    "{format:?} 无 alpha 通道，应补 255"
                );
            }
        }
    }

    /// `read_color` 的通道错位（AGENTS.md 的历史缺陷「Bgr24 因跨字节错位始终逐通道回退」）
    /// 不得被纹理寻址引入：索引必须是 `y * width + x` 的**像素序**，不是字节序。
    #[test]
    fn texel_index_follows_row_major_pixel_order() {
        // 3 texel × 1 行，BGR24。
        let texels = [
            1, 2, 3, // texel 0 = RGB(3, 2, 1)
            4, 5, 6, // texel 1 = RGB(6, 5, 4)
            7, 8, 9, // texel 2 = RGB(9, 8, 7)
        ];
        let texture = Texture2D::new(3, 1, &texels, PixelFormat::Bgr24);
        assert_eq!(
            texture.sample_nearest([1.0 / 6.0, 0.5]),
            Color::new(3, 2, 1)
        );
        assert_eq!(texture.sample_nearest([0.5, 0.5]), Color::new(6, 5, 4));
        assert_eq!(
            texture.sample_nearest([5.0 / 6.0, 0.5]),
            Color::new(9, 8, 7)
        );
    }

    /// 多行纹理：行跨度必须是 `width * bpp`，不能误用 `height` 或 `bpp`。
    #[test]
    fn multi_row_layout_uses_width_times_bpp() {
        // 4 texel × 3 行 × 4 字节 = 48 字节。
        let mut texels = [0u8; 4 * 3 * 4];
        // 逐 texel 写入：行跨度必须是 `width * bpp = 16`，不能误用 `bpp` 或 `height`。
        for y in 0..3usize {
            for x in 0..4usize {
                let offset = (y * 4 + x) * 4;
                let color = Color::new(x as u8 * 60, y as u8 * 60, 0);
                texels[offset..offset + 4].copy_from_slice(&color.to_bytes(PixelFormat::Rgba8888));
            }
        }
        let texture = Texture2D::new(4, 3, &texels, PixelFormat::Rgba8888);
        // 逐 texel 校验：行 2 的第 1 列不能读到行 0。
        for y in 0..3usize {
            for x in 0..4usize {
                let u = (x as f32 + 0.5) / 4.0;
                let v = (y as f32 + 0.5) / 3.0;
                assert_eq!(
                    texture.sample_nearest([u, v]),
                    Color::new(x as u8 * 60, y as u8 * 60, 0),
                    "texel ({x}, {y}) 行偏移错误"
                );
            }
        }
    }

    // ——————————————————————— from_rows ———————————————————————

    #[test]
    fn from_rows_strips_padding() {
        // 2 texel × 1 行，stride 8（每 texel 4 字节，无填充）→ 恒等拷贝。
        let source = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut destination = [0u8; 8];
        Texture2D::from_rows(2, 1, 8, &source, PixelFormat::Rgba8888, &mut destination);
        assert_eq!(destination, source);
    }

    #[test]
    fn from_rows_handles_real_padding() {
        // 2 texel × 2 行，stride 16（有效 8 字节 + 8 字节填充）。
        let mut source = [0xFFu8; 32];
        for row in 0..2 {
            for col in 0..2 {
                let value = (row * 2 + col) as u8;
                let offset = row * 16 + col * 4;
                source[offset..offset + 4].fill(value);
            }
        }
        let mut destination = [0u8; 16];
        Texture2D::from_rows(2, 2, 16, &source, PixelFormat::Rgba8888, &mut destination);
        // 填充字节（0xFF）不得进入紧凑输出。
        assert_eq!(&destination[..4], &[0, 0, 0, 0]);
        assert_eq!(&destination[4..8], &[1, 1, 1, 1]);
        assert_eq!(&destination[8..12], &[2, 2, 2, 2]);
        assert_eq!(&destination[12..], &[3, 3, 3, 3]);
    }

    #[test]
    #[should_panic(expected = "destination 长度")]
    fn from_rows_rejects_short_destination() {
        let source = [0u8; 32];
        let mut destination = [0u8; 8];
        Texture2D::from_rows(2, 2, 16, &source, PixelFormat::Rgba8888, &mut destination);
    }

    #[test]
    #[should_panic(expected = "row_stride 须至少")]
    fn from_rows_rejects_short_stride() {
        let source = [0u8; 32];
        let mut destination = [0u8; 16];
        // stride 4 < width(2) × bpp(4) = 8。
        Texture2D::from_rows(2, 2, 4, &source, PixelFormat::Rgba8888, &mut destination);
    }

    #[test]
    #[should_panic(expected = "source 长度")]
    fn from_rows_rejects_short_source() {
        let source = [0u8; 8];
        let mut destination = [0u8; 16];
        Texture2D::from_rows(2, 2, 16, &source, PixelFormat::Rgba8888, &mut destination);
    }

    /// 回归：`row_stride × height` 的乘法原先没有 `checked_mul`。
    ///
    /// 这条路径**独立于** `width × height × bpp`：行填充让 `row_stride` 可以
    /// 远大于 `width × bpp`，故前者不溢出不代表后者不溢出。构造一组
    /// `required = row_bytes × height` 很小、但 `row_stride × height` 回绕的
    /// 参数——未检查时 release 模式下 `source.len() >= 0` 恒成立，
    /// 过短的 `source` 被错误接受，直到逐行拷贝才在 `source[src_start..]`
    /// 上越界 panic（panic 点远离出错的参数）。
    #[test]
    #[should_panic(expected = "row_stride × height 溢出 usize")]
    fn from_rows_rejects_overflowing_row_stride_times_height() {
        // width=1 → row_bytes=4；height=2 → required=8（不溢出）。
        let height = 2usize;
        // row_stride × height 回绕到 0..height-1，故 ≤ source.len()。
        let row_stride = usize::MAX / height + 1;
        assert_eq!(row_stride.wrapping_mul(height), 0);

        let source = [0u8; 16];
        let mut destination = [0u8; 16];
        Texture2D::from_rows(1, height, row_stride, &source, PixelFormat::Rgba8888, &mut destination);
    }

    // ——————————————————————— 取整助手 ———————————————————————

    #[test]
    fn floor_to_i32_matches_mathematical_floor() {
        for value in [
            0.0f32, 1.0, -1.0, 0.5, -0.5, 2.5, -2.5, 3.999, -3.999, 0.0001, -0.0001, 1000.5,
            -1000.5,
        ] {
            let truncated = value as i32;
            let expected = if value < 0.0 && truncated as f32 != value {
                truncated - 1
            } else {
                truncated
            };
            assert_eq!(floor_to_i32(value), expected, "floor({value}) 错误");
        }
    }

    /// `±∞` 在这里饱和到 `i32::MAX` / `i32::MIN`——确定值，不 panic。
    /// 采样入口已用 `is_finite` 拦截，本测试守护助手自身的边界安全。
    #[test]
    fn floor_to_i32_saturates_on_infinities() {
        assert_eq!(floor_to_i32(f32::INFINITY), i32::MAX);
        assert_eq!(floor_to_i32(f32::NEG_INFINITY), i32::MIN);
        // 溢出 i32 范围同样饱和（Rust 定义的 `as` 行为），且保持向下取整语义。
        assert_eq!(floor_to_i32(1.0e18), i32::MAX);
        assert_eq!(floor_to_i32(-1.0e18), i32::MIN);
    }

    /// `NaN` 归零：饱和转换恰好给出 0，确定且不 panic。
    #[test]
    fn floor_to_i32_normalizes_nan_to_zero() {
        assert_eq!(floor_to_i32(f32::NAN), 0);
        assert_eq!(floor_to_i32(-f32::NAN), 0);
    }

    // ——————————————————————— 类型契约 ———————————————————————

    /// `WrapMode` / `FilterMode` 是小枚举，可自由复制、用于匹配。
    #[test]
    fn enums_are_copy_and_matchable() {
        let filter = FilterMode::Bilinear;
        let copy = filter;
        assert_eq!(filter, copy);
        let wrap_mode = WrapMode::Repeat;
        assert_ne!(wrap_mode, WrapMode::Clamp);
        assert_eq!(format!("{filter:?}"), "Bilinear");
    }

    /// `Texture2D` 是 `Copy`：着色器把它存在 `&self` 上时可按值传递，零分配。
    #[test]
    fn texture_is_copy() {
        let texels = [0u8; 16];
        let texture = Texture2D::new(2, 2, &texels, PixelFormat::Rgba8888);
        let copy = texture;
        assert_eq!(copy.width(), 2);
        assert_eq!(
            texture.sample_nearest([0.5, 0.5]),
            copy.sample_nearest([0.5, 0.5])
        );
    }

    /// 便捷入口与通用入口必须完全等价（不是「近似等价」）。
    #[test]
    fn convenience_methods_match_general_entry() {
        let mut texels = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let offset = (y * 4 + x) * 4;
                let color = Color::new(x as u8 * 50, y as u8 * 50, 100);
                texels[offset..offset + 4].copy_from_slice(&color.to_bytes(PixelFormat::Rgba8888));
            }
        }
        let texture = Texture2D::new(4, 4, &texels, PixelFormat::Rgba8888);
        for i in 0..=8 {
            let uv = [i as f32 / 8.0, 0.3];
            assert_eq!(
                texture.sample_nearest(uv),
                texture.sample(uv, FilterMode::Nearest, WrapMode::Clamp)
            );
            assert_eq!(
                texture.sample_bilinear(uv),
                texture.sample(uv, FilterMode::Bilinear, WrapMode::Clamp)
            );
            assert_eq!(
                texture.sample_nearest_repeat(uv),
                texture.sample(uv, FilterMode::Nearest, WrapMode::Repeat)
            );
            assert_eq!(
                texture.sample_bilinear_repeat(uv),
                texture.sample(uv, FilterMode::Bilinear, WrapMode::Repeat)
            );
        }
    }

    /// 借用检查的回归护栏：`texels` 为栈上数组时纹理必须可用，
    /// 且不要求 `'static`（与 `benches/renderer.rs` 用 `from_raw_parts_mut` 的做法对照）。
    #[test]
    fn borrows_from_stack() {
        let texels = [255u8, 0, 0, 255];
        {
            let texture = Texture2D::new(1, 1, &texels, PixelFormat::Rgba8888);
            assert_eq!(texture.sample_nearest([0.5, 0.5]), Color::new(255, 0, 0));
        }
        // `texture` 已离开作用域，栈存活。
        assert_eq!(texels[0], 255);
    }

    /// 未使用的 import 护栏：确认 `alloc` 设施确实被用到（避免 `unused_imports` 掩盖）。
    #[test]
    fn alloc_helpers_are_actually_used() {
        let owned: Vec<u8> = vec![0u8; 4];
        assert_eq!(owned.len(), 4);
        assert!(format!("{:?}", WrapMode::Repeat).contains("Repeat"));
    }
}
