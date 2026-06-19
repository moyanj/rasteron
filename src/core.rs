use crate::color::Color;
use crate::framebuffer::{FramebufferInfo, PixelFormat};
use crate::pixel::Pixel;
use crate::utils::{clip_line, saturating_usize_to_i64};
extern crate alloc;
use alloc::vec::Vec;

/// 软件光栅化渲染器，操作原始帧缓冲字节切片。
///
/// `Renderer` 将绘制命令（像素、线段、三角形、矩形、圆形）直接写入
/// 帧缓冲内存。支持可选的[双缓冲](index.html#feature-flags)和
/// Alpha 混合。
///
/// # 类型参数
///
/// * `'a` — 帧缓冲切片的生命周期。
///
/// # 示例
///
/// ```
/// # use rasteron::Renderer;
/// # use rasteron::framebuffer::FramebufferInfo;
/// # use rasteron::color::{PixelFormat, Color};
/// # use rasteron::Pixel;
/// let info = FramebufferInfo::new(320, 240, 960, PixelFormat::Bgr24);
/// let mut fb = vec![0u8; info.buffer_size()];
/// let mut renderer = Renderer::new(info, &mut fb);
///
/// renderer.set_pixel(10, 10, &Color::new(255, 0, 0));
/// ```
pub struct Renderer<'a> {
    info: FramebufferInfo,
    front_buffer: &'a mut [u8],
    #[cfg(feature = "double_buffer")]
    back_buffer: Vec<u8>,
}

/// 整数平方根（牛顿法），用于纯整数圆弧计算。
#[inline(always)]
fn integer_sqrt(n: usize) -> usize {
    if n <= 1 {
        return n;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (y + n / y) / 2;
    }
    x
}

impl<'a> Renderer<'a> {
    /// 创建新的渲染器。
    ///
    /// * `info` — 帧缓冲布局描述。
    /// * `front_buffer` — 前端帧缓冲的可变引用。
    ///
    /// 当启用 `double_buffer` 特性时，会自动分配后端缓冲区。
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

    /// 返回当前可写入的缓冲区切片（内部使用）。
    ///
    /// 双缓冲模式下返回 `back_buffer`，否则返回 `front_buffer`。
    #[inline(always)]
    pub(crate) fn current_buffer_mut(&mut self) -> &mut [u8] {
        #[cfg(feature = "double_buffer")]
        return &mut self.back_buffer;
        #[cfg(not(feature = "double_buffer"))]
        return &mut self.front_buffer;
    }

    /// 返回当前可读取的缓冲区切片（内部使用）。
    #[inline(always)]
    pub(crate) fn current_buffer(&self) -> &[u8] {
        #[cfg(feature = "double_buffer")]
        return &self.back_buffer;
        #[cfg(not(feature = "double_buffer"))]
        return &self.front_buffer;
    }

    /// 每个像素占用的字节数。
    #[inline(always)]
    pub fn pixel_size(&self) -> u16 {
        self.info.bytes_per_pixel() as u16
    }

    /// 计算像素 `(x, y)` 在缓冲区中的字节偏移。
    ///
    /// 不执行边界检查。
    #[inline(always)]
    pub fn pixel_offset(&self, x: usize, y: usize) -> usize {
        self.pixel_offset_unchecked(x, y)
    }

    /// 计算像素 `(x, y)` 在缓冲区中的字节偏移（不执行边界检查，内部使用）。
    #[inline(always)]
    pub(crate) fn pixel_offset_unchecked(&self, x: usize, y: usize) -> usize {
        y * self.info.pitch + x * self.info.bytes_per_pixel()
    }

    /// 尝试计算像素 `(x, y)` 的字节偏移，在越界时返回 `None`。
    ///
    /// 接受 `i64` 坐标，用于插值计算后安全转换。
    #[inline(always)]
    pub(crate) fn try_pixel_offset_i64(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 {
            return None;
        }

        let (x, y) = (x as usize, y as usize);
        if x >= self.info.width || y >= self.info.height {
            return None;
        }

        Some(self.pixel_offset_unchecked(x, y))
    }

    /// 在指定字节偏移处写入颜色（支持 Alpha 混合）。
    ///
    /// 如果 `color.a == 0`，直接返回，不执行任何写入。
    #[inline(always)]
    pub(crate) fn write_color_at_offset(&mut self, offset: usize, color: &Color) {
        if color.a == 0 {
            return;
        }

        let format = self.info.format;
        let target = self.current_buffer_mut();
        let bpp = format.bytes_per_pixel();
        debug_assert!(
            offset + bpp <= target.len(),
            "write_color_at_offset: offset {offset} + bpp {bpp} exceeds buffer length {}",
            target.len()
        );
        let dst = &mut target[offset..offset + bpp];
        format.write_color(dst, color);
    }

