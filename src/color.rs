#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// Creates a new Color with the specified RGB values and default alpha (255)
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates a new Color with the specified RGBA values
    pub const fn with_alpha(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

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

    pub fn to_rgba_u32(&self) -> u32 {
        let r = self.r as u32;
        let g = self.g as u32;
        let b = self.b as u32;
        let a = self.a as u32;
        (r << 24) | (g << 16) | (b << 8) | a
    }

    pub fn from_rgba_u32(color: u32) -> Self {
        let r = ((color >> 24) & 0xFF) as u8;
        let g = ((color >> 16) & 0xFF) as u8;
        let b = ((color >> 8) & 0xFF) as u8;
        let a = (color & 0xFF) as u8;
        Self::with_alpha(r, g, b, a)
    }

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

    pub fn from_bytes(format: PixelFormat, bytes: &[u8]) -> Self {
        assert!(
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

    pub fn mix_alpha(&self, alpha: u8) -> Self {
        Self::with_alpha(self.r, self.g, self.b, alpha)
    }

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
    pub fn invert(&self) -> Color {
        Color::new(255 - self.r, 255 - self.g, 255 - self.b)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorParseError {
    InvalidLength,
    InvalidDigit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb24,
    Bgr24,
    Xrgb8888,
    Argb8888,
    Rgba8888,
    Bgra8888,
}

impl PixelFormat {
    pub const fn red_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 0,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 16,
        }
    }

    pub const fn green_shift(self) -> u8 {
        8
    }

    pub const fn blue_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 16,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 0,
        }
    }

    pub const fn alpha_shift(self) -> Option<u8> {
        match self {
            PixelFormat::Argb8888 | PixelFormat::Rgba8888 | PixelFormat::Bgra8888 => Some(24),
            PixelFormat::Rgb24 | PixelFormat::Bgr24 | PixelFormat::Xrgb8888 => None,
        }
    }

    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => 3,
            PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Rgba8888
            | PixelFormat::Bgra8888 => 4,
        }
    }

    pub const fn bits_per_pixel(self) -> usize {
        self.bytes_per_pixel() * 8
    }
}

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

// 基础颜色
pub const BLACK: Color = color!(0, 0, 0);
pub const WHITE: Color = color!(255, 255, 255);
pub const RED: Color = color!(255, 0, 0);
pub const GREEN: Color = color!(0, 255, 0);
pub const BLUE: Color = color!(0, 0, 255);
// 实用混合色
pub const YELLOW: Color = color!(255, 255, 0); // 红+绿
pub const CYAN: Color = color!(0, 255, 255); // 绿+蓝
pub const MAGENTA: Color = color!(255, 0, 255); // 红+蓝
// 中性色
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
        assert_eq!(Color::from_hex("#12345"), Err(ColorParseError::InvalidLength));
    }

    #[test]
    fn from_hex_rejects_invalid_digits() {
        assert_eq!(Color::from_hex("#12GG56"), Err(ColorParseError::InvalidDigit));
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
