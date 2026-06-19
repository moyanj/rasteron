pub use crate::color::PixelFormat;

pub struct FramebufferInfo {
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub format: PixelFormat,
}

impl FramebufferInfo {
    /// Create a new FramebufferInfo
    pub fn new(
        width: usize,
        height: usize,
        pitch: usize,
        format: PixelFormat,
    ) -> FramebufferInfo {
        let min_pitch = width.saturating_mul(format.bytes_per_pixel());
        assert!(
            pitch >= min_pitch,
            "pitch must be at least width * bytes_per_pixel"
        );
        FramebufferInfo {
            width: width,
            height: height,
            pitch: pitch,
            format: format,
        }
    }

    pub fn buffer_size(&self) -> usize {
        self.pitch * self.height
    }

    pub const fn bytes_per_pixel(&self) -> usize {
        self.format.bytes_per_pixel()
    }

    pub const fn bits_per_pixel(&self) -> usize {
        self.format.bits_per_pixel()
    }
}

#[cfg(test)]
mod tests {
    use super::FramebufferInfo;
    use crate::color::PixelFormat;

    #[test]
    fn derives_pixel_size_from_format() {
        let info = FramebufferInfo::new(4, 3, 16, PixelFormat::Rgba8888);

        assert_eq!(info.bytes_per_pixel(), 4);
        assert_eq!(info.bits_per_pixel(), 32);
        assert_eq!(info.buffer_size(), 48);
    }

    #[test]
    #[should_panic(expected = "pitch must be at least width * bytes_per_pixel")]
    fn rejects_pitch_smaller_than_visible_row() {
        let _ = FramebufferInfo::new(4, 1, 11, PixelFormat::Bgr24);
    }
}
