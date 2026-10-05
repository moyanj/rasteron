use crate::error::ColorParseError;

/// 具有 8 位通道的 RGBA 颜色。
///
/// 大多数绘制方法接受 `&Color`，以便在热路径中做出零成本的混合决策，
/// 同时避免强制调用方进行克隆。
///
/// # 示例
///
/// ```
/// # use rasteron::color::Color;
/// let opaque = Color::new(0xFF, 0x00, 0x00);              // 红色
/// let semi   = Color::with_alpha(0x00, 0x00, 0xFF, 0x80); // 蓝色，50% 透明度
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// 红色通道。
    pub r: u8,
    /// 绿色通道。
    pub g: u8,
    /// 蓝色通道。
    pub b: u8,
    /// Alpha 通道（`0` = 完全透明，`255` = 完全不透明）。
    pub a: u8,
}

impl Color {
    /// 从 RGB 通道创建不透明颜色（alpha 默认为 `255`）。
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// 从 RGBA 通道创建颜色。
    pub const fn with_alpha(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 从十六进制字符串解析颜色。
    ///
    /// 支持的格式：
    ///
    /// | 格式 | 示例 |
    /// |------|------|
    /// | `#RRGGBB` | `"#FF0000"` → 不透明红色 |
    /// | `#RRGGBBAA` | `"#FF000080"` → 红色，50% 透明度 |
    ///
    /// 开头的 `#` 是可选的。
    ///
    /// # 错误
    ///
    /// 如果十六进制字符串（去除 `#` 后）长度不是 6 或 8 个字符，返回
    /// [`ColorParseError::InvalidLength`]；
    /// 如果包含无效的十六进制字符，返回 [`ColorParseError::InvalidDigit`]。
    pub fn from_hex(hex: &str) -> Result<Self, ColorParseError> {
        let hex = hex.trim_start_matches('#');

        if hex.len() != 6 && hex.len() != 8 {
            return Err(ColorParseError::InvalidLength);
        }

        fn parse_channel(hex: &str) -> Result<u8, ColorParseError> {
            u8::from_str_radix(hex, 16).map_err(|_| ColorParseError::InvalidDigit)
        }

        let r = parse_channel(&hex[0..2])?;
        let g = parse_channel(&hex[2..4])?;
        let b = parse_channel(&hex[4..6])?;

        if hex.len() == 8 {
            let a = parse_channel(&hex[6..8])?;
            Ok(Self::with_alpha(r, g, b, a))
        } else {
            Ok(Self::new(r, g, b))
        }
    }

    /// 将颜色编码为 `R24G16B8A0` 格式的 `u32`。
    ///
    /// 仅用于与外部系统交换；内部帧缓冲写入请使用 [`to_bytes`](Color::to_bytes)。
    pub fn to_rgba_u32(&self) -> u32 {
        let r = self.r as u32;
        let g = self.g as u32;
        let b = self.b as u32;
        let a = self.a as u32;
        (r << 24) | (g << 16) | (b << 8) | a
    }

    /// 从 `R24G16B8A0` 格式的 `u32` 解码颜色。
    pub fn from_rgba_u32(color: u32) -> Self {
        let r = ((color >> 24) & 0xFF) as u8;
        let g = ((color >> 16) & 0xFF) as u8;
        let b = ((color >> 8) & 0xFF) as u8;
        let a = (color & 0xFF) as u8;
        Self::with_alpha(r, g, b, a)
    }

    /// 将颜色按指定像素格式编码为字节数组（小端序）。
    ///
    /// 返回一个 4 字节数组；对于 3 字节格式，最后一个字节未使用。
    #[inline(always)]
    pub fn to_bytes(&self, format: PixelFormat) -> [u8; 4] {
        let mut value = 0u32;
        value |= (self.r as u32) << format.red_shift();
        value |= (self.g as u32) << format.green_shift();
        value |= (self.b as u32) << format.blue_shift();

        if let Some(alpha_shift) = format.alpha_shift() {
            value |= (self.a as u32) << alpha_shift;
        }

        value.to_le_bytes()
    }

    /// 从按指定像素格式编码的字节数组解码颜色。
    ///
    /// # 恐慌
    ///
    /// 如果 `bytes` 长度小于 `format.bytes_per_pixel()`，将会恐慌。
    #[inline(always)]
    pub fn from_bytes(format: PixelFormat, bytes: &[u8]) -> Self {
        debug_assert!(
            bytes.len() >= format.bytes_per_pixel(),
            "not enough bytes for pixel format {:?}",
            format
        );

        let mut value = 0u32;
        for (index, byte) in bytes.iter().take(format.bytes_per_pixel()).enumerate() {
            value |= (*byte as u32) << (index * 8);
        }

        let r = ((value >> format.red_shift()) & 0xFF) as u8;
        let g = ((value >> format.green_shift()) & 0xFF) as u8;
        let b = ((value >> format.blue_shift()) & 0xFF) as u8;
        let a = format
            .alpha_shift()
            .map(|shift| ((value >> shift) & 0xFF) as u8)
            .unwrap_or(255);

        Self::with_alpha(r, g, b, a)
    }

    /// 替换颜色的 alpha 值，保留 RGB 通道不变。
    pub fn mix_alpha(&self, alpha: u8) -> Self {
        Self::with_alpha(self.r, self.g, self.b, alpha)
    }

    /// 将当前颜色与另一个颜色做标准 Alpha 混合，透明度由 `alpha` 控制。
    ///
    /// 逐通道委托 [`PixelFormat::blend_channel`]，与固定功能路径的字节层实现
    /// （[`PixelFormat::write_color`]）**共用同一公式**：
    ///
    /// ```text
    /// (src × alpha + dst × (255 − alpha) + 255) >> 8
    /// ```
    ///
    /// 这是对精确 `/255` 的近似，误差 ≤ 1。全库只此一份 Alpha 混合语义——
    /// SWAR 内核、固定功能跨度填充与可编程管线都收敛到这里，因此同一次混合
    /// 在任意路径上得到逐位相同的像素。
    ///
    /// 返回值恒为不透明（`alpha == 255`）：混合结果的 alpha 通道从不落盘，
    /// 4bpp 格式写入的是格式自身的 opaque alpha 字节。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::Color;
    ///
    /// let src = Color::new(255, 0, 0);
    /// let dst = Color::new(0, 0, 255);
    /// // alpha = 128 时结果落在两色之间。
    /// assert_eq!(src.blend(&dst, 128), Color::new(128, 0, 127));
    /// ```
    #[inline(always)]
    pub fn blend(&self, other: &Color, alpha: u8) -> Self {
        let inv_alpha = 255 - alpha;
        let channel = |src: u8, dst: u8| {
            PixelFormat::blend_channel(src, dst, alpha as u16, inv_alpha as u16)
        };
        Color::new(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
        )
    }

    /// 加法混合：各通道饱和相加（`min(src + dst, 255)`），常用于发光/叠加。
    ///
    /// 仅供可编程管线的 `BlendMode::Add` 使用，结果恒为不透明。
    #[cfg(feature = "programmable")]
    pub(crate) fn blend_add(self, dst: &Color) -> Color {
        Color::new(
            self.r.saturating_add(dst.r),
            self.g.saturating_add(dst.g),
            self.b.saturating_add(dst.b),
        )
    }

    /// 乘法混合：各通道 `src × dst / 255`（`+127` 四舍五入），用于阴影/染色。
    ///
    /// 仅供可编程管线的 `BlendMode::Multiply` 使用，结果恒为不透明。
    #[cfg(feature = "programmable")]
    pub(crate) fn blend_multiply(self, dst: &Color) -> Color {
        let channel = |src: u8, dst: u8| ((src as u16 * dst as u16 + 127) / 255) as u8;
        Color::new(
            channel(self.r, dst.r),
            channel(self.g, dst.g),
            channel(self.b, dst.b),
        )
    }

    /// 预乘 Alpha 混合：片元颜色被视为已乘过 `src.a`，故直接加到衰减后的目标上，
    /// 即 `src + dst × (1 - src.a)`，结果饱和到 255。
    ///
    /// 与 [`Color::blend`] 的区别：后者会对源色再乘一次 `alpha`，本函数不会。
    ///
    /// 仅供可编程管线的 `BlendMode::PremultipliedAlpha` 使用，结果恒为不透明。
    #[cfg(feature = "programmable")]
    pub(crate) fn blend_premultiplied(self, dst: &Color) -> Color {
        let inv_alpha = 255 - self.a;
        let channel = |src: u8, dst: u8| {
            src.saturating_add(((dst as u16 * inv_alpha as u16 + 127) / 255) as u8)
        };
        Color::new(
            channel(self.r, dst.r),
            channel(self.g, dst.g),
            channel(self.b, dst.b),
        )
    }

    /// 返回颜色的反色（按位取反 RGB 通道）。
    pub fn invert(&self) -> Color {
        Color::new(!self.r, !self.g, !self.b)
    }
}

/// 像素格式描述，定义了内存中通道的布局和字节对齐。
///
/// | 格式 | 字节数 | 说明 |
/// |------|--------|------|
/// | `Rgb24` | 3 | 红、绿、蓝（小端序），无 alpha |
/// | `Bgr24` | 3 | 蓝、绿、红（小端序），无 alpha |
/// | `Xrgb8888` | 4 | 蓝、绿、红 + 保留字节，无 alpha |
/// | `Argb8888` | 4 | 蓝、绿、红、Alpha |
/// | `Rgba8888` | 4 | 红、绿、蓝、Alpha |
/// | `Bgra8888` | 4 | 蓝、绿、红、Alpha |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 3 字节小端序：R, G, B。
    Rgb24,
    /// 3 字节小端序：B, G, R。
    Bgr24,
    /// 4 字节小端序：B, G, R, 保留（通常忽略）。
    Xrgb8888,
    /// 4 字节小端序：B, G, R, A。
    Argb8888,
    /// 4 字节小端序：R, G, B, A。
    Rgba8888,
    /// 4 字节小端序：B, G, R, A。
    Bgra8888,
}

