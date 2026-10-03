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

    /// 将当前颜色与另一个颜色混合，透明度由 `alpha` 控制。
    ///
    /// 使用标准 Alpha 混合公式：
    /// `result = src × alpha + dst × (255 - alpha)`
    pub fn mix(&self, other: &Color, alpha: u8) -> Self {
        let r =
            ((self.r as u16 * alpha as u16 + other.r as u16 * (255 - alpha) as u16) / 255) as u8;
        let g =
            ((self.g as u16 * alpha as u16 + other.g as u16 * (255 - alpha) as u16) / 255) as u8;
        let b =
            ((self.b as u16 * alpha as u16 + other.b as u16 * (255 - alpha) as u16) / 255) as u8;
        let a = 255;
        Self { r, g, b, a }
    }
    /// 返回颜色的反色（按位取反 RGB 通道）。
    pub fn invert(&self) -> Color {
        Color::new(!self.r, !self.g, !self.b)
    }
}

/// 解析十六进制颜色字符串时可能发生的错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorParseError {
    /// 字符串长度不是 6 或 8 个字符（不含 `#`）。
    InvalidLength,
    /// 包含无效的十六进制字符。
    InvalidDigit,
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

#[cfg(test)]
mod tests {
    use super::{Color, ColorParseError, PixelFormat};

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
