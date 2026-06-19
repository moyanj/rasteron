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
        if clip_line(p1, p2, self.info.width, self.info.height).is_none() {
            return;
        }

        unsafe { self.draw_line_unchecked(p1, p2, color) };
    }

    /// Draw a line while clipping writes against the framebuffer bounds.
    ///
    /// # Safety
    ///
    /// 调用方必须保证 renderer 绑定的 framebuffer 有效。
    pub unsafe fn draw_line_unchecked(&mut self, p1: Pixel, p2: Pixel, color: Color) {
        let (mut x1, mut y1) = (p1.x as i64, p1.y as i64);
        let (mut x2, mut y2) = (p2.x as i64, p2.y as i64);
        let dx_abs = (x2 - x1).unsigned_abs();
        let dy_abs = (y2 - y1).unsigned_abs();
        let steep = dy_abs > dx_abs;
        if steep {
            core::mem::swap(&mut x1, &mut y1);
            core::mem::swap(&mut x2, &mut y2);
        }
        if x1 > x2 {
            core::mem::swap(&mut x1, &mut x2);
            core::mem::swap(&mut y1, &mut y2);
        }
        let dx = x2 - x1;
        let dy = (y2 - y1).unsigned_abs();
        let mut error = dx / 2;
        let y_step = if y1 < y2 { 1 } else { -1 };
        let mut y = y1;
        for x in x1..=x2 {
            let (px, py) = if steep { (y, x) } else { (x, y) };
            if px >= 0
                && py >= 0
                && (px as usize) < self.info.width
                && (py as usize) < self.info.height
            {
                unsafe { self.set_pixel_unchecked(px as usize, py as usize, color) };
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

    /// 绘制三角形轮廓
    #[inline]
    pub fn draw_triangle(&mut self, p1: Pixel, p2: Pixel, p3: Pixel, color: Color) {
        self.draw_line(p1, p2, color);
        self.draw_line(p2, p3, color);
        self.draw_line(p3, p1, color);
    }

    /// 填充三角形（扫描线算法）
    ///
    /// 使用重心坐标和扫描线填充，性能敏感场景下直接操作缓冲区。
    #[inline]
    pub fn fill_triangle(&mut self, p1: Pixel, p2: Pixel, p3: Pixel, color: Color) {
        // 按 Y 坐标排序三个顶点
        let mut pts = [p1, p2, p3];
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }
        if pts[1].y > pts[2].y {
            pts.swap(1, 2);
        }
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }

        let (x1, y1) = (pts[0].x as i32, pts[0].y as i32);
        let (x2, y2) = (pts[1].x as i32, pts[1].y as i32);
        let (x3, y3) = (pts[2].x as i32, pts[2].y as i32);

        // 如果三个点的 Y 坐标相同，不绘制
        if y1 == y3 {
            return;
        }

        let width = self.info.width as i32;
        let height = self.info.height as i32;

        // 水平线填充闭包
        let mut fill_h_line = |start_x: i32, end_x: i32, y: i32| {
            if y < 0 || y >= height {
                return;
            }
            let mut sx = start_x.max(0).min(width - 1);
            let mut ex = end_x.max(0).min(width - 1);
            if sx > ex {
                core::mem::swap(&mut sx, &mut ex);
            }
            for x in sx..=ex {
                // SAFETY: sx/ex 已被裁剪到 [0, width-1]，y 已检查在 [0, height-1] 内
                unsafe { self.set_pixel_unchecked(x as usize, y as usize, color) };
            }
        };

        let long_dx = (x3 - x1) as i64;
        let long_dy = (y3 - y1) as i64;

        // 上半部分三角形 (p1 -> p2)
        let upper_dx = (x2 - x1) as i64;
        let upper_dy = (y2 - y1) as i64;
        for y in y1..=y2 {
            let dy = (y - y1) as i64;
            let x_long = if long_dy != 0 {
                (x1 as i64 + (long_dx * dy + long_dy / 2) / long_dy) as i32
            } else {
                x1
            };
            let x_upper = if upper_dy != 0 {
                (x1 as i64 + (upper_dx * dy + upper_dy / 2) / upper_dy) as i32
            } else {
                x1
            };
            fill_h_line(x_long, x_upper, y);
        }

        // 下半部分三角形 (p2 -> p3)
        let lower_dx = (x3 - x2) as i64;
        let lower_dy = (y3 - y2) as i64;
        for y in y2..=y3 {
            let dy_long = (y - y1) as i64;
            let dy_lower = (y - y2) as i64;
            let x_long = if long_dy != 0 {
                (x1 as i64 + (long_dx * dy_long + long_dy / 2) / long_dy) as i32
            } else {
                x1
            };
            let x_lower = if lower_dy != 0 {
                (x2 as i64 + (lower_dx * dy_lower + lower_dy / 2) / lower_dy) as i32
            } else {
                x2
            };
            fill_h_line(x_long, x_lower, y);
        }
    }

    /// 绘制矩形轮廓
    #[inline]
    pub fn draw_rect(&mut self, top_left: Pixel, width: usize, height: usize, color: Color) {
        let (x, y) = top_left.to_coord();
        let x2 = x + width;
        let y2 = y + height;
        self.draw_line(Pixel::new(x, y), Pixel::new(x2, y), color);
        self.draw_line(Pixel::new(x2, y), Pixel::new(x2, y2), color);
        self.draw_line(Pixel::new(x2, y2), Pixel::new(x, y2), color);
        self.draw_line(Pixel::new(x, y2), Pixel::new(x, y), color);
    }

    /// 填充矩形
    ///
    /// 性能优化版本：直接操作内存缓冲区，避免逐像素边界检查开销。
    #[inline]
    pub fn fill_rect(&mut self, top_left: Pixel, width: usize, height: usize, color: Color) {
        let (x_min, y_min) = top_left.to_coord();
        let x_max = (x_min + width).min(self.info.width);
        let y_max = (y_min + height).min(self.info.height);
        let x_start = x_min;
        let y_start = y_min;

        if x_start >= x_max || y_start >= y_max {
            return;
        }

        let format = self.info.format;
        let pixel_size = format.bytes_per_pixel();
        let bytes = color.to_bytes(format);
        let pattern = &bytes[..pixel_size];
        let pitch = self.info.pitch;

        // 优化：逐行填充，使用 copy 重复模式
        let buffer = self.current_buffer_mut();
        for y in y_start..y_max {
            let row_offset = y * pitch;
            for x in x_start..x_max {
                let offset = row_offset + x * pixel_size;
                buffer[offset..offset + pixel_size].copy_from_slice(pattern);
            }
        }
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

    #[test]
    fn draw_line_preserves_bresenham_coverage_when_end_is_below_screen() {
        let info = FramebufferInfo::new(4, 4, 12, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x12, 0x34, 0x56);

        renderer.draw_line(Pixel::new(0, 0), Pixel::new(1, 4), color);

        assert_eq!(renderer.get_pixel(0, 0), Some(color));
        assert_eq!(renderer.get_pixel(0, 1), Some(color));
        assert_eq!(renderer.get_pixel(0, 2), Some(color));
        assert_eq!(renderer.get_pixel(0, 3), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(1, 3), Some(color));
    }

    #[test]
    fn draw_triangle_renders_outline() {
        let info = FramebufferInfo::new(4, 4, 12, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0xFF, 0x00, 0x00);

        renderer.draw_triangle(Pixel::new(0, 0), Pixel::new(3, 0), Pixel::new(1, 3), color);

        // 验证三条边都被绘制
        assert_eq!(renderer.get_pixel(0, 0), Some(color));
        assert_eq!(renderer.get_pixel(1, 0), Some(color));
        assert_eq!(renderer.get_pixel(2, 0), Some(color));
        assert_eq!(renderer.get_pixel(3, 0), Some(color));
        assert_eq!(renderer.get_pixel(1, 3), Some(color));
    }

    #[test]
    fn fill_triangle_fills_interior() {
        let info = FramebufferInfo::new(5, 5, 15, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 75];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x00, 0xFF, 0x00);

        // 等边三角形
        renderer.fill_triangle(Pixel::new(2, 0), Pixel::new(0, 4), Pixel::new(4, 4), color);

        // 验证顶点被填充
        assert_eq!(renderer.get_pixel(2, 0), Some(color));
        assert_eq!(renderer.get_pixel(0, 4), Some(color));
        assert_eq!(renderer.get_pixel(4, 4), Some(color));
        // 验证底部行被填充
        assert_eq!(renderer.get_pixel(1, 4), Some(color));
        assert_eq!(renderer.get_pixel(2, 4), Some(color));
        assert_eq!(renderer.get_pixel(3, 4), Some(color));
    }

    #[test]
    fn draw_rect_renders_outline() {
        let info = FramebufferInfo::new(5, 5, 15, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 75];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x00, 0x00, 0xFF);

        renderer.draw_rect(Pixel::new(0, 0), 3, 3, color);

        // 验证四条边
        // 上边: (0,0) -> (3,0)
        assert_eq!(renderer.get_pixel(0, 0), Some(color));
        assert_eq!(renderer.get_pixel(1, 0), Some(color));
        assert_eq!(renderer.get_pixel(2, 0), Some(color));
        assert_eq!(renderer.get_pixel(3, 0), Some(color));
        // 左边: (0,0) -> (0,3)
        assert_eq!(renderer.get_pixel(0, 1), Some(color));
        assert_eq!(renderer.get_pixel(0, 2), Some(color));
        assert_eq!(renderer.get_pixel(0, 3), Some(color));
        // 右边: (3,0) -> (3,3)
        assert_eq!(renderer.get_pixel(3, 1), Some(color));
        assert_eq!(renderer.get_pixel(3, 2), Some(color));
        assert_eq!(renderer.get_pixel(3, 3), Some(color));
        // 下边: (0,3) -> (3,3)
        assert_eq!(renderer.get_pixel(1, 3), Some(color));
        assert_eq!(renderer.get_pixel(2, 3), Some(color));
        // 内部不被填充
        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(2, 2), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn fill_rect_fills_interior() {
        let info = FramebufferInfo::new(4, 4, 12, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0xFF, 0xFF, 0x00);

        renderer.fill_rect(Pixel::new(0, 0), 3, 3, color);

        // 验证整个矩形被填充
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(renderer.get_pixel(x, y), Some(color));
            }
        }
        // 边界外不被填充
        assert_eq!(renderer.get_pixel(3, 0), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(0, 3), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn fill_rect_clips_to_bounds() {
        let info = FramebufferInfo::new(4, 4, 12, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0xFF, 0x00, 0xFF);

        // 超出边界的矩形应该被裁剪
        renderer.fill_rect(Pixel::new(2, 2), 4, 4, color);

        // 只有可见部分被填充
        assert_eq!(renderer.get_pixel(2, 2), Some(color));
        assert_eq!(renderer.get_pixel(3, 2), Some(color));
        assert_eq!(renderer.get_pixel(2, 3), Some(color));
        assert_eq!(renderer.get_pixel(3, 3), Some(color));
        // 不可见部分保持黑色
        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
    }
}