impl PixelFormat {
    /// Alpha 混合单通道的计算。
    ///
    /// 使用 `(src * alpha + dst * inv_alpha + 255) >> 8` 近似
    /// 标准 `(src * alpha + dst * inv_alpha) / 255`，误差 ≤ 1。
    #[inline(always)]
    pub fn blend_channel(src: u8, dst: u8, alpha: u16, inv_alpha: u16) -> u8 {
        let v = src as u16 * alpha + dst as u16 * inv_alpha;
        ((v + 255) >> 8) as u8
    }

    #[inline(always)]
    const fn opaque_alpha_byte(self) -> u8 {
        match self {
            PixelFormat::Xrgb8888 => 0,
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => 255,
            PixelFormat::Argb8888 | PixelFormat::Rgba8888 | PixelFormat::Bgra8888 => 255,
        }
    }

    /// 红色通道在 32 位小端字中的位移。
    ///
    /// 例如 BGR 布局下 red 在高 16 位，位移为 16。
    #[inline(always)]
    pub const fn red_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 0,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 16,
        }
    }

    /// 绿色通道的位移（所有格式均为 8）。
    #[inline(always)]
    pub const fn green_shift(self) -> u8 {
        8
    }

    /// 蓝色通道在 32 位小端字中的位移。
    #[inline(always)]
    pub const fn blue_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 16,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 0,
        }
    }

    /// Alpha 通道的位移（如果格式支持）。
    ///
    /// 对于不含 alpha 的格式（`Rgb24`、`Bgr24`、`Xrgb8888`）返回 `None`。
    #[inline(always)]
    pub const fn alpha_shift(self) -> Option<u8> {
        match self {
            PixelFormat::Argb8888 | PixelFormat::Rgba8888 | PixelFormat::Bgra8888 => Some(24),
            PixelFormat::Rgb24 | PixelFormat::Bgr24 | PixelFormat::Xrgb8888 => None,
        }
    }

    /// 每个像素占用的字节数。
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => 3,
            PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Rgba8888
            | PixelFormat::Bgra8888 => 4,
        }
    }

    /// 每个像素占用的位数。
    pub const fn bits_per_pixel(self) -> usize {
        self.bytes_per_pixel() * 8
    }

    /// 将颜色写入目标缓冲区（支持 Alpha 混合）。
    ///
    /// 当 `color.a == 255` 时直接覆盖；否则使用 Alpha 混合。
    ///
    /// # 调用方保证
    ///
    /// `dst` 长度至少为 `bytes_per_pixel()`。
    #[inline(always)]
    pub fn write_color(self, dst: &mut [u8], color: &Color) {
        debug_assert!(dst.len() >= self.bytes_per_pixel());
        let alpha = color.a as u16;
        if alpha == 255 {
            match self {
                PixelFormat::Rgb24 | PixelFormat::Rgba8888 => {
                    dst[0] = color.r;
                    dst[1] = color.g;
                    dst[2] = color.b;
                    if matches!(self, PixelFormat::Rgba8888) {
                        dst[3] = 255;
                    }
                }
                PixelFormat::Bgr24
                | PixelFormat::Xrgb8888
                | PixelFormat::Argb8888
                | PixelFormat::Bgra8888 => {
                    dst[0] = color.b;
                    dst[1] = color.g;
                    dst[2] = color.r;
                    if self.bytes_per_pixel() == 4 {
                        dst[3] = self.opaque_alpha_byte();
                    }
                }
            }
            return;
        }

        let inv_alpha = 255 - alpha;
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => {
                dst[0] = Self::blend_channel(color.r, dst[0], alpha, inv_alpha);
                dst[1] = Self::blend_channel(color.g, dst[1], alpha, inv_alpha);
                dst[2] = Self::blend_channel(color.b, dst[2], alpha, inv_alpha);
                if matches!(self, PixelFormat::Rgba8888) {
                    dst[3] = 255;
                }
            }
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => {
                dst[0] = Self::blend_channel(color.b, dst[0], alpha, inv_alpha);
                dst[1] = Self::blend_channel(color.g, dst[1], alpha, inv_alpha);
                dst[2] = Self::blend_channel(color.r, dst[2], alpha, inv_alpha);
                if self.bytes_per_pixel() == 4 {
                    dst[3] = self.opaque_alpha_byte();
                }
            }
        }
    }

    /// 从源缓冲区读取一个像素并解码为颜色。
    ///
    /// 对于不含 alpha 的格式，返回的 alpha 为 `255`。
    ///
    /// # 调用方保证
    ///
    /// `src` 长度至少为 `bytes_per_pixel()`。
    #[inline(always)]
    pub fn read_color(self, src: &[u8]) -> Color {
        match self {
            PixelFormat::Rgb24 => Color::new(src[0], src[1], src[2]),
            PixelFormat::Bgr24 => Color::new(src[2], src[1], src[0]),
            PixelFormat::Xrgb8888 => Color::new(src[2], src[1], src[0]),
            PixelFormat::Argb8888 => Color::with_alpha(src[2], src[1], src[0], src[3]),
            PixelFormat::Rgba8888 => Color::with_alpha(src[0], src[1], src[2], src[3]),
            PixelFormat::Bgra8888 => Color::with_alpha(src[2], src[1], src[0], src[3]),
        }
    }
}

