use crate::color::Color;
use crate::framebuffer::{FramebufferInfo, PixelFormat};
use crate::pixel::Pixel;
#[cfg(feature = "simd")]
use crate::simd;
use crate::utils::{clip_line, saturating_usize_to_i64};
#[cfg(feature = "double_buffer")]
extern crate alloc;
#[cfg(feature = "double_buffer")]
use alloc::vec::Vec;

/// 根据字节序写入 RGB 三通道到目标切片。
///
/// `$is_rgb_order` 为 `true` 时按 R,G,B 写入，为 `false` 时按 B,G,R 写入。
macro_rules! write_rgb {
    ($dst:expr, $r:expr, $g:expr, $b:expr, $is_rgb_order:expr) => {
        if $is_rgb_order {
            $dst[0] = $r;
            $dst[1] = $g;
            $dst[2] = $b;
        } else {
            $dst[0] = $b;
            $dst[1] = $g;
            $dst[2] = $r;
        }
    };
}

/// 对 RGB 三通道执行 Alpha 混合后写入目标切片。
///
/// 使用 [`PixelFormat::blend_channel`] 进行单通道混合。
/// `$is_rgb_order` 控制字节序。
macro_rules! write_rgb_blend {
    ($dst:expr, $r:expr, $g:expr, $b:expr, $alpha:expr, $inv_alpha:expr, $is_rgb_order:expr) => {
        if $is_rgb_order {
            $dst[0] = PixelFormat::blend_channel($r, $dst[0], $alpha, $inv_alpha);
            $dst[1] = PixelFormat::blend_channel($g, $dst[1], $alpha, $inv_alpha);
            $dst[2] = PixelFormat::blend_channel($b, $dst[2], $alpha, $inv_alpha);
        } else {
            $dst[0] = PixelFormat::blend_channel($b, $dst[0], $alpha, $inv_alpha);
            $dst[1] = PixelFormat::blend_channel($g, $dst[1], $alpha, $inv_alpha);
            $dst[2] = PixelFormat::blend_channel($r, $dst[2], $alpha, $inv_alpha);
        }
    };
}

/// 对 RGB 三通道执行 Alpha 混合后写入裸指针位置。
///
/// 使用 [`PixelFormat::blend_channel`] 进行单通道混合。
/// `$is_rgb_order` 控制字节序。`$dst` 为 `*mut u8` 裸指针。
macro_rules! write_rgb_blend_ptr {
    ($dst:expr, $r:expr, $g:expr, $b:expr, $alpha:expr, $inv_alpha:expr, $is_rgb_order:expr) => {
        if $is_rgb_order {
            unsafe {
                *$dst = PixelFormat::blend_channel($r, *$dst, $alpha, $inv_alpha);
                *$dst.add(1) = PixelFormat::blend_channel($g, *$dst.add(1), $alpha, $inv_alpha);
                *$dst.add(2) = PixelFormat::blend_channel($b, *$dst.add(2), $alpha, $inv_alpha);
            }
        } else {
            unsafe {
                *$dst = PixelFormat::blend_channel($b, *$dst, $alpha, $inv_alpha);
                *$dst.add(1) = PixelFormat::blend_channel($g, *$dst.add(1), $alpha, $inv_alpha);
                *$dst.add(2) = PixelFormat::blend_channel($r, *$dst.add(2), $alpha, $inv_alpha);
            }
        }
    };
}

/// 对 RGB 三通道直接写入裸指针位置（不透明路径）。
///
/// `$is_rgb_order` 控制字节序。`$dst` 为 `*mut u8` 裸指针。
macro_rules! write_rgb_ptr {
    ($dst:expr, $r:expr, $g:expr, $b:expr, $is_rgb_order:expr) => {
        if $is_rgb_order {
            unsafe {
                *$dst = $r;
                *$dst.add(1) = $g;
                *$dst.add(2) = $b;
            }
        } else {
            unsafe {
                *$dst = $b;
                *$dst.add(1) = $g;
                *$dst.add(2) = $r;
            }
        }
    };
}

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

    // --- 缓存的像素格式属性（在 new() 中初始化一次，消除热路径中的格式匹配）---
    pub(crate) fmt_bpp: usize,
    pub(crate) fmt_rgb_order: bool,
    pub(crate) fmt_opaque_alpha: u8,
    pub(crate) fmt_stores_color_alpha: bool,
}