    /// 在可见的 `(x, y)` 坐标处写入颜色；如果坐标越界则忽略。
    #[inline(always)]
    fn write_color_if_visible_i64(&mut self, x: i64, y: i64, color: &Color) {
        if let Some(offset) = self.try_pixel_offset_i64(x, y) {
            self.write_color_at_offset(offset, color);
        }
    }

    /// 将同格式像素数据块 blit（原始拷贝）到渲染目标。
    ///
    /// `dst_top_left` 指定目标区域左上角，`src` 为源像素数据（格式必须与帧缓冲相同）。
    /// 使用源数据的 pith（每行字节数）来正确换行。
    /// 自动裁剪到帧缓冲可见区域。
    ///
    /// 当 `src_format` 与帧缓冲格式不匹配、`width`/`height` 为 0、或 `src` 切片长度
    /// 不足以容纳 `pitch × height` 字节时，静默返回，不写入任何像素。
    pub fn blit(
        &mut self,
        dst_top_left: Pixel,
        width: usize,
        height: usize,
        src_format: PixelFormat,
        src: &[u8],
        src_pitch: usize,
    ) {
        if src_format != self.info.format || width == 0 || height == 0 {
            return;
        }
        if src.len() < src_pitch.saturating_mul(height) {
            return;
        }
        let bpp = self.info.format.bytes_per_pixel();
        let fb_width = self.info.width;
        let dst_x = dst_top_left.x;

        for row in 0..height {
            let dy = dst_top_left.y + row;
            if dy >= self.info.height {
                break;
            }
            // Row-level clipping: skip if entire row is outside the framebuffer
            if dst_x >= fb_width {
                continue;
            }
            let visible_cols = width.min(fb_width - dst_x);
            let copy_bytes = visible_cols * bpp;
            let src_start = row * src_pitch;
            let dst_offset = self.pixel_offset_unchecked(dst_x, dy);
            self.current_buffer_mut()[dst_offset..dst_offset + copy_bytes]
                .copy_from_slice(&src[src_start..src_start + copy_bytes]);
        }
    }

    /// 使用 8bpp 的 mask 将指定颜色绘制到渲染目标。
    ///
    /// `mask` 中每个字节代表对应像素的不透明度（0=透明，255=完全不透明）。
    /// 适用于文本渲染/精灵绘制的前置能力。
    ///
    /// 当 `width` 或 `height` 为 0，或 `mask` 切片长度不足时，静默返回。
    pub fn blit_mask(
        &mut self,
        dst_top_left: Pixel,
        width: usize,
        height: usize,
        mask: &[u8],
        mask_pitch: usize,
        color: Color,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        if mask.len() < mask_pitch.saturating_mul(height) {
            return;
        }
        let fb_width = self.info.width;
        let fb_height = self.info.height;
        let dst_x = dst_top_left.x;
        let bpp = self.info.format.bytes_per_pixel();

        for row in 0..height {
            let dy = dst_top_left.y + row;
            if dy >= fb_height {
                break;
            }
            // Row-level clipping
            if dst_x >= fb_width {
                continue;
            }
            let visible_cols = width.min(fb_width - dst_x);
            let row_mask_off = row * mask_pitch;
            let base_off = self.pixel_offset_unchecked(dst_x, dy);

            for col in 0..visible_cols {
                let alpha = mask[row_mask_off + col];
                if alpha == 0 {
                    continue;
                }
                let shaded = Color::with_alpha(color.r, color.g, color.b, alpha);
                let offset = base_off + col * bpp;
                self.write_color_at_offset(offset, &shaded);
            }
        }
    }

    /// 用不透明颜色快速填充一行中的水平跨度。
    ///
    /// 使用倍增复制策略（先写入一个像素，然后不断将已填充部分复制到
    /// 未填充区域），以最小化逐个像素写入的开销。
    ///
    /// 倍增循环中源和目标区域不存在重叠，因此使用 `copy_nonoverlapping` 替代
    /// `copy_within` 以消除切片边界检查开销。
    #[inline(always)]
    pub(crate) fn fill_span_opaque(
        &mut self,
        x_start: usize,
        x_end: usize,
        y: usize,
        color: &Color,
    ) {
        let format = self.info.format;
        let pixel_size = format.bytes_per_pixel();
        let start = self.pixel_offset_unchecked(x_start, y);
        let row_len = (x_end - x_start + 1) * pixel_size;
        let target = self.current_buffer_mut();
        let row = &mut target[start..start + row_len];

        format.write_color(&mut row[..pixel_size], color);
        Self::doubling_fill(row, pixel_size, row_len);
    }