/// 便捷宏：从字面量创建 [`Color`]。
///
/// 支持以下调用方式：
///
/// | 形式 | 说明 |
/// |------|------|
/// | `color!(r, g, b)` | 不透明颜色（alpha = 255） |
/// | `color!(r, g, b, a)` | 带透明度的颜色 |
/// | `color!("#RRGGBB")` | 十六进制字符串（不透明） |
/// | `color!("#RRGGBBAA")` | 十六进制字符串（带透明度） |
///
/// # 示例
///
/// ```
/// # use rasteron::{color, color::Color};
/// assert_eq!(color!(255, 0, 0), Color::new(255, 0, 0));
/// ```
#[macro_export]
macro_rules! color {
    // RGB 格式 (自动填充 alpha=255)
    ($r:expr, $g:expr, $b:expr) => {
        Color::new($r as u8, $g as u8, $b as u8)
    };

    // RGBA 格式
    ($r:expr, $g:expr, $b:expr, $a:expr) => {
        Color::with_alpha($r as u8, $g as u8, $b as u8, $a as u8)
    };

    // 十六进制颜色码 (支持 #RRGGBB 和 #RRGGBBAA)
    (#$hex:expr) => {
        Color::from_hex($hex).expect("invalid hex color literal")
    };
}

// ———————————————————————————————— 基础颜色 ————————————————————————————————

/// 黑色 `(0, 0, 0)`。
pub const BLACK: Color = color!(0, 0, 0);
/// 白色 `(255, 255, 255)`。
pub const WHITE: Color = color!(255, 255, 255);
/// 红色 `(255, 0, 0)`。
pub const RED: Color = color!(255, 0, 0);
/// 绿色 `(0, 255, 0)`。
pub const GREEN: Color = color!(0, 255, 0);
/// 蓝色 `(0, 0, 255)`。
pub const BLUE: Color = color!(0, 0, 255);

