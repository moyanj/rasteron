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

    /// 将颜色按指定像素格式编码为字节数组。
    ///
    /// 返回一个 4 字节数组；对于 3 字节格式，最后一个字节未使用；对于
    /// 2 字节的 565 格式，高两字节恒为 0，有效字节按该格式的存储字节序排列
    /// （`Rgb565` 小端、`Bgr565` 大端）。565 的通道会被量化到 5/6/5 位。
    #[inline(always)]
    pub fn to_bytes(&self, format: PixelFormat) -> [u8; 4] {
        // 565 的通道不足 8 位，无法用「8 位通道左移 shift」的通用写法表达。
        if format.bytes_per_pixel() == 2 {
            let bytes = encode_565_bytes(pack_565(self), format.stores_bytes_swapped());
            return [bytes[0], bytes[1], 0, 0];
        }

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
    /// 565 格式会把 5/6/5 位字段扩展回 8 位（位复制），因此解出的通道值不是
    /// 编码前的任意 8 位值，而是「量化后再扩展」的规范值。
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

        if format.bytes_per_pixel() == 2 {
            return unpack_565(decode_565_bytes(bytes, format.stores_bytes_swapped()));
        }

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
        let channel =
            |src: u8, dst: u8| PixelFormat::blend_channel(src, dst, alpha as u16, inv_alpha as u16);
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
/// | `Rgb565` | 2 | 5/6/5 位打包（小端序存储），无 alpha |
/// | `Bgr565` | 2 | 同 `Rgb565` 的位域，但两个存储字节交换 |
///
/// 前六种格式的通道都是 8 位且按字节对齐，通道位置由
/// [`red_shift`](PixelFormat::red_shift) 等以**位**为单位描述（对它们而言恰好
/// 等于字节位移 × 8）。`Rgb565` / `Bgr565` 的通道宽度分别是 5/6/5 位，故写入时
/// 需要量化、读取时需要扩展，由 [`Color::to_bytes`] / [`Color::read_color`] 等
/// 入口统一处理。
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
    /// 2 字节 5/6/5 打包：位域 `R = 15..11`、`G = 10..5`、`B = 4..0`，
    /// 小端序存储（字节 0 = `G[2:0] << 5 | B`，字节 1 = `R << 3 | G[5:3]`）。
    ///
    /// 无 alpha 通道。写入时 8 位通道量化到 5/6/5 位，读取时位复制扩展回 8 位。
    Rgb565,
    /// 与 [`PixelFormat::Rgb565`] 位域完全相同的 5/6/5 打包，但两个存储字节
    /// 顺序相反（字节 0 = `R << 3 | G[5:3]`，字节 1 = `G[2:0] << 5 | B`），
    /// 即面板厂商常称的 **RGB565_BE**。
    ///
    /// **注意**：本变体只交换字节序，通道位域仍是 R 高 5 位——这与 Linux DRM 的
    /// `DRM_FORMAT_BGR565`（把 B 放到高 5 位）**不是**同一回事。
    Bgr565,
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
            // 565 没有 alpha 字节，该值只是穷举占位（bpp != 4，永不被读取）。
            PixelFormat::Rgb24
            | PixelFormat::Bgr24
            | PixelFormat::Argb8888
            | PixelFormat::Rgba8888
            | PixelFormat::Bgra8888
            | PixelFormat::Rgb565
            | PixelFormat::Bgr565 => 255,
        }
    }

    /// 红色通道在 32 位小端字中的位移（**以位为单位**）。
    ///
    /// 例如 BGR 布局下 red 在高 16 位，位移为 16；`Rgb565` 的 red 在 15..11 位，
    /// 位移为 11。对 8 位通道的格式，该值同时是字节位移 × 8。
    #[inline(always)]
    pub const fn red_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 0,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 16,
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => 11,
        }
    }

    /// 绿色通道的位移（8 位格式均为 8，565 为 5）。
    #[inline(always)]
    pub const fn green_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => 5,
            PixelFormat::Rgb24
            | PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Rgba8888
            | PixelFormat::Bgra8888 => 8,
        }
    }

    /// 蓝色通道在 32 位小端字中的位移（以位为单位）。
    #[inline(always)]
    pub const fn blue_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 16,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 0,
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => 0,
        }
    }

    /// Alpha 通道的位移（如果格式支持）。
    ///
    /// 对于不含 alpha 的格式（`Rgb24`、`Bgr24`、`Xrgb8888`、`Rgb565`、`Bgr565`）
    /// 返回 `None`。
    #[inline(always)]
    pub const fn alpha_shift(self) -> Option<u8> {
        match self {
            PixelFormat::Argb8888 | PixelFormat::Rgba8888 | PixelFormat::Bgra8888 => Some(24),
            PixelFormat::Rgb24
            | PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Rgb565
            | PixelFormat::Bgr565 => None,
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
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => 2,
        }
    }

    /// 16 位打包格式是否按「高字节在前」存储（即 `Rgb565` 的字节交换版本）。
    ///
    /// 只对 [`bytes_per_pixel`](PixelFormat::bytes_per_pixel) 为 2 的格式有意义：
    /// 8 位通道格式的字节顺序已由各自的 shift 完整表达，故恒为 `false`。
    #[inline(always)]
    pub(crate) const fn stores_bytes_swapped(self) -> bool {
        matches!(self, PixelFormat::Bgr565)
    }

    /// 每个像素占用的位数。
    pub const fn bits_per_pixel(self) -> usize {
        self.bytes_per_pixel() * 8
    }

    /// 将颜色写入目标缓冲区（支持 Alpha 混合）。
    ///
    /// 当 `color.a == 255` 时直接覆盖；否则使用 Alpha 混合。
    ///
    /// 对 565 格式，混合语义是「把目标解码到 8 位 → 用
    /// [`blend_channel`](PixelFormat::blend_channel) 混合 → 量化回 565」，
    /// 与其余格式共用同一套参考公式，误差只来自量化。
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
                // 565 无 alpha 通道，不透明写入即「量化打包」，无需读取目标像素。
                PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                    let bytes = encode_565_bytes(pack_565(color), self.stores_bytes_swapped());
                    dst[0] = bytes[0];
                    dst[1] = bytes[1];
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
            // 565 就地混合：解码 → blend_channel → 量化回写。
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                let swapped = self.stores_bytes_swapped();
                let word = decode_565_bytes(dst, swapped);
                let bytes = encode_565_bytes(blend_565(word, color), swapped);
                dst[0] = bytes[0];
                dst[1] = bytes[1];
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
            // 565：按存储字节序取字，再把 5/6/5 字段位复制扩展回 8 位。
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                unpack_565(decode_565_bytes(src, self.stores_bytes_swapped()))
            }
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

