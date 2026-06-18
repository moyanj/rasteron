pub use crate::color::PixelFormat;

pub struct FramebufferInfo {
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub bpp: u16,
    pub format: PixelFormat,
}

impl FramebufferInfo {
    /// Create a new FramebufferInfo
    pub fn new(
        width: usize,
        height: usize,
        pitch: usize,
        bpp: u16,
        format: PixelFormat,
    ) -> FramebufferInfo {
        if bpp != 32 && bpp != 24 {
            panic!("Unsupported bits per pixel: {}", bpp);
        }
        FramebufferInfo {
            width: width,
            height: height,
            pitch: pitch,
            bpp: bpp,
            format: format,
        }
    }

    pub fn buffer_size(&self) -> usize {
        self.pitch * self.height
    }

    #[inline(always)]
    pub fn pixel_size(&self) -> u16 {
        self.bpp / 8
    }

    #[inline(always)]
    pub fn pixel_offset(&self, x: usize, y: usize) -> usize {
        y * self.pitch + x * self.pixel_size() as usize
    }
}