// ———————————————————————————————— 实用混合色 ————————————————————————————————

/// 黄色 `(255, 255, 0)`（红 + 绿）。
pub const YELLOW: Color = color!(255, 255, 0);
/// 青色 `(0, 255, 255)`（绿 + 蓝）。
pub const CYAN: Color = color!(0, 255, 255);
/// 品红色 `(255, 0, 255)`（红 + 蓝）。
pub const MAGENTA: Color = color!(255, 0, 255);

// ———————————————————————————————— 中性色 ————————————————————————————————

/// 灰色 `(128, 128, 128)`。
pub const GRAY: Color = color!(128, 128, 128);

// ———————————————————————————————— SWAR Alpha 混合 ————————————————————————————————

/// 面向 4bpp 像素格式的 SWAR（SIMD Within A Register）Alpha 混合内核。
///
/// 只用 32 位整数运算，不依赖任何架构 intrinsics 或内联汇编，
/// 因此在 `no_std` 与任意目标上都可用。
///
/// 布局约定：一个 `u32` 字按小端序承载 4 个字节 `[c0, c1, c2, c3]`，
/// 即字节 `i` 位于位 `8i`。SWAR 运算时先把待处理的字节「摊」到 16 位槽里，
/// 空出的高 8 位用作乘法与加法的进位空间。
pub(crate) mod swar {
    use super::{Color, PixelFormat};

    /// 「偶数号字节」掩码：保留 `c0`、`c2`，抹掉 `c1`、`c3`。
    ///
    /// 被保留的字节因此各自独占一个 16 位槽（低 8 位是数据，高 8 位空出）。
    const LANE_MASK: u32 = 0x00FF_00FF;

    /// 每个 16 位槽的舍入项，等价于 [`PixelFormat::blend_channel`] 里的 `+ 255`。
    ///
    /// 数值与 [`LANE_MASK`] 相同，但语义不同（把 255 加到每个槽上），
    /// 故单独命名。
    const ROUND_LANES: u32 = 0x00FF_00FF;

    /// 构造与目标字同布局的 SWAR 源字。
    ///
    /// 返回字的 4 个字节依次为 `[c0, c1, c2, opaque_alpha]`：`is_rgb_order`
    /// 为真时 `c0..c2` 是 R/G/B，否则是 B/G/R。第 4 字节在混合后会被强制
    /// 覆写成 `opaque_alpha`，这里的取值只影响一个即将被丢弃的槽。
    #[inline(always)]
    pub(crate) fn source_word(r: u8, g: u8, b: u8, opaque_alpha: u8, is_rgb_order: bool) -> u32 {
        let (c0, c1, c2) = if is_rgb_order { (r, g, b) } else { (b, g, r) };
        (c0 as u32) | ((c1 as u32) << 8) | ((c2 as u32) << 16) | ((opaque_alpha as u32) << 24)
    }

    /// 对两个「偶数号字节」槽并行执行一次 `(d*(255-a) + s*a + 255) >> 8`。
    ///
    /// `d` / `s` 必须已用 [`LANE_MASK`] 把待处理字节放进 16 位槽的低 8 位；
    /// 返回值的槽布局与输入一致，每个槽的低 8 位是混合结果。
    ///
    /// # 取值范围
    ///
    /// 因为 `a ≤ 255`，槽内各项都不超过 65025：
    ///
    /// | 项 | 上界 |
    /// |----|------|
    /// | `d * 255` | 65025 |
    /// | `d * a` | 65025 |
    /// | `s * a` | 65025 |
    /// | `d*255 - d*a + s*a = d*(255-a) + s*a` | 65025 |
    /// | 再加舍入项 `255` | 65280 |
    ///
    /// 中间量 `d*255 - d*a` 与最终结果都落在 `[0, 65025]`，因此槽与槽之间
    /// 既不会进位也不会借位，整个 32 位字也不会回绕（debug 下不会触发溢出检查）。
    #[inline(always)]
    fn blend_lane_pairs(d: u32, s: u32, alpha: u32) -> u32 {
        // `d*255` 展开成 `(d << 8) - d`，省掉一次乘法。`d` 已被 LANE_MASK
        // 约束为每槽只占低 8 位，左移不会丢失任何数据。
        let blended = (d << 8) - d - d * alpha + s * alpha + ROUND_LANES;
        (blended >> 8) & LANE_MASK
    }

    /// 用 SWAR 对 4 个 8 位通道并行做 Alpha 混合。
    ///
    /// `word` 与 `src` 都按小端字节序布局，返回值的字节 `i` 等于
    /// `PixelFormat::blend_channel(src_i, word_i, alpha, 255 - alpha)`，
    /// 与逐通道参考实现逐字节一致。
    ///
    /// # 为什么分两批
    ///
    /// 逐通道参考式 `s*a + d*(255-a)` 的中间值需要 16 位，4 个通道就要 64 位；
    /// 这里把 `c0`/`c2` 留在原位、`c1`/`c3` 右移 8 位，于是一个 32 位字恰好
    /// 同时容纳两个通道的 16 位中间值，两批各处理两个通道。
    ///
    /// 直接展开 `s*a + d*(255-a)` 的中间和可达 `130050 > 2^16`，会跨槽进位；
    /// 改写成 `d*255 - d*a + s*a` 后每一项都不超过 65025，槽内自洽
    /// （推导见 [`blend_lane_pairs`]）。
    #[inline(always)]
    pub(crate) fn blend_word(word: u32, src: u32, alpha: u8) -> u32 {
        let a = alpha as u32;
        let even = blend_lane_pairs(word & LANE_MASK, src & LANE_MASK, a);
        let odd = blend_lane_pairs((word >> 8) & LANE_MASK, (src >> 8) & LANE_MASK, a);
        even | (odd << 8)
    }