// ———————————————————————————————— 5/6/5 打包格式 ————————————————————————————————

/// 把 8 位通道量化到 `bits` 位，四舍五入（`(v × max + 127) / 255`）。
///
/// `bits ∈ {5, 6, 8}`；`bits == 8` 时结果恒等于输入。取值上界：
/// `(255 × 63 + 127) / 255 = 63`，不会溢出 `u16`。
///
/// 与 [`expand_channel`] 互逆：对任意 `v < 2^bits` 有
/// `quantize_channel(expand_channel(v, bits), bits) == v`。
#[inline(always)]
pub(crate) const fn quantize_channel(value: u8, bits: u8) -> u16 {
    let max = (1u32 << bits) - 1;
    ((value as u32 * max + 127) / 255) as u16
}

/// 把 `bits` 位的字段扩展回 8 位（位复制填充低位）。
///
/// 例：5 位 `v` → `(v << 3) | (v >> 2)`；6 位 `v` → `(v << 2) | (v >> 4)`。
#[inline(always)]
pub(crate) const fn expand_channel(value: u16, bits: u8) -> u8 {
    let shift = 8 - bits;
    ((value << shift) | (value >> (bits - shift))) as u8
}

/// 把颜色量化打包成 565 字（位域 `R = 15..11`、`G = 10..5`、`B = 4..0`）。
///
/// 不涉及字节序——字节序由 [`encode_565_bytes`] 负责。
#[inline(always)]
pub(crate) const fn pack_565(color: &Color) -> u16 {
    (quantize_channel(color.r, 5) << 11)
        | (quantize_channel(color.g, 6) << 5)
        | quantize_channel(color.b, 5)
}

/// 把 565 字展开成 8 位通道颜色（alpha 恒为 `255`）。
#[inline(always)]
pub(crate) const fn unpack_565(word: u16) -> Color {
    Color::with_alpha(
        expand_channel((word >> 11) & 0x1F, 5),
        expand_channel((word >> 5) & 0x3F, 6),
        expand_channel(word & 0x1F, 5),
        255,
    )
}

/// 565 就地混合：把目标字解码到 8 位，用 [`PixelFormat::blend_channel`] 混合，
/// 再量化回 565。
///
/// 这是 565 唯一的混合语义，与其余格式共用同一套参考公式，误差只来自量化。
/// 两个短路：`color.a == 255` 直接返回 [`pack_565`]（不读目标像素）；
/// `color.a == 0` 原样返回 `word`（与 8 位路径 `blend_channel` 的不动点一致）。
#[inline(always)]
pub(crate) fn blend_565(word: u16, color: &Color) -> u16 {
    let alpha = color.a;
    if alpha == 255 {
        return pack_565(color);
    }
    if alpha == 0 {
        return word;
    }

    let dst = unpack_565(word);
    let alpha = alpha as u16;
    let inv_alpha = 255 - alpha;
    // 先算出量化后的 5/6/5 字段，再拼字：与 `pack_565` 的布局保持一致。
    let r = quantize_channel(
        PixelFormat::blend_channel(color.r, dst.r, alpha, inv_alpha),
        5,
    );
    let g = quantize_channel(
        PixelFormat::blend_channel(color.g, dst.g, alpha, inv_alpha),
        6,
    );
    let b = quantize_channel(
        PixelFormat::blend_channel(color.b, dst.b, alpha, inv_alpha),
        5,
    );
    (r << 11) | (g << 5) | b
}

/// 按存储字节序把两个字节解码成 565 字。
///
/// `swapped` 为 `false` 时按小端序（`Rgb565`），为 `true` 时按大端序（`Bgr565`）。
///
/// # 调用方保证
///
/// `src` 长度至少为 2。
#[inline(always)]
pub(crate) fn decode_565_bytes(src: &[u8], swapped: bool) -> u16 {
    debug_assert!(src.len() >= 2, "565 像素需要 2 字节");
    let pair = [src[0], src[1]];
    if swapped {
        u16::from_be_bytes(pair)
    } else {
        u16::from_le_bytes(pair)
    }
}

/// 按存储字节序把 565 字编码成两个字节（[`decode_565_bytes`] 的逆操作）。
#[inline(always)]
pub(crate) fn encode_565_bytes(word: u16, swapped: bool) -> [u8; 2] {
    if swapped {
        word.to_be_bytes()
    } else {
        word.to_le_bytes()
    }
}

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
}