    /// 在可见区域内填充水平跨度。自动裁剪到帧缓冲边界。
    ///
    /// 不透明颜色使用 `fill_span_opaque` 快速填充，否则逐像素写入。
    #[inline(always)]
    pub(crate) fn fill_visible_horizontal_span(
        &mut self,
        start_x: i64,
        end_x: i64,
        y: i64,
        color: &Color,
    ) {
        if y < 0 || y >= self.info.height as i64 {
            return;
        }
        let mut sx = start_x.max(0);
        let mut ex = end_x.min(self.info.width.saturating_sub(1) as i64);
        if sx > ex {
            core::mem::swap(&mut sx, &mut ex);
        }
        if color.a == 255 {
            self.fill_span_opaque(sx as usize, ex as usize, y as usize, color);
        } else {
            for x in sx..=ex {
                self.write_color_at_offset(
                    self.pixel_offset_unchecked(x as usize, y as usize),
                    color,
                );
            }
        }
    }

    /// 扫描线插值：给定基坐标 `base_x`、增量 `delta_x` / `delta_y` 和
    /// 垂直偏移 `offset_y`，计算对应的 X 坐标。
    ///
    /// 使用 `i128` 中间运算以避免溢出，并进行饱和钳位。
    #[inline(always)]
    fn interpolate_scan_x(base_x: i64, delta_x: i64, delta_y: i64, offset_y: i64) -> i64 {
        if delta_y == 0 {
            return base_x;
        }

        let value = base_x as i128
            + ((delta_x as i128 * offset_y as i128) + (delta_y as i128 / 2)) / delta_y as i128;

        value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// 在指定坐标处写入像素，**不执行边界检查**。
    ///
    /// # Safety
    ///
    /// 调用方必须保证 `(x, y)` 在帧缓冲范围内。
    pub unsafe fn set_pixel_unchecked(&mut self, x: usize, y: usize, color: &Color) {
        let offset = self.pixel_offset(x, y);
        self.write_color_at_offset(offset, color);
    }

    /// 在指定坐标处写入像素（安全版本，执行边界检查）。
    ///
    /// 如果坐标越界，调用被安全忽略。
    #[inline(always)]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: &Color) {
        if x >= self.info.width || y >= self.info.height {
            return;
        }
        unsafe { self.set_pixel_unchecked(x, y, color) };
    }

    /// 使用 [`Pixel`] 坐标写入像素，**不执行边界检查**。
    ///
    /// # Safety
    ///
    /// 调用方必须保证像素坐标在帧缓冲范围内。
    #[inline(always)]
    pub unsafe fn set_pixel_at_unchecked(&mut self, pixel: Pixel, color: &Color) {
        unsafe { self.set_pixel_unchecked(pixel.x, pixel.y, color) };
    }

    /// 使用 [`Pixel`] 坐标写入像素（安全版本）。
    #[inline(always)]
    pub fn set_pixel_at(&mut self, pixel: Pixel, color: &Color) {
        self.set_pixel(pixel.x, pixel.y, color);
    }

    /// 读取指定坐标处的像素颜色，**不执行边界检查**。
    ///
    /// # Safety
    ///
    /// 调用方必须保证 `(x, y)` 在帧缓冲范围内。
    pub unsafe fn get_pixel_unchecked(&self, x: usize, y: usize) -> Color {
        let offset = self.pixel_offset(x, y);
        let format = self.info.format;
        let target = self.current_buffer();
        let src = &target[offset..offset + format.bytes_per_pixel()];
        format.read_color(src)
    }