    /// 对 4bpp 格式的一行像素做 SWAR 批量 Alpha 混合。
    ///
    /// 每次处理 4 个像素（16 字节 = 4 个 `u32` 字），不足 4 像素的尾部回退到
    /// 逐通道实现。第 4 字节恒被写成 `opaque_alpha` 而不参与插值
    /// （`Xrgb8888` 为 0，其余 4bpp 格式为 255）。
    ///
    /// `color.a` 必须落在 `1..=254`：完全透明由调用方提前跳过，完全不透明走
    /// 不透明填充路径。
    ///
    /// # Safety
    ///
    /// * `row` 必须指向 `pixel_count * 4` 字节的可写区域；
    /// * `row` 必须按 4 字节对齐（内核按 `u32` 读写）。
    #[inline(never)]
    pub(crate) unsafe fn blend_span_alpha_4bpp(
        row: *mut u8,
        pixel_count: usize,
        color: &Color,
        is_rgb_order: bool,
        opaque_alpha: u8,
    ) {
        let alpha = color.a;
        let src_word = source_word(color.r, color.g, color.b, opaque_alpha, is_rgb_order);
        let opaque_word = (opaque_alpha as u32) << 24;

        let quads = pixel_count / 4;
        for quad in 0..quads {
            // SAFETY: 调用方保证 row 指向 pixel_count * 4 字节且 4 字节对齐，
            // 故 quad * 16 + 16 <= quads * 16 <= pixel_count * 4，且每次访问都对齐。
            let dst = unsafe { row.add(quad * 16) as *mut u32 };
            for lane in 0..4 {
                unsafe {
                    let word = *dst.add(lane);
                    let blended = blend_word(word, src_word, alpha);
                    // 第 4 字节必须恒为 opaque_alpha，而不是参与混合的结果——
                    // 与逐像素路径的 `*dst.add(3) = opaque_alpha` 保持一致。
                    *dst.add(lane) = (blended & 0x00FF_FFFF) | opaque_word;
                }
            }
        }

        // 尾部不足 4 像素的部分回退到逐通道路径
        let alpha16 = alpha as u16;
        let inv_alpha = 255 - alpha16;
        let (c0, c1, c2) = if is_rgb_order {
            (color.r, color.g, color.b)
        } else {
            (color.b, color.g, color.r)
        };
        for pixel_idx in (quads * 4)..pixel_count {
            // SAFETY: pixel_idx < pixel_count，故 pixel_idx * 4 + 4 <= pixel_count * 4。
            let dst = unsafe { row.add(pixel_idx * 4) };
            unsafe {
                *dst = PixelFormat::blend_channel(c0, *dst, alpha16, inv_alpha);
                *dst.add(1) = PixelFormat::blend_channel(c1, *dst.add(1), alpha16, inv_alpha);
                *dst.add(2) = PixelFormat::blend_channel(c2, *dst.add(2), alpha16, inv_alpha);
                *dst.add(3) = opaque_alpha;
            }
        }
    }