/// 整数平方根（牛顿法），用于纯整数圆弧计算。
///
/// 使用 bit-shift 估初值，减少迭代次数。
#[inline(always)]
fn integer_sqrt(n: usize) -> usize {
    if n <= 1 {
        return n;
    }
    // 用最高有效位估初值，减少牛顿法迭代次数约 2-3 次
    let bits = usize::BITS as usize - n.leading_zeros() as usize;
    let mut x = 1usize << (bits + 1) / 2;
    let mut y = (x + n / x) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
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
    ///
    /// # 恐慌
    ///
    /// 如果 `front_buffer.len() < info.buffer_size()` 则恐慌。渲染器的所有写入都
    /// 以 `info` 为准进行边界保护（安全 API 的边界判断比较的是 `width` / `height`，
    /// 底层写入使用 `get_unchecked_mut`），因此调用方必须保证切片能容纳
    /// `pitch × height` 字节。这一校验是安全 API 不发生越界写的前提。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::Renderer;
    /// # use rasteron::framebuffer::FramebufferInfo;
    /// # use rasteron::color::PixelFormat;
    /// let info = FramebufferInfo::new(16, 16, 48, PixelFormat::Bgr24);
    /// let mut fb = vec![0u8; info.buffer_size()];
    /// let mut renderer = Renderer::new(info, &mut fb);
    /// renderer.set_pixel(0, 0, &rasteron::color::Color::new(255, 0, 0));
    /// ```
    pub fn new(info: FramebufferInfo, front_buffer: &'a mut [u8]) -> Self {
        // 以 info 为准的边界保护（含 get_unchecked_mut）要求切片足够大。
        let required = info.buffer_size();
        assert!(
            front_buffer.len() >= required,
            "front_buffer too small: need {} bytes (pitch {} x height {}), got {}",
            required,
            info.pitch,
            info.height,
            front_buffer.len()
        );
        #[cfg(feature = "double_buffer")]
        let buffer_size = info.buffer_size();
        let (fmt_bpp, fmt_rgb_order, fmt_opaque_alpha, fmt_stores_color_alpha) = match info.format {
            PixelFormat::Rgb24 => (3, true, 255, false),
            PixelFormat::Bgr24 => (3, false, 255, false),
            PixelFormat::Xrgb8888 => (4, false, 0, false),
            PixelFormat::Argb8888 => (4, false, 255, true),
            PixelFormat::Rgba8888 => (4, true, 255, true),
            PixelFormat::Bgra8888 => (4, false, 255, true),
        };
        Self {
            info,
            front_buffer,
            #[cfg(feature = "double_buffer")]
            back_buffer: alloc::vec![0; buffer_size],
            fmt_bpp,
            fmt_rgb_order,
            fmt_opaque_alpha,
            fmt_stores_color_alpha,
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
    ///
    /// 使用缓存格式属性（`fmt_bpp` / `fmt_rgb_order` / `fmt_opaque_alpha`）代替
    /// `PixelFormat::write_color` 的运行时枚举匹配，消除热路径中的分支开销。
    #[inline(always)]
    pub(crate) fn write_color_at_offset(&mut self, offset: usize, color: &Color) {
        if color.a == 0 {
            return;
        }

        let alpha = color.a as u16;
        let bpp = self.fmt_bpp;
        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        let target = self.current_buffer_mut();
        debug_assert!(
            offset + bpp <= target.len(),
            "write_color_at_offset: offset {offset} + bpp {bpp} exceeds buffer length {}",
            target.len()
        );
        // SAFETY: 调用方保证 offset + bpp <= buffer.len()
        let dst = unsafe { target.get_unchecked_mut(offset..offset + bpp) };

        if alpha == 255 {
            // 不透明快速路径：直接写入，无混合
            write_rgb!(dst, color.r, color.g, color.b, is_rgb_order);
            if bpp == 4 {
                dst[3] = opaque_alpha;
            }
        } else {
            // Alpha 混合路径
            let inv_alpha = 255 - alpha;
            write_rgb_blend!(
                dst,
                color.r,
                color.g,
                color.b,
                alpha,
                inv_alpha,
                is_rgb_order
            );
            if bpp == 4 {
                dst[3] = opaque_alpha;
            }
        }
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
    /// 使用源数据的 pitch（每行字节数）来正确换行。
    /// 自动裁剪到帧缓冲可见区域。
    ///
    /// 当 `src_format` 与帧缓冲格式不匹配、`width`/`height` 为 0、`src_pitch` 小于
    /// `width × bytes_per_pixel`、或 `src` 切片长度不足以容纳 `src_pitch × height`
    /// 字节时，静默返回，不写入任何像素。
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
        // 每行实际读取 width * bpp 字节，故 src_pitch 不得小于该值，
        // 否则 src_start + copy_bytes 会越过行尾（见下方切片索引）。
        if src_pitch < width.saturating_mul(self.info.format.bytes_per_pixel()) {
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
            let dst_ptr = &mut self.current_buffer_mut()[dst_offset..dst_offset + copy_bytes];
            let src_ptr = &src[src_start..src_start + copy_bytes];
            #[cfg(feature = "simd")]
            unsafe {
                simd::memcpy(dst_ptr.as_mut_ptr(), src_ptr.as_ptr(), copy_bytes);
            }
            #[cfg(not(feature = "simd"))]
            dst_ptr.copy_from_slice(src_ptr);
        }
    }

    /// 使用 8bpp 的 mask 将指定颜色绘制到渲染目标。
    ///
    /// `mask` 中每个字节代表对应像素的不透明度（0=透明，255=完全不透明)。
    /// 适用于文本渲染/精灵绘制的前置能力。
    ///
    /// 当 `width`/`height` 为 0、颜色完全透明、`mask_pitch` 小于 `width`、
    /// 或 `mask` 切片长度不足 `mask_pitch × height` 时，静默返回。
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
        // 每行实际读取 width 字节的 mask，故 mask_pitch 不得小于 width，
        // 否则 row_mask_off + col 会越过行尾（见下方 mask 索引）。
        if mask_pitch < width {
            return;
        }
        if mask.len() < mask_pitch.saturating_mul(height) {
            return;
        }
        let fb_width = self.info.width;
        let fb_height = self.info.height;
        let dst_x = dst_top_left.x;
        let bpp = self.fmt_bpp;
        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        let pitch = self.info.pitch;

        let src_r = color.r;
        let src_g = color.g;
        let src_b = color.b;
        let src_alpha = color.a as u16;
        if src_alpha == 0 {
            return;
        }

        // 一次性获取缓冲区裸指针，避免逐像素借用
        let buffer = self.current_buffer_mut();
        let base_ptr = buffer.as_mut_ptr();
        let buf_len = buffer.len();

        for row in 0..height {
            let dy = dst_top_left.y + row;
            if dy >= fb_height {
                break;
            }
            if dst_x >= fb_width {
                continue;
            }
            let visible_cols = width.min(fb_width - dst_x);
            let row_mask_off = row * mask_pitch;
            let base_off = dy * pitch + dst_x * bpp;

            for col in 0..visible_cols {
                let m_alpha = mask[row_mask_off + col];
                if m_alpha == 0 {
                    continue;
                }
                let offset = base_off + col * bpp;
                debug_assert!(offset + bpp <= buf_len, "blit_mask: offset overflow");
                // SAFETY: offset + bpp <= buf_len 由裁剪保证
                let dst = unsafe { base_ptr.add(offset) };
                if m_alpha == 255 {
                    write_rgb_ptr!(dst, src_r, src_g, src_b, is_rgb_order);
                    if bpp == 4 {
                        unsafe { *dst.add(3) = opaque_alpha };
                    }
                } else {
                    // mask byte 直接作为该像素的绘制 alpha
                    let m_alpha_u16 = m_alpha as u16;
                    let m_inv_alpha = 255 - m_alpha_u16;
                    write_rgb_blend_ptr!(dst, src_r, src_g, src_b, m_alpha_u16, m_inv_alpha, is_rgb_order);
                    if bpp == 4 {
                        unsafe { *dst.add(3) = opaque_alpha };
                    }
                }
            }
        }
    }

    /// 用不透明颜色快速填充一行中的水平跨度。
    ///
    /// 对 4bpp 格式，把像素写成 `[u8; 4]` 后按 4 字节块批量填充，
    /// 避免倍增复制的逐字节依赖链。对 3bpp 格式回退到倍增复制策略。
    #[inline(always)]
    pub(crate) fn fill_span_opaque(
        &mut self,
        x_start: usize,
        x_end: usize,
        y: usize,
        color: &Color,
    ) {
        let pixel_size = self.fmt_bpp;
        let start = self.pixel_offset_unchecked(x_start, y);
        let row_len = (x_end - x_start + 1) * pixel_size;
        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        let target = self.current_buffer_mut();
        // SAFETY: 调用方 fill_visible_horizontal_span 保证
        // x_start..=x_end 已裁剪到 [0, width)，y 已裁剪到 [0, height)，
        // 故 start + row_len = pixel_offset(x_end, y) + pixel_size <= buffer.len()。
        let row = unsafe { target.get_unchecked_mut(start..start + row_len) };

        if pixel_size == 4 {
            // 构造 4 字节 pattern，然后按 4 字节块批量填充。
            // 注意：这里必须用 chunks_exact_mut 而不是把 row 的指针转成 *mut u32 后
            // 逐元素写入——`FramebufferInfo` 允许任意 `pitch >= width * bpp`，当
            // `pitch % 4 != 0` 时（如 Bgra8888 配 pitch = width*4 + 1），第二行起的
            // 起始地址不是 4 字节对齐的，解引用 *mut u32 属于未定义行为
            // （x86 上通常能工作，ARM / RISC-V 上可能触发异常）。
            let mut pattern = [0u8; 4];
            write_rgb!(pattern, color.r, color.g, color.b, is_rgb_order);
            pattern[3] = opaque_alpha;
            row.chunks_exact_mut(4).for_each(|pixel| {
                pixel.copy_from_slice(&pattern);
            });
        } else {
            // 3bpp: 单像素模式 + 倍增复制
            write_rgb!(row, color.r, color.g, color.b, is_rgb_order);
            Self::pattern_fill(row, &[row[0], row[1], row[2]]);
        }
    }

    /// 用半透明颜色快速填充一行中的水平跨度。
    ///
    /// 4bpp 格式走 SWAR 批量路径：把 RGB 重排成 `[R,G,B,R]` / `[B,G,R,B]` 形式，
    /// 用 16 位通道并行做 alpha 插值；3bpp 格式因跨字节不对齐，保留逐通道路径。
    ///
    /// # Safety
    ///
    /// 调用方必须保证 `x_start..=x_end` 已裁剪到 `[0, width)`，
    /// `y` 已裁剪到 `[0, height)`，且 `color.a > 0 && color.a < 255`。
    #[inline]
    pub(crate) fn fill_span_alpha(
        &mut self,
        x_start: usize,
        x_end: usize,
        y: usize,
        color: &Color,
    ) {
        debug_assert!(color.a > 0 && color.a < 255);

        let bpp = self.fmt_bpp;
        let start = y * self.info.pitch + x_start * bpp;
        let pixel_count = x_end - x_start + 1;
        let span_len = pixel_count * bpp;

        let buffer = self.current_buffer_mut();
        debug_assert!(start + span_len <= buffer.len());
        let base_ptr = buffer.as_mut_ptr();

        if bpp == 4 {
            // SWAR 路径独立成函数：其代码体明显大于逐像素路径，
            // 若整体内联进 fill_rect，会挤占不透明路径的代码布局。
            unsafe { self.swar_fill_span_alpha_4bpp(base_ptr.add(start), pixel_count, color) };
            return;
        }

        let row = unsafe { base_ptr.add(start) };
        let is_rgb_order = self.fmt_rgb_order;
        let alpha = color.a as u16;
        let inv_alpha = 255 - alpha;
        let (src_r, src_g, src_b) = (color.r, color.g, color.b);

        for pixel_idx in 0..pixel_count {
            let dst = unsafe { row.add(pixel_idx * bpp) };
            write_rgb_blend_ptr!(dst, src_r, src_g, src_b, alpha, inv_alpha, is_rgb_order);
        }
    }

    /// 4bpp 的 SWAR alpha 跨度填充。
    ///
    /// # Safety
    ///
    /// `row` 必须指向 `pixel_count * 4` 字节的可写区域。
    #[inline(never)]
    unsafe fn swar_fill_span_alpha_4bpp(&mut self, row: *mut u8, pixel_count: usize, color: &Color) {
        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        let alpha = color.a as u16;
        let (src_r, src_g, src_b) = (color.r, color.g, color.b);

        // 一次处理 4 个像素（16 字节 = 4 个 u32 字）。
        //
        // 通道插值恒等式（见 `swar_blend_word` 的推导）：
        //     ((s*a + d*ia) + 255) >> 8  ==  (d*255 + a*(s-d) + 255) >> 8
        // 后者把 `a` 提成了公共因子，四个通道的和上界为
        // `255 + 255*255 = 65280 < 2^16`，因此可以安全地并行处理，
        // 不会发生跨字节进位。
        let s_delta = Self::swar_delta_word(src_r, src_g, src_b, opaque_alpha, is_rgb_order);
        let a4 = Self::swar_scale_word(alpha);
        let opaque_word = (opaque_alpha as u32) << 24;

        let quads = pixel_count / 4;
        for quad in 0..quads {
            // SAFETY: quad*16 + 16 <= quads*16 <= pixel_count*4
            let dst = unsafe { row.add(quad * 16) as *mut u32 };
            for lane in 0..4 {
                unsafe {
                    let word = *dst.add(lane);
                    let blended = Self::swar_blend_word(word, s_delta, a4);
                    // 第 4 字节必须恒为 opaque_alpha（Xrgb8888 为 0，
                    // 其余 4bpp 格式为 255），而不是参与混合的结果——
                    // 这与逐像素路径 `*dst.add(3) = opaque_alpha` 一致。
                    *dst.add(lane) = (blended & 0x00FF_FFFF) | opaque_word;
                }
            }
        }

        // 尾部不足 4 像素的部分回退到逐像素路径
        let inv_alpha = 255 - alpha;
        for pixel_idx in (quads * 4)..pixel_count {
            let dst = unsafe { row.add(pixel_idx * 4) };
            write_rgb_blend_ptr!(dst, src_r, src_g, src_b, alpha, inv_alpha, is_rgb_order);
            unsafe { *dst.add(3) = opaque_alpha };
        }
    }

    /// 构造 SWAR 混合用的「源差分」字：每个 16 位通道的值为 `s - d` 的源项部分。
    ///
    /// 由于目标通道 `d` 在运行时才知道，这里只预计算 `s` 部分——
    /// 实际计算在 [`Self::swar_blend_word`] 中与目标字完成合并。
    #[inline(always)]
    fn swar_delta_word(r: u8, g: u8, b: u8, alpha: u8, is_rgb_order: bool) -> u32 {
        // 通道布局与目标字一致：[R,G,B,A] 或 [B,G,R,A]（低字节在前）
        let (c0, c1, c2) = if is_rgb_order { (r, g, b) } else { (b, g, r) };
        (c0 as u32) | ((c1 as u32) << 8) | ((c2 as u32) << 16) | ((alpha as u32) << 24)
    }

    /// 把 8 位标量 `alpha` 复制到 4 个 16 位通道。
    #[inline(always)]
    fn swar_scale_word(alpha: u16) -> u32 {
        let a = alpha as u32;
        a | (a << 16)
    }

    /// 用 SWAR 对 4 个 8 位通道并行做 alpha 插值。
    ///
    /// 推导：设目标字节为 `d`、源字节为 `s`、透明度为 `a`，则
    /// `blend_channel(s, d, a, 255-a) = ((s*a + d*(255-a)) + 255) >> 8`。
    ///
    /// 直接展开会让两个乘积之和达到 `130050 > 2^16`，跨通道进位。
    /// 改用恒等变形 `d*255 + a*(s-d)` 后，四通道和的上界为
    /// `255 + 255*255 = 65280 < 2^16`，可安全并行。
    ///
    /// 每个 8 位通道在这里被展宽到 16 位处理：`word` 的字节 `i` 作为
    /// 16 位通道 `i` 的低字节，高字节由 `d255` 提供的 255 与 `s_delta`
    /// 的源字节相乘后填充。
    #[inline(always)]
    fn swar_blend_word(word: u32, s_delta: u32, a4: u32) -> u32 {
        // 逐通道拆分到 16 位槽：d = (word >> 8i) & 0xFF
        let d0 = word & 0xFF;
        let d1 = (word >> 8) & 0xFF;
        let d2 = (word >> 16) & 0xFF;
        let d3 = (word >> 24) & 0xFF;

        let s0 = s_delta & 0xFF;
        let s1 = (s_delta >> 8) & 0xFF;
        let s2 = (s_delta >> 16) & 0xFF;
        let s3 = (s_delta >> 24) & 0xFF;

        let a = a4 & 0xFFFF;

        // 每通道: (d*255 + a*(s - d) + 255) >> 8
        // 用 i32 计算以避免 u32 下溢（s - d 可能为负）。
        let blend = |d: u32, s: u32| -> u32 {
            let v = d as i32 * 255 + a as i32 * (s as i32 - d as i32);
            (((v + 255) >> 8) as u32) & 0xFF
        };

        blend(d0, s0) | (blend(d1, s1) << 8) | (blend(d2, s2) << 16) | (blend(d3, s3) << 24)
    }

    /// 在可见区域内填充水平跨度。自动裁剪到帧缓冲边界。
    ///
    /// 不透明颜色使用 `fill_span_opaque` 快速填充；
    /// 半透明颜色使用 `fill_span_alpha` 行级批量处理；
    /// 完全透明颜色直接跳过。
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
        if color.a == 0 {
            // 完全透明，跳过
            return;
        }
        if color.a == 255 {
            self.fill_span_opaque(sx as usize, ex as usize, y as usize, color);
        } else {
            self.fill_span_alpha(sx as usize, ex as usize, y as usize, color);
        }
    }

    /// 扫描线插值：给定基坐标 `base_x`、增量 `delta_x` / `delta_y` 和
    /// 垂直偏移 `offset_y`，计算对应的 X 坐标。
    ///
    /// 使用 `i64` 饱和运算代替 `i128`，在保证安全的同时提升除法性能（~2-3×）。
    #[inline(always)]
    fn interpolate_scan_x(base_x: i64, delta_x: i64, delta_y: i64, offset_y: i64) -> i64 {
        if delta_y == 0 {
            return base_x;
        }

        base_x + (delta_x.saturating_mul(offset_y) + delta_y / 2) / delta_y
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
        let pixel_size = self.info.format.bytes_per_pixel();
        let width = self.info.width;
        let height = self.info.height;
        let row_len = width * pixel_size;

        if row_len == 0 || height == 0 {
            return;
        }

        unsafe {
            self.clear_unchecked(&color, row_len, height);
        }
    }

    /// 用重复像素模式填充当前缓冲区的可见区域。
    ///
    /// 先把单个像素展开成 16 字节宽的模式描述符，再用一次定长写入铺满整个
    /// 缓冲区，避免逐像素的枚举匹配与倍增复制的写入放大。
    ///
    /// # Safety
    ///
    /// 调用方必须保证 `row_len > 0` 且 `row_len` 是当前格式的整数倍像素宽度。
    unsafe fn clear_unchecked(&mut self, color: &Color, row_len: usize, height: usize) {
        let pitch = self.info.pitch;
        let total_bytes = pitch * height;

        // 16 字节模式描述符。对 4bpp 可铺满整个缓冲，把每像素一次写入降为
        // 每 16 字节一次写入；对 3bpp 退回单像素模式。
        let (pattern, pattern_len) = self.build_pattern(color);

        // 缓冲区可能小于一个完整模式（例如 1×1 的 4bpp 只有 4 字节），
        // 此时只能写入实际存在的字节数，否则会越界。
        let init_len = pattern_len.min(total_bytes);

        let buffer = self.current_buffer_mut();
        let base_ptr = buffer.as_mut_ptr();

        unsafe {
            #[cfg(feature = "simd")]
            simd::memcpy(base_ptr, pattern.as_ptr(), init_len);
            #[cfg(not(feature = "simd"))]
            core::ptr::copy_nonoverlapping(pattern.as_ptr(), base_ptr, init_len);
        }

        if pitch == row_len {
            // 无行间填充：一次铺满整个 buffer
            let full_buf = unsafe { core::slice::from_raw_parts_mut(base_ptr, total_bytes) };
            Self::pattern_fill(full_buf, &pattern[..init_len]);
        } else {
            // 有行间填充：先铺满第一行，再逐行复制
            let first_row = unsafe { core::slice::from_raw_parts_mut(base_ptr, row_len) };
            Self::pattern_fill(first_row, &pattern[..init_len.min(row_len)]);

            for y in 1..height {
                let row_ptr = unsafe { base_ptr.add(y * pitch) };
                unsafe {
                    #[cfg(feature = "simd")]
                    simd::memcpy(row_ptr, base_ptr, row_len);
                    #[cfg(not(feature = "simd"))]
                    core::ptr::copy_nonoverlapping(base_ptr, row_ptr, row_len);
                }
            }
        }
    }

    /// 构造像素模式缓冲，返回 `(pattern, pattern_len)`。
    ///
    /// `pattern_len` 为 16 表示模式可一次铺满 16 字节；否则等于 `bpp`。
    /// 参数从 `self` 的缓存格式属性与给定颜色派生。
    #[inline(always)]
    fn build_pattern(&self, color: &Color) -> ([u8; 16], usize) {
        let bpp = self.fmt_bpp;
        let is_rgb_order = self.fmt_rgb_order;
        let color_alpha = color.a;
        let stores_alpha = self.fmt_stores_color_alpha;
        let opaque_alpha = self.fmt_opaque_alpha;
        let mut pattern = [0u8; 16];
        if bpp == 0 || bpp > 4 {
            return (pattern, bpp);
        }

        // 单像素的规范字节布局（含 alpha 处理），通过 write_rgb! 统一字节序。
        let mut pixel = [0u8; 4];
        write_rgb!(pixel, color.r, color.g, color.b, is_rgb_order);
        if bpp == 4 {
            pixel[3] = if stores_alpha { color_alpha } else { opaque_alpha };
        }

        // 4bpp：像素 4 字节，恰好铺满 16 字节。
        if bpp == 4 {
            let mut offset = 0;
            while offset < 16 {
                pattern[offset..offset + 4].copy_from_slice(&pixel);
                offset += 4;
            }
            return (pattern, 16);
        }

        // 2bpp / 1bpp：由 2 或 1 字节组成，均可整除 16。
        if bpp == 2 || bpp == 1 {
            let reps = 16 / bpp;
            for i in 0..reps {
                pattern[i * bpp..(i + 1) * bpp].copy_from_slice(&pixel[..bpp]);
            }
            return (pattern, 16);
        }

        // 3bpp 无法整除 16，退回单像素模式。
        pattern[..3].copy_from_slice(&pixel[..3]);
        (pattern, 3)
    }

    /// 用重复模式填充整个切片。
    ///
    /// 对 4bpp 等能整除 16 的格式，模式固定为 16 字节：按 16 字节块**直接重复写入**，
    /// 而不是"复制已填部分来翻倍"。后者需要 `O(log n)` 次串行 `memcpy`，
    /// 每次的源区域都不同、无法向量化，且总写入量随步数累积；直接块写是
    /// 单一 stready-state 循环，流水线友好。
    ///
    /// 3bpp 无法整除 16，退回逐次倍增复制。
    #[inline(always)]
    fn pattern_fill(row: &mut [u8], pattern: &[u8]) {
        let len = row.len();
        let pattern_len = pattern.len();
        if pattern_len == 0 || len <= pattern_len {
            return;
        }

        // 16 字节定长块：直接铺满，无倍增链
        if pattern_len == 16 {
            let mut block = [0u8; 16];
            block.copy_from_slice(&pattern[..16]);
            let mut offset = 16;
            while offset + 16 <= len {
                row[offset..offset + 16].copy_from_slice(&block);
                offset += 16;
            }
            if offset < len {
                let tail = len - offset;
                row[offset..].copy_from_slice(&block[..tail]);
            }
            return;
        }

        // 非 16 字节模式（3bpp）：逐次倍增复制
        let base = row.as_mut_ptr();
        let mut filled = pattern_len;
        while filled < len {
            let copy_len = core::cmp::min(filled, len - filled);
            unsafe {
                #[cfg(feature = "simd")]
                simd::memcpy(base.add(filled), base, copy_len);
                #[cfg(not(feature = "simd"))]
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
    ///
    /// 先用 Cohen–Sutherland 裁剪（`clip_line`）对整条线段做快速拒绝，再交给
    /// `draw_line_clipped_internal`。该内部函数会把迭代范围收窄到可见区间，
    /// 因此工作量与**线段总长度无关**，只与可见部分的像素数成正比。
    ///
    /// 收窄不会改变像素覆盖：Bresenham 的递推是确定的，内部函数保留原始端点
    /// 与完整的相位，仅跳过必然越界的迭代。
    pub fn draw_line(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
        if self.info.width == 0 || self.info.height == 0 {
            return;
        }
        if clip_line(p1, p2, self.info.width, self.info.height).is_none() {
            return;
        }
        self.draw_line_clipped_internal(p1, p2, color);
    }

    /// Bresenham 线段绘制（内部快速路径）。
    ///
    /// 预取缓冲区裸指针，内循环直接指针写入，避免逐像素调用
    /// `write_color_if_visible_i64` 的函数调用与格式匹配开销。
    ///
    /// # 迭代范围收窄
    ///
    /// 外层 `draw_line` 只会拒绝完全不可见的线段，可见线段仍可能带有极长的
    /// 越界部分（例如端点来自 `usize` 上的饱和算术）。若从原始端点一路步进，
    /// 代价是 O(线段长度)——在实时系统里这是一个可被外部输入触发的长耗时路径。
    ///
    /// 因此这里先做两件事：
    ///
    /// 1. **主轴收窄**：主循环变量 `x` 对应原始坐标 `(px, py) = steep ? (y, x) : (x, y)`，
    ///    因此它的可见区间是 `[0, width) `（非 steep）或 `[0, height)`（steep）。
    ///    把迭代区间直接收窄为该区间与 `[x1, x2]` 的交集。
    /// 2. **相位重算**：`error` 与 `y` 的递推是确定的，可以从 `x1` 精确重演到新的
    ///    起点 `x_lo`，得到与完整步进完全一致的中间状态。
    ///
    /// 次轴仍在内循环中逐像素检查，故覆盖与未收窄时逐像素相同。
    #[inline(always)]
    fn draw_line_clipped_internal(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
        // 用饱和转换（而非 `as i64`）把 `usize` 坐标转到 `i64`：`as` 会把
        // 大于 `i64::MAX` 的值回绕成负数（如 `usize::MAX as i64 == -1`），
        // 使端点被错误地折到画布另一侧，覆盖结果完全错误。
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
        let dy = (y2 - y1).unsigned_abs() as i64;
        let y_step = if y1 < y2 { 1i64 } else { -1i64 };
        let width = self.info.width as i64;
        let height = self.info.height as i64;

        // 主循环变量 `x` 与次轴变量 `y` 各自对应哪条原始坐标轴，由 `steep` 决定：
        //   steep = false → (px, py) = (x, y)  → 主轴 x 是原始 x，次轴 y 是原始 y
        //   steep = true  → (px, py) = (y, x)  → 主轴 x 是原始 y，次轴 y 是原始 x
        // 因此：主轴的上界 = width-1（非 steep）/ height-1（steep）；
        //       次轴的上界 = height-1（非 steep）/ width-1（steep）。
        let (x_hi_bound, y_hi_bound) = if steep {
            (height - 1, width - 1)
        } else {
            (width - 1, height - 1)
        };

        // 收窄迭代区间；与 [x1, x2] 求交后若为空则整条线段不可见。
        // 下界恒为 0（坐标非负），上界由主轴决定。
        let x_lo = x1.max(0);
        let x_hi = x2.min(x_hi_bound);
        if x_lo > x_hi {
            return;
        }

        // 从 x1 重演递推到 x_lo，恢复 error 与 y 的相位。
        // 这里的算术与下方主循环逐字一致，保证相位完全相同。
        let mut error = dx / 2;
        let mut y = y1;
        for _ in x1..x_lo {
            error -= dy;
            if error < 0 {
                y += y_step;
                error += dx;
            }
        }

        let bpp = self.fmt_bpp;
        let pitch = self.info.pitch;
        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        let alpha = color.a as u16;
        let inv_alpha = 255 - alpha;
        let buffer = self.current_buffer_mut();
        let base_ptr = buffer.as_mut_ptr();
        for x in x_lo..=x_hi {
            let (px, py) = if steep { (y, x) } else { (x, y) };
            // 主轴（x）已由区间收窄保证在 [0, x_hi_bound] 内；次轴（y）在这里
            // 单独检查其上界。下界恒为 0：坐标非负，且 y 从 y1 >= 0 出发，
            // 步进方向 y_step 只会让它朝 y2 所在的一侧移动，不会越过 0。
            if y <= y_hi_bound {
                let offset = py as usize * pitch + px as usize * bpp;
                let dst = unsafe { base_ptr.add(offset) };
                if alpha == 0 {
                    // 完全透明：跳过写入
                } else if alpha == 255 {
                    write_rgb_ptr!(dst, color.r, color.g, color.b, is_rgb_order);
                    if bpp == 4 {
                        unsafe { *dst.add(3) = opaque_alpha };
                    }
                } else {
                    write_rgb_blend_ptr!(
                        dst,
                        color.r,
                        color.g,
                        color.b,
                        alpha,
                        inv_alpha,
                        is_rgb_order
                    );
                    if bpp == 4 {
                        unsafe { *dst.add(3) = opaque_alpha };
                    }
                }
            }
            error -= dy;
            if error < 0 {
                y += y_step;
                error += dx;
            }
        }
    }

    /// Bresenham 线段算法，**不进行裁剪检查**。
    ///
    /// 仅通过 `write_color_if_visible_i64` 进行逐个像素的边界保护，
    /// 不会通过 `clip_line` 提前拒绝整条线段。
    ///
    /// # Safety
    ///
    /// 调用方必须保证渲染器绑定的帧缓冲有效。
    #[inline]
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
        } else if color.a > 0 {
            for y in y_start..y_max {
                self.fill_span_alpha(x_start, x_max - 1, y, &color);
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
    #[inline]
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
    // 测试需要堆分配（部分用 vec! 构造帧缓冲，draw_line 回归测试构造像素列表）
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

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

    // ── clear 缓冲区边界（安全性回归） ──

    /// `clear` 在缓冲区小于一个填充模式时不得越界写。
    ///
    /// 4bpp 的填充模式是 16 字节；1×1 的 4bpp 缓冲区只有 4 字节，
    /// 若不把模式写入长度收敛到 `pitch * height`，会越界写 12 字节。
    ///
    /// 用 canary 检测：缓冲区后紧跟守卫字节，写入越界会破坏守卫。
    #[test]
    fn clear_does_not_write_past_tiny_buffer() {
        #[repr(C)]
        struct Guarded {
            buf: [u8; 4],
            canary: [u8; 32],
        }

        for (format, bpp, pitch, w, h) in [
            (PixelFormat::Rgba8888, 4usize, 4usize, 1usize, 1usize),
            (PixelFormat::Bgra8888, 4, 4, 1, 1),
            (PixelFormat::Argb8888, 4, 4, 1, 1),
            (PixelFormat::Xrgb8888, 4, 4, 1, 1),
            (PixelFormat::Rgb24, 3, 3, 1, 1),
            (PixelFormat::Bgr24, 3, 3, 1, 1),
        ] {
            let _ = bpp;
            let mut g = Guarded {
                buf: [0u8; 4],
                canary: [0xAAu8; 32],
            };
            let info = FramebufferInfo::new(w, h, pitch, format);
            {
                let mut r = Renderer::new(info, &mut g.buf);
                r.clear(Color::new(1, 2, 3));
            }
            assert_eq!(
                g.canary, [0xAAu8; 32],
                "{format:?} ({w}x{h}, pitch {pitch}) 的 clear 越界写了 canary"
            );
        }
    }

    /// `clear` 在极小缓冲区下的实际像素值应正确（不只是不越界）。
    #[test]
    fn clear_fills_tiny_buffer_correctly() {
        let info = FramebufferInfo::new(1, 1, 4, PixelFormat::Rgba8888);
        let mut front = [0u8; 4];
        let mut renderer = Renderer::new(info, &mut front);
        renderer.clear(Color::new(1, 2, 3));
        let got = renderer.get_pixel(0, 0).unwrap();
        assert_eq!((got.r, got.g, got.b), (1, 2, 3));
    }

    // ── SWAR 混合路径等价性 ──

    /// SWAR 混合必须与逐通道 `blend_channel` 逐字节一致。
    ///
    /// 覆盖全部 255 个有效 alpha 值 × 广泛的 src/dst 组合，包含会被
    /// 4 像素分组与不足 4 像素的尾部两条分支。
    #[test]
    fn swar_alpha_blend_matches_per_channel_reference() {
        fn reference(src: Color, dst: Color, alpha: u16) -> Color {
            let inv = 255 - alpha;
            Color::with_alpha(
                PixelFormat::blend_channel(src.r, dst.r, alpha, inv),
                PixelFormat::blend_channel(src.g, dst.g, alpha, inv),
                PixelFormat::blend_channel(src.b, dst.b, alpha, inv),
                255,
            )
        }

        // 5 像素宽度：4 像素走 SWAR，1 像素走尾部回退分支。
        const WIDTH: usize = 5;
        const HEIGHT: usize = 1;
        let make_info = || FramebufferInfo::new(WIDTH, HEIGHT, WIDTH * 4, PixelFormat::Rgba8888);

        let src_colors = [
            Color::new(0, 0, 0),
            Color::new(255, 255, 255),
            Color::new(255, 0, 0),
            Color::new(0, 255, 0),
            Color::new(17, 201, 99),
        ];
        let dst_colors = [
            Color::new(0, 0, 0),
            Color::new(255, 255, 255),
            Color::new(255, 0, 0),
            Color::new(1, 128, 254),
            Color::new(200, 33, 77),
        ];

        // 单个 renderer 内完成：先种 dst，再叠加半透明 src，全程操作同一缓冲区。
        // （不能创建两个 Renderer——`double_buffer` 下各自持有独立的 back_buffer。）
        for alpha in 1u8..255 {
            for src in src_colors {
                for dst in dst_colors {
                    let mut front = [0u8; WIDTH * 4];
                    let mut renderer = Renderer::new(make_info(), &mut front);
                    for x in 0..WIDTH {
                        renderer.set_pixel(x, 0, &dst);
                    }
                    renderer.fill_rect(Pixel::new(0, 0), WIDTH, 1, src.mix_alpha(alpha));

                    // fill_rect 走的是填充路径（非 set_pixel 的 alpha 分支），
                    // 期望值用同一套 blend_channel 语义推导。
                    let want = reference(src, dst, alpha as u16);
                    for x in 0..WIDTH {
                        let got = renderer.get_pixel(x, 0).expect("列在范围内");
                        // 同时校验第 4 字节：4bpp 混合后必须被强制为 opaque_alpha，
                        // 而不是参与插值的结果（曾因此漏掉一个真实 bug）。
                        let off = x * 4;
                        assert_eq!(
                            (got.r, got.g, got.b),
                            (want.r, want.g, want.b),
                            "alpha={alpha} src={src:?} dst={dst:?} col={x}"
                        );
                        assert_eq!(
                            renderer.buffer_mut()[off + 3],
                            255,
                            "Rgba8888 第 4 字节应为 255, alpha={alpha} col={x}"
                        );
                    }
                }
            }
        }
    }

    /// 同一行内 4 像素分组与尾部回退必须产出相同结果（无分组边界错位）。
    #[test]
    fn swar_alpha_blend_row_is_uniform_for_uniform_target() {
        const WIDTH: usize = 9; // 2 组 SWAR + 1 像素尾部
        let info = FramebufferInfo::new(WIDTH, 1, WIDTH * 4, PixelFormat::Rgba8888);
        let mut front = [0u8; WIDTH * 4];
        let mut renderer = Renderer::new(info, &mut front);
        for x in 0..WIDTH {
            renderer.set_pixel(x, 0, &Color::new(64, 128, 192));
        }
        renderer.fill_rect(
            Pixel::new(0, 0),
            WIDTH,
            1,
            Color::with_alpha(200, 30, 90, 77),
        );
        let first = renderer.get_pixel(0, 0).unwrap();
        for x in 0..WIDTH {
            assert_eq!(renderer.get_pixel(x, 0), Some(first), "列 {x} 与列 0 不一致");
        }
    }

    /// 4bpp 的 alpha 填充必须把第 4 字节强制成该格式的 `opaque_alpha`，
    /// 而不是让它参与插值。
    ///
    /// `Xrgb8888` 的 `opaque_alpha` 是 0，其余 4bpp 格式是 255。SWAR 路径
    /// 早期版本对第 4 字节也做了混合，导致 Xrgb8888 得到非 0 的残留值。
    #[test]
    fn alpha_fill_forces_opaque_alpha_byte_per_format() {
        for (format, expected) in [
            (PixelFormat::Xrgb8888, 0u8),
            (PixelFormat::Rgba8888, 255),
            (PixelFormat::Bgra8888, 255),
            (PixelFormat::Argb8888, 255),
        ] {
            const W: usize = 8;
            let info = FramebufferInfo::new(W, 1, W * 4, format);
            let mut front = [0u8; W * 4];
            let mut renderer = Renderer::new(info, &mut front);
            // 先把第 4 字节种成非零，确保填充会覆写它
            for x in 0..W {
                renderer.buffer_mut()[x * 4 + 3] = 0x7F;
            }
            // 宽度 8 = 2 组 SWAR，无尾部分支，专门覆盖 SWAR 路径
            renderer.fill_rect(
                Pixel::new(0, 0),
                W,
                1,
                Color::with_alpha(0x12, 0x34, 0x56, 128),
            );
            for x in 0..W {
                assert_eq!(
                    renderer.buffer_mut()[x * 4 + 3],
                    expected,
                    "{format:?} 第 {x} 列第 4 字节应为 {expected}"
                );
            }
        }
    }

    // ── 输入校验（安全性回归） ──

    #[test]
    #[should_panic(expected = "front_buffer too small")]
    fn new_rejects_undersized_front_buffer() {
        // 元数据声称 4x4（需要 48 字节），但切片只有一行的大小。
        // 若不校验，单缓冲 + release 下 set_pixel 会越界写（UB）。
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 12];
        let _ = Renderer::new(info, &mut front);
    }

    #[test]
    fn new_accepts_buffer_with_pitch_padding() {
        // pitch > width * bpp 时 buffer_size > width * height * bpp，属合法布局
        let info = FramebufferInfo::new(4, 4, 16, PixelFormat::Bgr24);
        let mut front = [0u8; 64];
        let mut renderer = Renderer::new(info, &mut front);
        renderer.set_pixel(3, 3, &Color::new(0x12, 0x34, 0x56));
        assert_eq!(
            renderer.get_pixel(3, 3),
            Some(Color::new(0x12, 0x34, 0x56))
        );
    }

    #[test]
    fn blit_rejects_src_pitch_smaller_than_row() {
        let info = FramebufferInfo::new(4, 2, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 24];
        let mut renderer = Renderer::new(info, &mut front);
        renderer.set_pixel(1, 0, &Color::new(255, 255, 255));
        // width=4, bpp=3 → 每行需要 12 字节，但 src_pitch=3。
        // src.len() 满足 3*1，因此旧实现会越过行尾 panic。
        let src = [0u8; 6];

        renderer.blit(Pixel::new(0, 0), 4, 1, PixelFormat::Bgr24, &src, 3);

        // 静默拒绝，原有像素不变
        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn blit_mask_rejects_mask_pitch_smaller_than_width() {
        let info = FramebufferInfo::new(4, 1, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 12];
        let mut renderer = Renderer::new(info, &mut front);
        renderer.set_pixel(1, 0, &Color::new(255, 255, 255));
        // width=4 但 mask_pitch=1；mask.len() 满足 1*1。
        let mask = [0u8; 2];

        renderer.blit_mask(Pixel::new(0, 0), 4, 1, &mask, 1, Color::new(255, 255, 255));

        assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn fill_rect_handles_unaligned_pitch() {
        // pitch = width*4 + 1 → 第 2 行起的目标指针不是 4 字节对齐。
        // 旧实现把行指针转成 *mut u32 后写入，属于未定义行为
        // （ARM / RISC-V 上可能触发异常）。
        let width = 8usize;
        let pitch = width * 4 + 1;
        let info = FramebufferInfo::new(width, 4, pitch, PixelFormat::Rgba8888);
        let mut front = vec![0u8; info.buffer_size()];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0x12, 0x34, 0x56);

        renderer.fill_rect(Pixel::new(0, 0), width, 4, color);

        for y in 0..4 {
            for x in 0..width {
                assert_eq!(renderer.get_pixel(x, y), Some(color), "pixel ({x}, {y})");
            }
        }
    }

    // ── draw_line 的迭代范围收窄 ──

    #[test]
    fn draw_line_with_huge_endpoint_stays_cheap() {
        // 端点在 usize::MAX 量级时必须完成得很快：迭代范围收窄后
        // 工作量只与可见像素数成正比，而非与线段长度成正比。
        // （收窄前 x 方向 2 亿的线段约需 586 ms。）
        let info = FramebufferInfo::new(64, 64, 64 * 3, PixelFormat::Bgr24);
        let mut front = [0u8; 64 * 64 * 3];
        let mut renderer = Renderer::new(info, &mut front);
        let color = Color::new(0xFF, 0xFF, 0xFF);

        renderer.draw_line(Pixel::new(0, 0), Pixel::new(usize::MAX, usize::MAX), &color);
        renderer.draw_line(Pixel::new(0, 0), Pixel::new(usize::MAX, 0), &color);
        renderer.draw_line(Pixel::new(0, 0), Pixel::new(0, usize::MAX), &color);

        // 对角线方向的可见部分应被绘制
        assert_eq!(renderer.get_pixel(0, 0), Some(color));
        assert_eq!(renderer.get_pixel(63, 63), Some(color));
    }

    #[test]
    fn draw_line_iteration_narrowing_preserves_coverage() {
        // 参考实现：从原始端点跑完整 Bresenham，仅过滤越界像素
        // （即收窄之前的旧行为）。收窄后必须逐像素一致。
        fn reference(p1: (i64, i64), p2: (i64, i64), w: i64, h: i64) -> Vec<(i64, i64)> {
            let (mut x1, mut y1) = p1;
            let (mut x2, mut y2) = p2;
            let steep = (y2 - y1).abs() > (x2 - x1).abs();
            if steep {
                core::mem::swap(&mut x1, &mut y1);
                core::mem::swap(&mut x2, &mut y2);
            }
            if x1 > x2 {
                core::mem::swap(&mut x1, &mut x2);
                core::mem::swap(&mut y1, &mut y2);
            }
            let (dx, dy) = (x2 - x1, (y2 - y1).abs());
            let mut error = dx / 2;
            let y_step = if y1 < y2 { 1 } else { -1 };
            let mut y = y1;
            let mut out = Vec::new();
            for x in x1..=x2 {
                let (px, py) = if steep { (y, x) } else { (x, y) };
                if px >= 0 && px < w && py >= 0 && py < h {
                    out.push((px, py));
                }
                error -= dy;
                if error < 0 {
                    y += y_step;
                    error += dx;
                }
            }
            out
        }

        let (w, h) = (16i64, 16i64);
        let color = Color::new(0xFF, 0xFF, 0xFF);
        // 覆盖水平、垂直、两方向对角、极陡 / 极缓，以及大量越界终点
        let pts: [(i64, i64); 12] = [
            (0, 0),
            (1, 0),
            (0, 1),
            (3, 7),
            (8, 0),
            (7, 9),
            (15, 15),
            (16, 0),
            (0, 16),
            (40, 3),
            (100, 100),
            (300, 7),
        ];

        for &(ax, ay) in &pts {
            for &(bx, by) in &pts {
                let info =
                    FramebufferInfo::new(w as usize, h as usize, w as usize * 3, PixelFormat::Bgr24);
                let mut front = [0u8; 16 * 16 * 3];
                let mut renderer = Renderer::new(info, &mut front);
                renderer.draw_line(
                    Pixel::new(ax as usize, ay as usize),
                    Pixel::new(bx as usize, by as usize),
                    &color,
                );

                let mut drawn = Vec::new();
                for y in 0..h {
                    for x in 0..w {
                        if renderer.get_pixel(x as usize, y as usize) == Some(color) {
                            drawn.push((x, y));
                        }
                    }
                }
                let mut expected = reference((ax, ay), (bx, by), w, h);
                drawn.sort_unstable();
                expected.sort_unstable();
                assert_eq!(
                    drawn, expected,
                    "coverage mismatch for ({ax},{ay})->({bx},{by})"
                );
            }
        }
    }
}

