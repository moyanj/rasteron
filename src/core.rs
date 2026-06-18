use crate::color::Color;
use crate::framebuffer::FramebufferInfo;
use crate::pixel::Pixel;
use crate::utils::clip_line;
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

    #[inline(always)]
    pub fn pixel_size(&self) -> u16 {
        self.info.bpp / 8
    }

    #[inline(always)]
    pub fn pixel_offset(&self, x: usize, y: usize) -> usize {
        y * self.info.pitch + x * self.pixel_size() as usize
    }

    /// 绘制一个像素
    pub unsafe fn set_pixel_unchecked(&mut self, x: usize, y: usize, color: Color) {
        if color.a == 0 {
            // 完全透明，跳过绘制
            return;
        }
        let format = self.info.format;
        let pixel_size = format.bytes_per_pixel();
        let offset = self.pixel_offset(x, y);

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
        unsafe { self.set_pixel_unchecked(x, y, color) };
    }

    #[inline(always)]
    pub unsafe fn set_pixel_at_unchecked(&mut self, pixel: Pixel, color: Color) {
        unsafe { self.set_pixel_unchecked(pixel.x, pixel.y, color) };
    }

    #[inline(always)]
    pub fn set_pixel_at(&mut self, pixel: Pixel, color: Color) {
        self.set_pixel(pixel.x, pixel.y, color);
    }

    pub unsafe fn get_pixel_unchecked(&self, x: usize, y: usize) -> Color {
        let offset = self.pixel_offset(x, y);
        let format = self.info.format;
        let target = self.current_buffer();
        let src = &target[offset..offset + format.bytes_per_pixel()];
        Color::from_bytes(format, src)
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= self.info.width || y >= self.info.height {
            return None;
        }
        Some(unsafe { self.get_pixel_unchecked(x, y) })
    }

    #[inline(always)]
    pub unsafe fn get_pixel_at_unchecked(&self, pixel: Pixel) -> Color {
        unsafe { self.get_pixel_unchecked(pixel.x, pixel.y) }
    }

    #[inline(always)]
    pub fn get_pixel_at(&self, pixel: Pixel) -> Option<Color> {
        self.get_pixel(pixel.x, pixel.y)
    }

    pub fn clear(&mut self, color: Color) {
        let format = self.info.format;
        let pixel_size = format.bytes_per_pixel();
        let width = self.info.width;
        let height = self.info.height;
        let row_len = width * pixel_size;

        if row_len == 0 || height == 0 {
            return;
        }

        let bytes = color.to_bytes(format);
        unsafe { self.clear_unchecked(&bytes[..pixel_size], row_len, height) };
    }

    /// 用重复像素模式填充当前缓冲区的可见区域。
    ///
    /// # Safety
    ///
    /// 调用方必须保证 `pattern.len()` 与当前像素大小一致，且 `row_len > 0`。
    unsafe fn clear_unchecked(&mut self, pattern: &[u8], row_len: usize, height: usize) {
        let pitch = self.info.pitch;
        let pixel_size = pattern.len();
        let buffer = self.current_buffer_mut();
        let base_ptr = buffer.as_mut_ptr();

        for y in 0..height {
            let row_ptr = unsafe { base_ptr.add(y * pitch) };

            unsafe {
                core::ptr::copy_nonoverlapping(pattern.as_ptr(), row_ptr, pixel_size);
            }

            let mut filled = pixel_size;
            while filled < row_len {
                let copy_len = core::cmp::min(filled, row_len - filled);
                unsafe {
                    core::ptr::copy(row_ptr, row_ptr.add(filled), copy_len);
                }
                filled += copy_len;
            }
        }
    }

    /// Draw a line
    pub fn draw_line(&mut self, p1: Pixel, p2: Pixel, color: Color) {
        let cliped = clip_line(p1, p2, self.info.width, self.info.height);
        if let Some((p1, p2)) = cliped {
            unsafe {
                self.draw_line_unchecked(p1, p2, color);
            }
        }
        return;
    }

    /// Draw a line without per-pixel bounds checks.
    ///
    /// # Safety
    ///
    /// 调用方必须保证整条线上的所有像素都落在当前 framebuffer 可见区域内。
    pub unsafe fn draw_line_unchecked(&mut self, p1: Pixel, p2: Pixel, color: Color) {
        let dx_abs = (p2.x as i64 - p1.x as i64).unsigned_abs();
        let dy_abs = (p2.y as i64 - p1.y as i64).unsigned_abs();
        let steep = dy_abs > dx_abs;
        let (mut x1, mut y1) = p1.to_coord();
        let (mut x2, mut y2) = p2.to_coord();
        if steep {
            core::mem::swap(&mut x1, &mut y1);
            core::mem::swap(&mut x2, &mut y2);
        }
        if x1 > x2 {
            core::mem::swap(&mut x1, &mut x2);
            core::mem::swap(&mut y1, &mut y2);
        }
        let dx = x2 - x1;
        let dy = (y2 as i64 - y1 as i64).unsigned_abs();
        let mut error = (dx / 2) as i64;
        let y_step = if y1 < y2 { 1 } else { -1 };
        let mut y = y1 as i64;
        for x in x1..=x2 {
            if steep {
                self.set_pixel(y as usize, x, color);
            } else {
                self.set_pixel(x, y as usize, color);
            }
            error -= dy as i64;
            if error < 0 {
                y += y_step;
                error += dx as i64;
            }
        }
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
    use crate::pixel::Pixel;

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

    #[test]
    fn clear_fills_bgr24_rows() {
        let info = FramebufferInfo::new(2, 2, 6, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 12];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.clear(Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(
            renderer.buffer_mut(),
            &[
                0x56, 0x34, 0x12, 0x56, 0x34, 0x12, 0x56, 0x34, 0x12, 0x56, 0x34, 0x12
            ]
        );
    }

    #[test]
    fn clear_respects_pitch_padding() {
        let info = FramebufferInfo::new(2, 1, 8, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 8];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.clear(Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(
            renderer.buffer_mut(),
            &[0x56, 0x34, 0x12, 0x56, 0x34, 0x12, 0x00, 0x00]
        );
    }

    #[test]
    fn pixel_api_wraps_coordinate_api() {
        let info = FramebufferInfo::new(2, 1, 6, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 6];
        let mut renderer = Renderer::new(info, &mut front);
        let pixel = Pixel::new(1, 0);

        renderer.set_pixel_at(pixel, Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(
            renderer.get_pixel_at(pixel),
            Some(Color::new(0x12, 0x34, 0x56))
        );
    }

    #[test]
    fn draw_line_renders_diagonal() {
        let info = FramebufferInfo::new(3, 3, 9, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 27];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.draw_line(
            Pixel::new(0, 0),
            Pixel::new(2, 2),
            Color::new(0x12, 0x34, 0x56),
        );

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0x12, 0x34, 0x56)));
        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(0x12, 0x34, 0x56)));
        assert_eq!(renderer.get_pixel(2, 2), Some(Color::new(0x12, 0x34, 0x56)));
    }

    #[test]
    fn draw_line_unchecked_matches_safe_path() {
        let mut front_safe = [0u8; 48];
        let mut front_unchecked = [0u8; 48];
        let mut safe = Renderer::new(
            FramebufferInfo::new(4, 4, 12, 24, PixelFormat::Bgr24),
            &mut front_safe,
        );
        let mut unchecked = Renderer::new(
            FramebufferInfo::new(4, 4, 12, 24, PixelFormat::Bgr24),
            &mut front_unchecked,
        );
        let start = Pixel::new(0, 1);
        let end = Pixel::new(3, 2);
        let color = Color::new(0x12, 0x34, 0x56);

        safe.draw_line(start, end, color);
        unsafe { unchecked.draw_line_unchecked(start, end, color) };

        assert_eq!(safe.buffer_mut(), unchecked.buffer_mut());
    }

    #[test]
    fn draw_line_clips_right_edge() {
        let info = FramebufferInfo::new(4, 2, 12, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 24];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x12, 0x34, 0x56);

        renderer.draw_line(Pixel::new(2, 0), Pixel::new(6, 0), color);

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(2, 0), Some(color));
        assert_eq!(renderer.get_pixel(3, 0), Some(color));
    }
}
