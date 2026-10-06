use fontdue;

use crate::font::{Char, Font};

pub struct TtfFont {
    font: fontdue::Font,
}

impl TtfFont {
    pub fn new(font: &[u8]) -> Self {
        Self {
            font: fontdue::Font::from_bytes(font, fontdue::FontSettings::default()).unwrap(),
        }
    }
}

impl Font for TtfFont {
    fn get_char(&self, c: char) -> Option<Char<'_>> {
        None
    }
}
