use crate::color::Color;
use crate::framebuffer::FramebufferInfo;
extern crate alloc;
use alloc::vec::Vec;

pub struct Renderer<'a> {
    info: FramebufferInfo,
    front_buffer: &'a mut [u8],
    #[cfg(feature = "double_buffer")]
    back_buffer: Vec<u8>,
}

impl<'a> Renderer<'a> {
    /// 创建渲染器
    pub fn new(info: FramebufferInfo, front_buffer: &'a mut [u8]) -> Self {
        #[cfg(feature = "double_buffer")]
        let buffer_size = info.buffer_size();
        Self {
            info,
            front_buffer,
            #[cfg(feature = "double_buffer")]
            back_buffer: alloc::vec![0; buffer_size],
        }
    }

    /// 获取当前可写入的缓冲区切片（内部使用）
    #[inline(always)]
    fn current_buffer_mut(&mut self) -> &mut [u8] {
        #[cfg(feature = "double_buffer")]
        return &mut self.back_buffer;
        #[cfg(not(feature = "double_buffer"))]
        return &mut self.front_buffer;
    }

    #[inline(always)]
    fn current_buffer(&self) -> &[u8] {
        #[cfg(feature = "double_buffer")]
        return &self.back_buffer;
        #[cfg(not(feature = "double_buffer"))]
        return &self.front_buffer;
    }

    /// 绘制一个像素
    pub unsafe fn set_pixel_raw_unchecked(&mut self, x: usize, y: usize, color: Color) {
        let offset = self.info.pixel_offset(x, y);

        if color.a == 0 {
            // 完全透明，跳过绘制
            return;
        }
        let format = self.info.format;
        let pixel_size = format.bytes_per_pixel();

        let bytes = if color.a == 255 {
            color.to_bytes(format)
        } else {
            // 半透明，进行 alpha 混合
            let target = self.current_buffer_mut();
            let dst = &mut target[offset..offset + pixel_size];
            let existing_color = Color::from_bytes(format, dst);
            let blended = color.mix(&existing_color, color.a);
            blended.to_bytes(format)
        };

        unsafe {
            let dst_ptr = self.back_buffer.as_mut_ptr().add(offset);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst_ptr, pixel_size);
        }
    }

    #[inline(always)]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.info.width || y >= self.info.height {
            return;
        }
        unsafe { self.set_pixel_raw_unchecked(x, y, color) };
    }

    pub unsafe fn get_pixel_raw_unchecked(&self, x: usize, y: usize) -> Color {
        let offset = self.info.pixel_offset(x, y);
        let format = self.info.format;
        let target = self.current_buffer();
        let src = &target[offset..offset + format.bytes_per_pixel()];
        Color::from_bytes(format, src)
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= self.info.width || y >= self.info.height {
            return None;
        }
        Some(unsafe { self.get_pixel_raw_unchecked(x, y) })
    }

    /// 如果启用双缓冲，将后台缓冲区交换到前台（即刷新到显存）
    #[cfg(feature = "double_buffer")]
    pub fn swap_buffers(&mut self) {
        // 直接拷贝整个后台到前台，或使用 DMA（此处留接口）
        self.front_buffer.copy_from_slice(&self.back_buffer);
    }

    /// 可选：绘制大批量像素时，直接获取当前缓冲区指针，减少调用开销
    #[inline(always)]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.current_buffer_mut()
    }

    pub fn info(&self) -> &FramebufferInfo {
        &self.info
    }
}

#[cfg(test)]
mod tests {
    use super::Renderer;
    use crate::color::{Color, PixelFormat};
    use crate::framebuffer::FramebufferInfo;

    #[test]
    fn set_pixel_uses_bgr24_format() {
        let info = FramebufferInfo::new(2, 1, 6, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 6];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(1, 0, Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(&renderer.buffer_mut()[3..6], &[0x56, 0x34, 0x12]);
    }

    #[test]
    fn set_pixel_uses_rgba8888_format() {
        let info = FramebufferInfo::new(1, 1, 4, 32, PixelFormat::Rgba8888);
        let mut front = [0u8; 4];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(&renderer.buffer_mut()[..4], &[0x12, 0x34, 0x56, 0xFF]);
    }

    #[test]
    fn set_pixel_alpha_blends_into_bgr24_buffer() {
        let info = FramebufferInfo::new(1, 1, 3, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 3];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, Color::new(10, 20, 30));
        renderer.set_pixel(0, 0, Color::with_alpha(110, 120, 130, 128));

        assert_eq!(&renderer.buffer_mut()[..3], &[80, 70, 60]);
    }

    #[test]
    fn set_pixel_alpha_blends_into_rgba8888_buffer() {
        let info = FramebufferInfo::new(1, 1, 4, 32, PixelFormat::Rgba8888);
        let mut front = [0u8; 4];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, Color::with_alpha(20, 40, 60, 255));
        renderer.set_pixel(0, 0, Color::with_alpha(120, 140, 160, 128));

        assert_eq!(&renderer.buffer_mut()[..4], &[70, 90, 110, 255]);
    }

    #[test]
    fn set_pixel_uses_shift_based_xrgb8888_layout() {
        let info = FramebufferInfo::new(1, 1, 4, 32, PixelFormat::Xrgb8888);
        let mut front = [0u8; 4];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(&renderer.buffer_mut()[..4], &[0x56, 0x34, 0x12, 0x00]);
    }
}
