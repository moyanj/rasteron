use crate::{Pixel, Renderer, color::Color};

#[cfg(feature = "font_bitmap")]
mod font_8x16;
#[cfg(feature = "font_truetype")]
pub mod ttf;
#[cfg(feature = "font_bitmap")]
use font_8x16::DEFAULT_FONT;

// Hand-drawn 1bpp replacement glyph: a framed X, expanded at compile time.
const UNKNOWN_BITMAP: [u8; 16] = [
    0xFF, 0x81, 0xC3, 0xA5, 0x99, 0x99, 0xA5, 0xC3, 0xC3, 0xA5, 0x99, 0x99, 0xA5, 0xC3, 0x81, 0xFF,
];

const fn expand_unknown_bitmap() -> [u8; 128] {
    let mut expanded = [0u8; 128];
    let mut row = 0;
    while row < 16 {
        let mut col = 0;
        while col < 8 {
            expanded[row * 8 + col] = if UNKNOWN_BITMAP[row] & (0x80 >> col) != 0 {
                255
            } else {
                0
            };
            col += 1;
        }
        row += 1;
    }
    expanded
}

static UNKNOWN_MASK: [u8; 128] = expand_unknown_bitmap();

/// 无字体实现能提供该字符时使用的手绘替代字形。
pub static UNKNOWN_CHAR: Char<'static> = Char::new(&UNKNOWN_MASK, (8, 16));

#[derive(Clone, Copy)]
pub struct Char<'a> {
    size: (usize, usize),
    /// 每个字节代表一个像素的不透明度（0=透明，255=完全不透明）。
    mask: &'a [u8],
}

impl<'a> Char<'a> {
    pub const fn new(mask: &'a [u8], size: (usize, usize)) -> Self {
        Char { size, mask }
    }
}

pub trait Font {
    /// 获取字符的 8bpp Alpha 掩码。
    fn get_char(&self, c: char) -> Option<Char<'_>>;
}

impl<'a> Renderer<'a> {
    /// 绘制一个 8bpp Alpha 字符掩码。
    pub fn draw_char(&mut self, p: Pixel, c: char, color: &Color, font: Option<&dyn Font>) {
        let glyph = if let Some(font) = font {
            font.get_char(c)
        } else {
            #[cfg(feature = "font_bitmap")]
            {
                DEFAULT_FONT.get_char(c)
            }
            #[cfg(not(feature = "font_bitmap"))]
            {
                None
            }
        };
        let char = glyph.unwrap_or(UNKNOWN_CHAR);
        if char.size.0 == 0 || char.size.1 == 0 {
            return;
        }
        self.blit_mask(p, char.size.0, char.size.1, char.mask, char.size.0, *color);
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::Renderer;
    use crate::{
        Pixel,
        color::Color,
        framebuffer::{FramebufferInfo, PixelFormat},
    };
    use alloc::vec;

    #[test]
    fn draw_char_uses_font_alpha_mask() {
        let info = FramebufferInfo::new(
            8,
            16,
            8 * PixelFormat::Rgb24.bytes_per_pixel(),
            PixelFormat::Rgb24,
        );
        let mut framebuffer = vec![0u8; info.buffer_size()];
        let mut renderer = Renderer::new(info, &mut framebuffer);
        renderer.draw_char(Pixel::new(0, 0), 'A', &Color::new(255, 255, 255), None);

        assert_eq!(renderer.get_pixel(0, 2), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(3, 2), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(0, 3), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(2, 3), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn default_font_reports_missing_non_ascii_glyph() {
        use super::Font;
        use crate::font::font_8x16::DEFAULT_FONT;

        assert!(DEFAULT_FONT.get_char('中').is_none());
    }

    #[test]
    fn unknown_char_is_independent_of_default_font() {
        use super::{Font, UNKNOWN_CHAR};
        use crate::font::font_8x16::DEFAULT_FONT;

        assert_eq!(UNKNOWN_CHAR.size, (8, 16));
        assert_ne!(UNKNOWN_CHAR.mask, DEFAULT_FONT.get_char('?').unwrap().mask);
    }
}