    /// 读取指定坐标处的像素颜色（安全版本）。
    ///
    /// 坐标越界时返回 `None`。
    pub fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= self.info.width || y >= self.info.height {
            return None;
        }
        Some(unsafe { self.get_pixel_unchecked(x, y) })
    }

    /// 使用 [`Pixel`] 坐标读取像素颜色，**不执行边界检查**。
    ///
    /// # Safety
    ///
    /// 调用方必须保证像素坐标在帧缓冲范围内。
    #[inline(always)]
    pub unsafe fn get_pixel_at_unchecked(&self, pixel: Pixel) -> Color {
        unsafe { self.get_pixel_unchecked(pixel.x, pixel.y) }
    }

    /// 使用 [`Pixel`] 坐标读取像素颜色（安全版本）。
    #[inline(always)]
    pub fn get_pixel_at(&self, pixel: Pixel) -> Option<Color> {
        self.get_pixel(pixel.x, pixel.y)
    }

    /// 用指定颜色清空整个帧缓冲。
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
    /// 使用倍增复制策略逐行填充。
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
            let row = unsafe { core::slice::from_raw_parts_mut(row_ptr, row_len) };
            Self::doubling_fill(row, pixel_size, row_len);
        }
    }

    #[inline(always)]
    fn doubling_fill(row: &mut [u8], pattern_len: usize, len: usize) {
        let base = row.as_mut_ptr();
        let mut filled = pattern_len;
        while filled < len {
            let copy_len = core::cmp::min(filled, len - filled);
            unsafe {
                core::ptr::copy_nonoverlapping(base, base.add(filled), copy_len);
            }
            filled += copy_len;
        }
    }

    #[inline(always)]
    fn walk_circle_points(
        &mut self,
        center: Pixel,
        radius: usize,
        mut visit: impl FnMut(&mut Self, i64, i64, i64, i64),
    ) {
        let cx = saturating_usize_to_i64(center.x);
        let cy = saturating_usize_to_i64(center.y);
        let mut x = 0i64;
        let mut y = radius as i64;
        let mut d = 3 - 2 * radius as i64;

        while x <= y {
            visit(self, cx, cy, x, y);
            if d < 0 {
                d += 4 * x + 6;
            } else {
                d += 4 * (x - y) + 10;
                y -= 1;
            }
            x += 1;
        }
    }

    /// 使用 Bresenham 算法绘制线段，自动裁剪到帧缓冲边界。
    pub fn draw_line(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
        if clip_line(p1, p2, self.info.width, self.info.height).is_none() {
            return;
        }

        unsafe { self.draw_line_unchecked(p1, p2, color) };
    }

    /// Bresenham 线段算法，**不进行裁剪检查**。
    ///
    /// 仅通过 `write_color_if_visible_i64` 进行逐个像素的边界保护，
    /// 不会通过 `clip_line` 提前拒绝整条线段。
    ///
    /// # Safety
    ///
    /// 调用方必须保证渲染器绑定的帧缓冲有效。
    pub unsafe fn draw_line_unchecked(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
        let (mut x1, mut y1) = (saturating_usize_to_i64(p1.x), saturating_usize_to_i64(p1.y));
        let (mut x2, mut y2) = (saturating_usize_to_i64(p2.x), saturating_usize_to_i64(p2.y));
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
            self.write_color_if_visible_i64(px, py, color);
            error -= dy as i64;
            if error < 0 {
                y += y_step;
                error += dx;
            }
        }
    }

    /// 绘制三角形轮廓（由三条线段组成）。
    #[inline]
    pub fn draw_triangle(&mut self, p1: Pixel, p2: Pixel, p3: Pixel, color: &Color) {
        self.draw_line(p1, p2, color);
        self.draw_line(p2, p3, color);
        self.draw_line(p3, p1, color);
    }

    /// 使用扫描线算法填充三角形。
    ///
    /// 将三角形按 Y 坐标拆分为上下两部分，对每行扫描线进行插值填充。
    /// 不透明颜色使用 `fill_span_opaque` 快速填充，否则逐像素写入。
    ///
    /// # 算法
    ///
    /// 1. 按 Y 对三个顶点排序。
    /// 2. 将三角形在中间顶点处分为上下两部分。
    /// 3. 对每行扫描线，计算与三角形两条边的交点并填充。
    #[inline]
    pub fn fill_triangle(&mut self, p1: Pixel, p2: Pixel, p3: Pixel, color: &Color) {
        if self.info.width == 0 || self.info.height == 0 {
            return;
        }

        let max_x = self.info.width - 1;
        let max_y = self.info.height - 1;

        // 按 Y 坐标排序三个顶点
        let mut pts = [
            Pixel::new(p1.x.min(max_x), p1.y.min(max_y)),
            Pixel::new(p2.x.min(max_x), p2.y.min(max_y)),
            Pixel::new(p3.x.min(max_x), p3.y.min(max_y)),
        ];
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }
        if pts[1].y > pts[2].y {
            pts.swap(1, 2);
        }
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }

        let (x1, y1) = (
            saturating_usize_to_i64(pts[0].x),
            saturating_usize_to_i64(pts[0].y),
        );
        let (x2, y2) = (
            saturating_usize_to_i64(pts[1].x),
            saturating_usize_to_i64(pts[1].y),
        );
        let (x3, y3) = (
            saturating_usize_to_i64(pts[2].x),
            saturating_usize_to_i64(pts[2].y),
        );

        // 如果三个点的 Y 坐标相同，不绘制
        if y1 == y3 {
            return;
        }

        let long_dx = x3 - x1;
        let long_dy = y3 - y1;

        // 上半部分三角形 (p1 -> p2)
        let upper_dx = x2 - x1;
        let upper_dy = y2 - y1;
        for y in y1..=y2 {
            let dy = y - y1;
            let x_long = Self::interpolate_scan_x(x1, long_dx, long_dy, dy);
            let x_upper = Self::interpolate_scan_x(x1, upper_dx, upper_dy, dy);
            self.fill_visible_horizontal_span(x_long, x_upper, y, color);
        }

        // 下半部分三角形 (p2 -> p3)
        let lower_dx = x3 - x2;
        let lower_dy = y3 - y2;
        for y in y2..=y3 {
            let dy_long = y - y1;
            let dy_lower = y - y2;
            let x_long = Self::interpolate_scan_x(x1, long_dx, long_dy, dy_long);
            let x_lower = Self::interpolate_scan_x(x2, lower_dx, lower_dy, dy_lower);
            self.fill_visible_horizontal_span(x_long, x_lower, y, color);
        }
    }

    /// 绘制矩形轮廓（由四条线段组成）。
    #[inline]
    pub fn draw_rect(&mut self, top_left: Pixel, width: usize, height: usize, color: &Color) {
        let (x, y) = top_left.to_coord();
        let x2 = x.saturating_add(width);
        let y2 = y.saturating_add(height);
        self.draw_line(Pixel::new(x, y), Pixel::new(x2, y), color);
        self.draw_line(Pixel::new(x2, y), Pixel::new(x2, y2), color);
        self.draw_line(Pixel::new(x2, y2), Pixel::new(x, y2), color);
        self.draw_line(Pixel::new(x, y2), Pixel::new(x, y), color);
    }

    /// 填充矩形区域。
    ///
    /// 不透明颜色使用 `fill_span_opaque` 快速填充，否则逐像素写入。
    /// 自动裁剪到帧缓冲边界。
    #[inline]
    pub fn fill_rect(&mut self, top_left: Pixel, width: usize, height: usize, color: Color) {
        let (x_min, y_min) = top_left.to_coord();
        let x_max = x_min.saturating_add(width).min(self.info.width);
        let y_max = y_min.saturating_add(height).min(self.info.height);
        let x_start = x_min;
        let y_start = y_min;

        if x_start >= x_max || y_start >= y_max {
            return;
        }

        if color.a == 255 {
            for y in y_start..y_max {
                self.fill_span_opaque(x_start, x_max - 1, y, &color);
            }
        } else {
            for y in y_start..y_max {
                for x in x_start..x_max {
                    self.write_color_at_offset(self.pixel_offset_unchecked(x, y), &color);
                }
            }
        }
    }

    /// 使用中点圆算法绘制圆形轮廓。
    ///
    /// 利用八对称性绘制所有对称点，自动忽略超出帧缓冲边界的像素。
    pub fn draw_circle(&mut self, center: Pixel, radius: usize, color: &Color) {
        if radius == 0 {
            return;
        }

        self.walk_circle_points(center, radius, |renderer, cx, cy, x, y| {
            for (px, py) in [
                (cx + x, cy + y),
                (cx + x, cy - y),
                (cx - x, cy + y),
                (cx - x, cy - y),
                (cx + y, cy + x),
                (cx + y, cy - x),
                (cx - y, cy + x),
                (cx - y, cy - x),
            ] {
                if px >= 0 && py >= 0 {
                    renderer.set_pixel(px as usize, py as usize, color);
                }
            }
        });
    }

    /// 使用中点圆算法填充圆形区域。
    ///
    /// 利用八对称性生成水平跨度，通过 `fill_visible_horizontal_span` 写入。
    /// 自动裁剪到帧缓冲边界。
    pub fn fill_circle(&mut self, center: Pixel, radius: usize, color: Color) {
        if radius == 0 || self.info.width == 0 || self.info.height == 0 {
            return;
        }

        self.walk_circle_points(center, radius, |renderer, cx, cy, x, y| {
            for (sx, ex, py) in [
                (cx - x, cx + x, cy + y),
                (cx - x, cx + x, cy - y),
                (cx - y, cx + y, cy + x),
                (cx - y, cx + y, cy - x),
            ] {
                renderer.fill_visible_horizontal_span(sx, ex, py, &color);
            }
        });
    }

    /// 使用中心矩形 + 整数中点圆弧扫描线填充圆角矩形。
    ///
    /// 先用两个贯穿矩形填充中心区域，再逐行计算四角的圆弧边界。
    /// 自动裁剪到帧缓冲边界。
    pub fn fill_rounded_rect(
        &mut self,
        top_left: Pixel,
        width: usize,
        height: usize,
        radius: usize,
        color: Color,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let radius = radius.min(width / 2).min(height / 2);
        if radius == 0 {
            self.fill_rect(top_left, width, height, color);
            return;
        }

        let core_w = width - radius * 2;
        let core_h = height - radius * 2;

        // Center body (no overlap with edge strips)
        self.fill_rect(
            Pixel::new(top_left.x + radius, top_left.y + radius),
            core_w,
            core_h,
            color,
        );
        // Top edge strip (between top-left and top-right corner arcs)
        self.fill_rect(
            Pixel::new(top_left.x + radius, top_left.y),
            core_w,
            radius,
            color,
        );
        // Bottom edge strip
        self.fill_rect(
            Pixel::new(top_left.x + radius, top_left.y + height - radius),
            core_w,
            radius,
            color,
        );
        // Left edge strip
        self.fill_rect(
            Pixel::new(top_left.x, top_left.y + radius),
            radius,
            core_h,
            color,
        );
        // Right edge strip
        self.fill_rect(
            Pixel::new(top_left.x + width - radius, top_left.y + radius),
            radius,
            core_h,
            color,
        );

        let bottom = top_left.y.saturating_add(height).saturating_sub(1);
        let r_sq = radius * radius;

        for dy in 0..radius {
            // vertical distance from the center of the corner arc
            let dist_y = radius - dy;
            // compute x² on the circle boundary at this distance
            let x_sq = r_sq - dist_y * dist_y;
            // integer square root: how far from corner center to the arc edge
            let x_edge = integer_sqrt(x_sq);
            // pixels to cut from the corner edge at this scanline
            let inset = radius - x_edge;

            let sx = (top_left.x + inset) as i64;
            let ex = (top_left.x + width - 1 - inset) as i64;
            let y_top = (top_left.y + dy) as i64;
            let y_bottom = bottom as i64 - dy as i64;
            self.fill_visible_horizontal_span(sx, ex, y_top, &color);
            self.fill_visible_horizontal_span(sx, ex, y_bottom, &color);
        }
    }

    /// 如果启用双缓冲，将后台缓冲区交换到前台（即刷新到显存）。
    ///
    /// 单缓冲模式下此方法不可用。
    #[cfg(feature = "double_buffer")]
    pub fn swap_buffers(&mut self) {
        self.front_buffer.copy_from_slice(&self.back_buffer);
    }

    /// 获取当前缓冲区的可变引用。
    ///
    /// 在需要直接操作缓冲区的大批量像素绘制场景中使用。
    #[inline(always)]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.current_buffer_mut()
    }

    /// 返回帧缓冲布局信息的引用。
    pub fn info(&self) -> &FramebufferInfo {
        &self.info
    }
}