    /// 对 4bpp 格式的一行像素做掩码（逐像素 alpha）混合。
    ///
    /// 与 [`blend_span_alpha_4bpp`] 的区别是：每个像素的透明度来自掩码而不是
    /// 同一个源色。
    ///
    /// # 实现策略：4 像素组分类 + 混合段批量
    ///
    /// 旧实现把 `mask == 0` 跳过、`mask == 255` 直写与混合计算写在同一个
    /// 循环的三分支里，数据依赖分支让整个循环无法向量化（SSE2 下实测
    /// 3.2 ns/像素）；而完全无分支的单遍混合又会让抗锯齿掩码里大量
    /// `0`/`255` 像素白白付出读-写代价（实测 AA 场景近乎翻倍）。现取两者
    /// 之长，按 **4 像素组** 分类：
    ///
    /// 1. 主循环每次读 4 个掩码字节，`m4 == 0` 整组跳过（不读不写）、
    ///    `m4 == 0xFFFF_FFFF` 整组直写 `opaque_pixel`（不读原字）——
    ///    判定成本仅 1 次比较 / 4 像素，短路语义按组保留；
    /// 2. 混合组（掩码含中间值）拼成连续段，交由 [`blend_masked_segment`]
    ///    批量处理。段边界是运行时长度、与段内数据无关，编译器可像
    ///    [`blend_span_alpha_4bpp`] 那样自动向量化；
    /// 3. 段内每个像素走单遍位运算混合：`blend_word(word, src, 0)` 恒等于
    ///    `word`、`blend_word(word, src, 255)` 的前三字节恒等于源色，故
    ///    混合本身无需分支；第 4 字节用 `(m - 1) >> 31` 构造的掩码做纯位
    ///    运算选择（`m == 0` 保留原字第 4 字节，否则写 `opaque_word`），
    ///    不引入任何控制流——组内偶然混入的 `0`/`255` 像素因此值仍正确。
    ///
    /// 与旧实现的差异仅在交错掩码（同一 4 像素组内混有 `0`/`255`）的内存
    /// 访问模式：这些像素会被读一次并把原值写回（像素值不变）。等价性由
    /// `blend_masked_row_matches_write_color_reference` 守护。
    ///
    /// # Safety
    ///
    /// * `row` 必须指向 `pixel_count * 4` 字节的可写区域，且按 4 字节对齐；
    /// * `mask` 必须指向 `pixel_count` 个可读字节。
    #[inline(never)]
    pub(crate) unsafe fn blend_masked_row_4bpp(
        row: *mut u8,
        mask: *const u8,
        pixel_count: usize,
        color: &Color,
        is_rgb_order: bool,
        opaque_alpha: u8,
    ) {
        let src_word = source_word(color.r, color.g, color.b, opaque_alpha, is_rgb_order);
        let opaque_word = (opaque_alpha as u32) << 24;
        // 整组不透明直写用的整像素字：通道顺序与目标字一致
        let (c0, c1, c2) = if is_rgb_order {
            (color.r, color.g, color.b)
        } else {
            (color.b, color.g, color.r)
        };
        let opaque_pixel = (c0 as u32) | ((c1 as u32) << 8) | ((c2 as u32) << 16) | opaque_word;

        // 进行中的混合段起点；usize::MAX 表示无待收尾的段。
        // 迭代量只取决于 pixel_count（帧缓冲派生），符合迭代量上界约束。
        let mut run_start = usize::MAX;
        let mut idx = 0usize;
        while pixel_count - idx >= 4 {
            // SAFETY: idx + 4 <= pixel_count，故 mask 的 4 字节可读。
            // 掩码按小端拼成 u32 后与字节序无关（只比较全 0 / 全 0xFF）。
            let m4 = unsafe { core::ptr::read_unaligned(mask.add(idx) as *const u32) };
            if m4 == 0 {
                // 整组全透明：不读不写；先收尾进行中的混合段
                if run_start != usize::MAX {
                    // SAFETY: run_start..idx ⊆ [0, pixel_count)，前置条件由
                    // 本函数 # Safety 段保证
                    unsafe {
                        blend_masked_segment(row, mask, run_start, idx, src_word, opaque_word)
                    };
                    run_start = usize::MAX;
                }
            } else if m4 == 0xFFFF_FFFF {
                // 整组全不透明：直写 4 个整像素，省掉读-改-写；同样先收尾
                if run_start != usize::MAX {
                    // SAFETY: 同上
                    unsafe {
                        blend_masked_segment(row, mask, run_start, idx, src_word, opaque_word)
                    };
                    run_start = usize::MAX;
                }
                // SAFETY: idx + 4 <= pixel_count 且 row 按 4 字节对齐，
                // 故 4 个 u32 写入都在可写区域内且对齐。
                unsafe {
                    let base = row.add(idx * 4) as *mut u32;
                    for k in 0..4 {
                        *base.add(k) = opaque_pixel;
                    }
                }
            } else {
                // 混合组：只记录段起点，把计算留给可向量化的段内核
                if run_start == usize::MAX {
                    run_start = idx;
                }
            }
            idx += 4;
        }
        if run_start != usize::MAX {
            // SAFETY: run_start..idx ⊆ [0, pixel_count)
            unsafe { blend_masked_segment(row, mask, run_start, idx, src_word, opaque_word) };
            run_start = usize::MAX;
        }
        if idx < pixel_count {
            // 尾部不足 4 像素：逐像素位运算混合（组内 0/255 的值由
            // blend_word 的恒等性质与 keep_high 掩码保证，见函数文档）
            // SAFETY: idx..pixel_count ⊆ [0, pixel_count)
            unsafe { blend_masked_segment(row, mask, idx, pixel_count, src_word, opaque_word) };
        }
        // run_start 在此处必为 usize::MAX（两条收尾路径已覆盖）
        debug_assert_eq!(run_start, usize::MAX);
    }