#[cfg(test)]
mod tests {
    use super::{Renderer, integer_sqrt};
    use crate::color::{Color, PixelFormat};
    use crate::framebuffer::FramebufferInfo;
    use crate::pixel::Pixel;

    #[test]
    fn set_pixel_uses_bgr24_format() {
        let info = FramebufferInfo::new(2, 1, 6, PixelFormat::Bgr24);
        let mut front = [0u8; 6];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(1, 0, &Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(&renderer.buffer_mut()[3..6], &[0x56, 0x34, 0x12]);
    }

    #[test]
    fn set_pixel_uses_rgba8888_format() {
        let info = FramebufferInfo::new(1, 1, 4, PixelFormat::Rgba8888);
        let mut front = [0u8; 4];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, &Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(&renderer.buffer_mut()[..4], &[0x12, 0x34, 0x56, 0xFF]);
    }

    #[test]
    fn set_pixel_alpha_blends_into_bgr24_buffer() {
        let info = FramebufferInfo::new(1, 1, 3, PixelFormat::Bgr24);
        let mut front = [0u8; 3];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, &Color::new(10, 20, 30));
        renderer.set_pixel(0, 0, &Color::with_alpha(110, 120, 130, 128));

        assert_eq!(&renderer.buffer_mut()[..3], &[80, 70, 60]);
    }

    #[test]
    fn set_pixel_alpha_blends_into_rgba8888_buffer() {
        let info = FramebufferInfo::new(1, 1, 4, PixelFormat::Rgba8888);
        let mut front = [0u8; 4];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, &Color::with_alpha(20, 40, 60, 255));
        renderer.set_pixel(0, 0, &Color::with_alpha(120, 140, 160, 128));

        assert_eq!(&renderer.buffer_mut()[..4], &[70, 90, 110, 255]);
    }

    #[test]
    fn set_pixel_uses_shift_based_xrgb8888_layout() {
        let info = FramebufferInfo::new(1, 1, 4, PixelFormat::Xrgb8888);
        let mut front = [0u8; 4];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(0, 0, &Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(&renderer.buffer_mut()[..4], &[0x56, 0x34, 0x12, 0x00]);
    }

    #[test]
    fn clear_fills_bgr24_rows() {
        let info = FramebufferInfo::new(2, 2, 6, PixelFormat::Bgr24);
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
        let info = FramebufferInfo::new(2, 1, 8, PixelFormat::Bgr24);
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
        let info = FramebufferInfo::new(2, 1, 6, PixelFormat::Bgr24);
        let mut front = [0u8; 6];
        let mut renderer = Renderer::new(info, &mut front);
        let pixel = Pixel::new(1, 0);

        renderer.set_pixel_at(pixel, &Color::with_alpha(0x12, 0x34, 0x56, 0xFF));

        assert_eq!(
            renderer.get_pixel_at(pixel),
            Some(Color::new(0x12, 0x34, 0x56))
        );
    }

    #[test]
    fn draw_line_renders_diagonal() {
        let info = FramebufferInfo::new(3, 3, 9, PixelFormat::Bgr24);
        let mut front = [0u8; 27];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.draw_line(
            Pixel::new(0, 0),
            Pixel::new(2, 2),
            &Color::new(0x12, 0x34, 0x56),
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
            FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24),
            &mut front_safe,
        );
        let mut unchecked = Renderer::new(
            FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24),
            &mut front_unchecked,
        );
        let start = Pixel::new(0, 1);
        let end = Pixel::new(3, 2);
        let color = Color::new(0x12, 0x34, 0x56);

        safe.draw_line(start, end, &color);
        unsafe { unchecked.draw_line_unchecked(start, end, &color) };

        assert_eq!(safe.buffer_mut(), unchecked.buffer_mut());
    }

    #[test]
    fn draw_line_clips_right_edge() {
        let info = FramebufferInfo::new(4, 2, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 24];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x12, 0x34, 0x56);

        renderer.draw_line(Pixel::new(2, 0), Pixel::new(6, 0), &color);

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(2, 0), Some(color));
        assert_eq!(renderer.get_pixel(3, 0), Some(color));
    }

    #[test]
    fn draw_line_preserves_bresenham_coverage_when_end_is_below_screen() {
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x12, 0x34, 0x56);

        renderer.draw_line(Pixel::new(0, 0), Pixel::new(1, 4), &color);

        assert_eq!(renderer.get_pixel(0, 0), Some(color));
        assert_eq!(renderer.get_pixel(0, 1), Some(color));
        assert_eq!(renderer.get_pixel(0, 2), Some(color));
        assert_eq!(renderer.get_pixel(0, 3), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(1, 3), Some(color));
    }

    #[test]
    fn draw_triangle_renders_outline() {
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0xFF, 0x00, 0x00);

        renderer.draw_triangle(Pixel::new(0, 0), Pixel::new(3, 0), Pixel::new(1, 3), &color);

        // 验证三条边都被绘制
        assert_eq!(renderer.get_pixel(0, 0), Some(color));
        assert_eq!(renderer.get_pixel(1, 0), Some(color));
        assert_eq!(renderer.get_pixel(2, 0), Some(color));
        assert_eq!(renderer.get_pixel(3, 0), Some(color));
        assert_eq!(renderer.get_pixel(1, 3), Some(color));
    }

    #[test]
    fn fill_triangle_fills_interior() {
        let info = FramebufferInfo::new(5, 5, 15, PixelFormat::Bgr24);
        let mut front = [0u8; 75];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x00, 0xFF, 0x00);

        // 等边三角形
        renderer.fill_triangle(Pixel::new(2, 0), Pixel::new(0, 4), Pixel::new(4, 4), &color);

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
        let info = FramebufferInfo::new(5, 5, 15, PixelFormat::Bgr24);
        let mut front = [0u8; 75];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x00, 0x00, 0xFF);

        renderer.draw_rect(Pixel::new(0, 0), 3, 3, &color);

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
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
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
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
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

    #[test]
    fn fill_rect_saturates_large_dimensions() {
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0xAA, 0xBB, 0xCC);

        renderer.fill_rect(Pixel::new(3, 3), usize::MAX, usize::MAX, color);

        assert_eq!(renderer.get_pixel(3, 3), Some(color));
        assert_eq!(renderer.get_pixel(2, 3), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(3, 2), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn draw_circle_clips_points_with_negative_intermediates() {
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x12, 0x34, 0x56);

        renderer.draw_circle(Pixel::new(0, 0), 2usize, &color);

        assert_eq!(renderer.get_pixel(0, 2), Some(color));
        assert_eq!(renderer.get_pixel(2, 0), Some(color));
        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(0, 0, 0)));
    }

    // ── fill_rounded_rect ──

    #[test]
    fn fill_rounded_rect_preserves_corner_cut_and_fills_body() {
        let info = FramebufferInfo::new(8, 6, 24, PixelFormat::Bgr24);
        let mut front = [0u8; 144];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(80, 160, 240);

        renderer.fill_rounded_rect(Pixel::new(1, 1), 6, 4, 2, color);

        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(3, 1), Some(color));
        assert_eq!(renderer.get_pixel(1, 3), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(6, 4), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(4, 3), Some(color));
    }

    // ── fill_circle ──

    #[test]
    fn fill_circle_fills_center_and_cardinal_points() {
        let info = FramebufferInfo::new(7, 7, 21, PixelFormat::Bgr24);
        let mut front = [0u8; 147];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(200, 100, 50);

        renderer.fill_circle(Pixel::new(3, 3), 2, color);

        assert_eq!(renderer.get_pixel(3, 3), Some(color));
        assert_eq!(renderer.get_pixel(3, 1), Some(color));
        assert_eq!(renderer.get_pixel(3, 5), Some(color));
        assert_eq!(renderer.get_pixel(1, 3), Some(color));
        assert_eq!(renderer.get_pixel(5, 3), Some(color));
        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
    }

    // ── blit ──

    #[test]
    fn blit_copies_source_pixels_into_destination() {
        let info = FramebufferInfo::new(4, 2, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 24];
        let mut renderer = Renderer::new(info, &mut front);
        let src = [
            0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF,
        ];

        renderer.blit(Pixel::new(1, 0), 2, 2, PixelFormat::Bgr24, &src, 6);

        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(255, 0, 0)));
        assert_eq!(renderer.get_pixel(2, 1), Some(Color::new(255, 255, 255)));
    }

    // ── blit_mask ──

    #[test]
    fn blit_mask_applies_foreground_only_on_non_zero_mask() {
        let info = FramebufferInfo::new(4, 1, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 12];
        let mut renderer = Renderer::new(info, &mut front);
        let blue = Color::new(0, 0, 255);
        let mask = [0u8, 255, 16, 0];

        renderer.blit_mask(Pixel::new(0, 0), 4, 1, &mask, 4, blue);

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(1, 0), Some(blue));
        // 16/255 alpha of blue over black → B channel = 255 * 16 / 255 = 16, opaque
        assert_eq!(renderer.get_pixel(2, 0), Some(Color::new(0, 0, 16)));
    }

    #[test]
    fn fill_triangle_handles_large_usize_vertices_without_i32_narrowing() {
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x66, 0x77, 0x88);
        let huge = usize::MAX;

        renderer.fill_triangle(
            Pixel::new(1, 1),
            Pixel::new(huge, 1),
            Pixel::new(1, huge),
            &color,
        );

        assert_eq!(renderer.get_pixel(1, 1), Some(color));
        assert_eq!(renderer.get_pixel(2, 2), Some(color));
        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(0, 0, 0)));
    }

    // ── integer_sqrt ──

    #[test]
    fn integer_sqrt_basics() {
        assert_eq!(integer_sqrt(0), 0);
        assert_eq!(integer_sqrt(1), 1);
        assert_eq!(integer_sqrt(4), 2);
        assert_eq!(integer_sqrt(9), 3);
        assert_eq!(integer_sqrt(16), 4);
        assert_eq!(integer_sqrt(25), 5);
    }

    #[test]
    fn integer_sqrt_rounds_down() {
        assert_eq!(integer_sqrt(2), 1);
        assert_eq!(integer_sqrt(8), 2);
        assert_eq!(integer_sqrt(15), 3);
    }

    // ── blit edge cases ──

    #[test]
    fn blit_format_mismatch_does_nothing() {
        let info = FramebufferInfo::new(4, 2, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 24];
        let mut renderer = Renderer::new(info, &mut front);
        // Pre-fill a pixel to verify it's not overwritten
        renderer.set_pixel(1, 0, &Color::new(255, 255, 255));
        let src = [0x00, 0x00, 0xFF, 0x00, 0x00, 0xFF];

        // Rgb24 ≠ Bgr24 → silent return
        renderer.blit(Pixel::new(0, 0), 2, 1, PixelFormat::Rgb24, &src, 6);

        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn blit_undersized_source_does_nothing() {
        let info = FramebufferInfo::new(4, 2, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 24];
        let mut renderer = Renderer::new(info, &mut front);
        renderer.set_pixel(1, 0, &Color::new(255, 255, 255));
        // 2 bytes is not enough for pitch=6, height=1
        let src = [0x00, 0x00];

        renderer.blit(Pixel::new(0, 0), 2, 1, PixelFormat::Bgr24, &src, 6);

        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(255, 255, 255)));
    }

    // ── fill_rounded_rect edge cases ──

    #[test]
    fn fill_rounded_rect_zero_radius_delegates_to_fill_rect() {
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(100, 150, 200);

        // radius=0 should be equivalent to fill_rect
        renderer.fill_rounded_rect(Pixel::new(0, 0), 3, 3, 0, color);

        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(renderer.get_pixel(x, y), Some(color));
            }
        }
    }

    #[test]
    fn fill_rounded_rect_clamps_large_radius() {
        let info = FramebufferInfo::new(6, 6, 18, PixelFormat::Bgr24);
        let mut front = [0u8; 108];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(100, 150, 200);

        // width=4, height=4 → max radius = 2, so radius=10 gets clamped to 2
        renderer.fill_rounded_rect(Pixel::new(1, 1), 4, 4, 10, color);

        // Center pixel should be filled (not cut by oversized radius)
        assert_eq!(renderer.get_pixel(3, 3), Some(color));
        // Corner should be cut
        assert_eq!(renderer.get_pixel(1, 1), Some(Color::new(0, 0, 0)));
    }

    // ── fill_rect alpha path ──

    #[test]
    fn fill_rect_alpha_blends_correctly() {
        let info = FramebufferInfo::new(2, 1, 6, PixelFormat::Bgr24);
        let mut front = [0u8; 6];
        let mut renderer = Renderer::new(info, &mut front);
        // Fill with white first
        renderer.set_pixel(0, 0, &Color::new(255, 255, 255));
        renderer.set_pixel(1, 0, &Color::new(255, 255, 255));

        // Overlay 50% red → each channel = (255 + 0) / 2 ≈ 127 for R, but R=255 → R=255*128/255+0*127/255
        // Actually: src=(255,0,0) a=128, dst=(255,255,255)
        // R = (255*128 + 255*127)/255 = 255, G = (0*128 + 255*127)/255 = 127, B = (0*128 + 255*127)/255 = 127
        renderer.fill_rect(Pixel::new(0, 0), 2, 1, Color::with_alpha(255, 0, 0, 128));

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(255, 127, 127)));
        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(255, 127, 127)));
    }
}