    /// 对 `[start, end)` 内的像素逐个执行位运算掩码混合（段内核）。
    ///
    /// 段内可能含任意掩码值：`m == 0` 时 `blend_word` 恒等于原字且
    /// `keep_high` 保留原第 4 字节（值不变），`m == 255` 时前三字节恒等于
    /// 源色——因此循环体无需任何数据依赖分支，唯一的边界 `start..end`
    /// 是运行时长度、与段内数据无关，可被自动向量化。
    ///
    /// # Safety
    ///
    /// * `row` 必须指向 `pixel_count * 4` 字节的可写区域，且按 4 字节对齐；
    /// * `mask` 必须指向 `pixel_count` 个可读字节；
    /// * `start..end` 必须落在 `[0, pixel_count]` 内。
    #[inline(always)]
    unsafe fn blend_masked_segment(
        row: *mut u8,
        mask: *const u8,
        start: usize,
        end: usize,
        src_word: u32,
        opaque_word: u32,
    ) {
        for idx in start..end {
            // SAFETY: start..end ⊆ [0, pixel_count)，故 mask[idx] 可读、
            // row.add(idx * 4) 落在可写区域内，且 4 字节对齐保证 u32 访问对齐。
            unsafe {
                let m = *mask.add(idx);
                let dst = row.add(idx * 4);
                let word = *(dst as *const u32);
                let blended = blend_word(word, src_word, m);
                // m == 0 时 blended == word，第 4 字节须保留原值：
                // keep_high = m == 0 ? 0xFF00_0000 : 0（无分支构造）
                let keep_high = 0u32.wrapping_sub((m as u32).wrapping_sub(1) >> 31) << 24;
                let out = (blended & 0x00FF_FFFF) | (word & keep_high) | (opaque_word & !keep_high);
                *(dst as *mut u32) = out;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{blend_masked_row_4bpp, blend_span_alpha_4bpp, blend_word, source_word};
        use crate::color::{Color, PixelFormat};

        /// 4bpp 格式的 `(格式, opaque_alpha)` 组合。
        const FORMATS: [(PixelFormat, u8); 4] = [
            (PixelFormat::Xrgb8888, 0),
            (PixelFormat::Rgba8888, 255),
            (PixelFormat::Bgra8888, 255),
            (PixelFormat::Argb8888, 255),
        ];

        /// 逐槽对照参考实现：全 alpha 扫描 × 代表性字节组合。
        ///
        /// 4 个槽填入互不相同的字节，一次调用即可同时校验 4 条通道，
        /// 也能暴露「某一槽的结果被相邻槽污染」这类错误。
        #[test]
        fn blend_word_matches_blend_channel_over_all_alphas() {
            const SAMPLES: [u8; 11] = [0, 1, 2, 63, 64, 127, 128, 129, 192, 254, 255];

            for alpha in 1u8..=254 {
                let inv = 255 - alpha as u16;
                for s in SAMPLES {
                    for d in SAMPLES {
                        let src_bytes =
                            [s, s.wrapping_add(1), s.wrapping_add(2), s.wrapping_add(3)];
                        let dst_bytes =
                            [d, d.wrapping_add(1), d.wrapping_add(2), d.wrapping_add(3)];
                        let got = blend_word(
                            u32::from_le_bytes(dst_bytes),
                            u32::from_le_bytes(src_bytes),
                            alpha,
                        )
                        .to_le_bytes();

                        for lane in 0..4 {
                            assert_eq!(
                                got[lane],
                                PixelFormat::blend_channel(
                                    src_bytes[lane],
                                    dst_bytes[lane],
                                    alpha as u16,
                                    inv
                                ),
                                "alpha={alpha} src={} dst={} lane={lane}",
                                src_bytes[lane],
                                dst_bytes[lane]
                            );
                        }
                    }
                }
            }
        }

        /// 穷举 `(src, dst)` 的全部 65536 种字节组合（固定若干 alpha）。
        ///
        /// 这是等价性的最强形式：任意一对字节值都必须与 `blend_channel` 一致。
        #[test]
        fn blend_word_matches_blend_channel_over_all_byte_pairs() {
            for alpha in [1u8, 2, 85, 127, 128, 170, 254] {
                let inv = 255 - alpha as u16;
                for s in 0u8..=255 {
                    for d in 0u8..=255 {
                        // 高位槽填互补值，进一步验证槽间互不干扰
                        let src_bytes = [s, 255 - s, s, 255 - s];
                        let dst_bytes = [d, d, 255 - d, 255 - d];
                        let got = blend_word(
                            u32::from_le_bytes(dst_bytes),
                            u32::from_le_bytes(src_bytes),
                            alpha,
                        )
                        .to_le_bytes();

                        for lane in 0..4 {
                            assert_eq!(
                                got[lane],
                                PixelFormat::blend_channel(
                                    src_bytes[lane],
                                    dst_bytes[lane],
                                    alpha as u16,
                                    inv
                                ),
                                "alpha={alpha} src={s} dst={d} lane={lane}"
                            );
                        }
                    }
                }
            }
        }

        /// `source_word` 的字节序必须跟随 `is_rgb_order`，且第 4 字节填 `opaque_alpha`。
        #[test]
        fn source_word_follows_byte_order() {
            assert_eq!(
                source_word(0x12, 0x34, 0x56, 0x78, true),
                0x78_56_34_12,
                "RGB 序应把 R 放在最低字节"
            );
            assert_eq!(
                source_word(0x12, 0x34, 0x56, 0x78, false),
                0x78_12_34_56,
                "BGR 序应把 B 放在最低字节"
            );
        }

        /// 掩码行混合必须与 [`PixelFormat::write_color`] 参考逐字节一致。
        ///
        /// 掩码交替取 0 / 255 / 中间值，覆盖「跳过」「不透明直写」「混合」
        /// 三条分支；`mask == 0` 的像素连第 4 字节都不应被改动。
        #[test]
        fn blend_masked_row_matches_write_color_reference() {
            const WIDTH: usize = 9;
            const MASK: [u8; WIDTH] = [0, 255, 1, 128, 254, 0, 255, 77, 200];
            let color = Color::new(0x12, 0x34, 0x56);

            // 用 u32 数组承载，保证行首 4 字节对齐（内核按 u32 读写）
            let mut storage = [0u32; WIDTH + 2];
            // SAFETY: storage 是有效的 u32 数组，字节视图长度为其 4 倍。
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(storage.as_mut_ptr() as *mut u8, storage.len() * 4)
            };

            for (format, opaque_alpha) in FORMATS {
                let is_rgb_order = format.red_shift() == 0;
                // 目标像素种入确定性的非平凡值，第 4 字节置非零以验证会被覆写
                let mut original = [0u8; WIDTH * 4];
                for x in 0..WIDTH {
                    let seed = [
                        (x * 37 + 11) as u8,
                        (x * 91 + 5) as u8,
                        (x * 13 + 200) as u8,
                        0x7F,
                    ];
                    bytes[4 + x * 4..8 + x * 4].copy_from_slice(&seed);
                    original[x * 4..x * 4 + 4].copy_from_slice(&seed);
                }

                // SAFETY: row 指向 bytes 中 4 + WIDTH * 4 字节的可写区域，
                // 且因来自 u32 数组而满足 4 字节对齐；MASK 有 WIDTH 个可读字节。
                unsafe {
                    blend_masked_row_4bpp(
                        bytes.as_mut_ptr().add(4),
                        MASK.as_ptr(),
                        WIDTH,
                        &color,
                        is_rgb_order,
                        opaque_alpha,
                    )
                };

                for x in 0..WIDTH {
                    let mut want = [0u8; 4];
                    want.copy_from_slice(&original[x * 4..x * 4 + 4]);
                    if MASK[x] != 0 {
                        // 掩码字节直接作为该像素的绘制 alpha
                        format.write_color(&mut want, &color.mix_alpha(MASK[x]));
                    }
                    assert_eq!(
                        &bytes[4 + x * 4..8 + x * 4],
                        &want[..],
                        "{format:?} 列 {x}（mask={}）",
                        MASK[x]
                    );
                }
            }
        }

        /// 跨度填充必须与安全的逐像素参考 [`PixelFormat::write_color`] 完全一致。
        ///
        /// 覆盖 3 种宽度形态：纯尾部（不足 4 像素）、整 4 像素分组、分组 + 尾部；
        /// 同时用首尾哨兵验证不会越界写。
        #[test]
        fn blend_span_matches_write_color_reference() {
            const MAX_WIDTH: usize = 9;
            let color = Color::with_alpha(200, 30, 90, 77);

            // 用 u32 数组承载，保证行首 4 字节对齐（SWAR 内核按 u32 读写）
            let mut storage = [0xA5A5_A5A5u32; MAX_WIDTH + 2];
            // SAFETY: storage 是有效的 u32 数组，字节视图长度为其 4 倍。
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(storage.as_mut_ptr() as *mut u8, storage.len() * 4)
            };

            for width in [1usize, 3, 4, 5, 8, 9] {
                for (format, opaque_alpha) in FORMATS {
                    let is_rgb_order = format.red_shift() == 0;

                    // 目标像素种入确定性的非平凡值，避免「零缓冲恰好正确」
                    let mut original = [0u8; MAX_WIDTH * 4];
                    for x in 0..width {
                        let seed = [
                            (x * 37 + 11) as u8,
                            (x * 91 + 5) as u8,
                            (x * 13 + 200) as u8,
                            0x7F,
                        ];
                        bytes[4 + x * 4..8 + x * 4].copy_from_slice(&seed);
                        original[x * 4..x * 4 + 4].copy_from_slice(&seed);
                    }
                    let head = u32::from_ne_bytes(bytes[0..4].try_into().expect("4 字节"));
                    let tail = u32::from_ne_bytes(
                        bytes[(width + 1) * 4..(width + 2) * 4]
                            .try_into()
                            .expect("4 字节"),
                    );

                    // SAFETY: row 指向 bytes 中 4 + width * 4 字节的可写区域，
                    // 且因来自 u32 数组而满足 4 字节对齐。
                    unsafe {
                        blend_span_alpha_4bpp(
                            bytes.as_mut_ptr().add(4),
                            width,
                            &color,
                            is_rgb_order,
                            opaque_alpha,
                        )
                    };

                    for x in 0..width {
                        let mut want = [0u8; 4];
                        want.copy_from_slice(&original[x * 4..x * 4 + 4]);
                        format.write_color(&mut want, &color);
                        assert_eq!(
                            &bytes[4 + x * 4..8 + x * 4],
                            &want[..],
                            "width={width} {format:?} 列 {x}"
                        );
                    }
                    assert_eq!(
                        u32::from_ne_bytes(bytes[0..4].try_into().expect("4 字节")),
                        head,
                        "width={width} 行首哨兵被改写"
                    );
                    assert_eq!(
                        u32::from_ne_bytes(
                            bytes[(width + 1) * 4..(width + 2) * 4]
                                .try_into()
                                .expect("4 字节")
                        ),
                        tail,
                        "width={width} 行尾哨兵被改写"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Color, PixelFormat};
    use crate::error::ColorParseError;

    #[test]
    fn from_hex_parses_rgb_and_defaults_alpha_to_opaque() {
        assert_eq!(Color::from_hex("#123456"), Ok(Color::new(0x12, 0x34, 0x56)));
    }

    #[test]
    fn from_hex_parses_rgba() {
        assert_eq!(
            Color::from_hex("#12345678"),
            Ok(Color::with_alpha(0x12, 0x34, 0x56, 0x78))
        );
    }

    #[test]
    fn from_hex_rejects_invalid_length() {
        assert_eq!(
            Color::from_hex("#12345"),
            Err(ColorParseError::InvalidLength)
        );
    }

    #[test]
    fn from_hex_rejects_invalid_digits() {
        assert_eq!(
            Color::from_hex("#12GG56"),
            Err(ColorParseError::InvalidDigit)
        );
    }

    #[test]
    fn rgba_u32_roundtrip_preserves_alpha() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);

        assert_eq!(color.to_rgba_u32(), 0x12345678);
        assert_eq!(Color::from_rgba_u32(0x12345678), color);
    }

    #[test]
    fn rgb24_roundtrip_preserves_rgb() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
        let bytes = color.to_bytes(PixelFormat::Rgb24);

        assert_eq!(&bytes[..3], &[0x12, 0x34, 0x56]);
        assert_eq!(
            Color::from_bytes(PixelFormat::Rgb24, &bytes[..3]),
            Color::new(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn bgr24_roundtrip_preserves_rgb() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
        let bytes = color.to_bytes(PixelFormat::Bgr24);

        assert_eq!(&bytes[..3], &[0x56, 0x34, 0x12]);
        assert_eq!(
            Color::from_bytes(PixelFormat::Bgr24, &bytes[..3]),
            Color::new(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn argb8888_roundtrip_preserves_rgba() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
        let bytes = color.to_bytes(PixelFormat::Argb8888);

        assert_eq!(bytes, [0x56, 0x34, 0x12, 0x78]);
        assert_eq!(Color::from_bytes(PixelFormat::Argb8888, &bytes), color);
    }

    #[test]
    fn rgba8888_roundtrip_preserves_rgba() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
        let bytes = color.to_bytes(PixelFormat::Rgba8888);

        assert_eq!(bytes, [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(Color::from_bytes(PixelFormat::Rgba8888, &bytes), color);
    }

    #[test]
    fn bgra8888_roundtrip_preserves_rgba() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
        let bytes = color.to_bytes(PixelFormat::Bgra8888);

        assert_eq!(bytes, [0x56, 0x34, 0x12, 0x78]);
        assert_eq!(Color::from_bytes(PixelFormat::Bgra8888, &bytes), color);
    }

    #[test]
    fn xrgb8888_discards_alpha_on_decode() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
        let bytes = color.to_bytes(PixelFormat::Xrgb8888);

        assert_eq!(bytes, [0x56, 0x34, 0x12, 0x00]);
        assert_eq!(
            Color::from_bytes(PixelFormat::Xrgb8888, &bytes),
            Color::new(0x12, 0x34, 0x56)
        );
    }
}
