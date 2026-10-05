use crate::angle::{Angle, Wedge};
use crate::color::{Color, blend_565, decode_565_bytes, encode_565_bytes, pack_565};
use crate::framebuffer::{FramebufferInfo, PixelFormat};
use crate::pixel::Pixel;
#[cfg(feature = "simd")]
use crate::simd;
use crate::utils::{clip_line, pattern_fill, sat_i64};

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

/// 从裸指针读取一个 16 位打包像素（`Rgb565` / `Bgr565`）。
///
/// 逐字节访问而非按 `u16` 解引用：`pitch` 允许是奇数，行首不保证 2 字节对齐。
///
/// # Safety
///
/// 调用方必须保证 `dst` 起至少 2 个字节可读。
#[inline(always)]
unsafe fn load_565(dst: *const u8, swapped: bool) -> u16 {
    // SAFETY: 由调用方保证 dst..dst+2 可读。
    let bytes = unsafe { [*dst, *dst.add(1)] };
    decode_565_bytes(&bytes, swapped)
}

/// 把一个 16 位打包像素写到裸指针（`Rgb565` / `Bgr565`）。
///
/// 与 [`load_565`] 对称，同样不要求 2 字节对齐。
///
/// # Safety
///
/// 调用方必须保证 `dst` 起至少 2 个字节可写。
#[inline(always)]
unsafe fn store_565(dst: *mut u8, word: u16, swapped: bool) {
    let bytes = encode_565_bytes(word, swapped);
    // SAFETY: 由调用方保证 dst..dst+2 可写。
    unsafe {
        *dst = bytes[0];
        *dst.add(1) = bytes[1];
    }
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
    /// 后台缓冲区。`Some` 表示启用双缓冲：所有绘制写入此切片，
    /// 由 [`Renderer::swap_buffers`] 逐行 present 到 `front_buffer`。
    back_buffer: Option<&'a mut [u8]>,

    // --- 缓存的像素格式属性（在 new() 中初始化一次，消除热路径中的格式匹配）---
    pub(crate) fmt_bpp: usize,
    pub(crate) fmt_rgb_order: bool,
    pub(crate) fmt_opaque_alpha: u8,
    pub(crate) fmt_stores_color_alpha: bool,
    /// 16 位打包格式（`Rgb565` / `Bgr565`）的存储字节序是否为「高字节在前」。
    ///
    /// 仅 `Bgr565` 为 `true`；8 位通道格式的字节序已由各自的 shift 表达，
    /// 在 2bpp 路径之外不会被读取。
    pub(crate) fmt_bytes_swapped: bool,
}

impl<'a> Renderer<'a> {
    /// 创建单缓冲渲染器。
    ///
    /// * `info` — 帧缓冲布局描述。
    /// * `front_buffer` — 前端帧缓冲的可变引用。
    ///
    /// 绘制直接写入 `front_buffer`，[`Renderer::swap_buffers`] 是空操作。
    /// 需要双缓冲时用 [`Renderer::with_back_buffer`]。
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
        Self::build(info, front_buffer, None)
    }

    /// 创建双缓冲渲染器，后台缓冲区由调用方提供。
    ///
    /// * `info` — 帧缓冲布局描述，同时描述前台与后台缓冲区。
    /// * `front_buffer` — 前端帧缓冲，[`Renderer::swap_buffers`] 的目标。
    /// * `back_buffer` — 后端帧缓冲，所有绘制命令的落点。
    ///
    /// 两个切片都必须能容纳 `pitch × height` 字节，且必须互不重叠——
    /// 后者由 Rust 的可变借用规则在编译期保证（同一个数组的两段可用
    /// `split_at_mut` 切开后分别传入）。
    ///
    /// # 恐慌
    ///
    /// 如果 `front_buffer` 或 `back_buffer` 短于 `info.buffer_size()` 则恐慌，
    /// 判定口径与 [`Renderer::new`] 相同。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::Renderer;
    /// # use rasteron::framebuffer::FramebufferInfo;
    /// # use rasteron::color::{Color, PixelFormat};
    /// let info = FramebufferInfo::new(16, 16, 48, PixelFormat::Bgr24);
    /// // 无分配器环境：前后台来自同一块静态内存的两段。
    /// let mut storage = [0u8; 16 * 48 * 2];
    /// {
    ///     let (front, back) = storage.split_at_mut(16 * 48);
    ///     let mut renderer = Renderer::with_back_buffer(info, front, back);
    ///     renderer.clear(Color::new(0, 0, 255));
    ///     // 绘制落在后台（Bgr24 字节序为 B,G,R），前台尚未被触碰
    ///     assert_eq!(&renderer.buffer()[..3], &[255, 0, 0]);
    ///     renderer.swap_buffers();
    /// }
    /// // 渲染器离开作用域后，前台已是呈现后的蓝色
    /// assert_eq!(&storage[..3], &[255, 0, 0]);
    /// ```
    pub fn with_back_buffer(
        info: FramebufferInfo,
        front_buffer: &'a mut [u8],
        back_buffer: &'a mut [u8],
    ) -> Self {
        Self::build(info, front_buffer, Some(back_buffer))
    }

    /// 构造函数的共用实现：校验两个缓冲区的尺寸并缓存格式属性。
    ///
    /// `back_buffer` 为 `None` 即单缓冲。
    fn build(
        info: FramebufferInfo,
        front_buffer: &'a mut [u8],
        back_buffer: Option<&'a mut [u8]>,
    ) -> Self {
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
        if let Some(back) = &back_buffer {
            assert!(
                back.len() >= required,
                "back_buffer too small: need {} bytes (pitch {} x height {}), got {}",
                required,
                info.pitch,
                info.height,
                back.len()
            );
        }
        let (fmt_bpp, fmt_rgb_order, fmt_opaque_alpha, fmt_stores_color_alpha, fmt_bytes_swapped) =
            match info.format {
                PixelFormat::Rgb24 => (3, true, 255, false, false),
                PixelFormat::Bgr24 => (3, false, 255, false, false),
                PixelFormat::Xrgb8888 => (4, false, 0, false, false),
                PixelFormat::Argb8888 => (4, false, 255, true, false),
                PixelFormat::Rgba8888 => (4, true, 255, true, false),
                PixelFormat::Bgra8888 => (4, false, 255, true, false),
                // 565 没有逐字节的 RGB 顺序概念，2bpp 路径不使用 `fmt_rgb_order`，
                // 这里填 `true` 仅为占位；通道位域由 `color::pack_565` 固定。
                PixelFormat::Rgb565 => (2, true, 255, false, false),
                PixelFormat::Bgr565 => (2, true, 255, false, true),
            };
        Self {
            info,
            front_buffer,
            back_buffer,
            fmt_bpp,
            fmt_rgb_order,
            fmt_opaque_alpha,
            fmt_stores_color_alpha,
            fmt_bytes_swapped,
        }
    }

    /// 是否处于双缓冲模式（即构造时提供了后台缓冲区）。
    #[inline(always)]
    pub fn is_double_buffered(&self) -> bool {
        self.back_buffer.is_some()
    }

    /// 返回当前可写入的缓冲区切片（内部使用）。
    ///
    /// 双缓冲模式下返回后台缓冲区，否则返回前台缓冲区。
    #[inline(always)]
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        if let Some(back) = self.back_buffer.as_deref_mut() {
            return back;
        }
        &mut self.front_buffer[..]
    }

    /// 返回当前可读取的缓冲区切片（内部使用）。
    ///
    /// 双缓冲模式下返回后台缓冲区，否则返回前台缓冲区。
    #[inline(always)]
    pub fn buffer(&self) -> &[u8] {
        if let Some(back) = self.back_buffer.as_deref() {
            return back;
        }
        &self.front_buffer[..]
    }

    /// 每个像素占用的字节数。
    #[inline(always)]
    pub fn pixel_size(&self) -> usize {
        self.info.bytes_per_pixel() as usize
    }

    /// 计算像素 `(x, y)` 在缓冲区中的字节偏移。
    #[inline(always)]
    pub fn pixel_offset(&self, x: usize, y: usize) -> usize {
        y * self.info.pitch + x * self.info.bytes_per_pixel()
    }

    /// 在指定字节偏移处写入颜色（支持 Alpha 混合）。
    ///
    /// 如果 `color.a == 0`，直接返回，不执行任何写入。
    ///
    /// 使用缓存格式属性（`fmt_bpp` / `fmt_rgb_order` / `fmt_opaque_alpha` /
    /// `fmt_bytes_swapped`）代替 `PixelFormat::write_color` 的运行时枚举匹配，
    /// 消除热路径中的分支开销。
    #[inline(always)]
    pub(crate) fn write_color_at_offset(&mut self, offset: usize, color: &Color) {
        if color.a == 0 {
            return;
        }

        let alpha = color.a as u16;
        let bpp = self.fmt_bpp;
        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        // 必须在 `buffer_mut()` 之前读入局部变量，否则与缓冲区的可变借用冲突。
        let bytes_swapped = self.fmt_bytes_swapped;
        let target = self.buffer_mut();
        debug_assert!(
            offset + bpp <= target.len(),
            "write_color_at_offset: offset {offset} + bpp {bpp} exceeds buffer length {}",
            target.len()
        );
        // SAFETY: 调用方保证 offset + bpp <= buffer.len()
        let dst = unsafe { target.get_unchecked_mut(offset..offset + bpp) };

        if bpp == 2 {
            // 565：解码 → 混合（`blend_565` 内含 a == 255 的短路）→ 量化回写。
            let word = decode_565_bytes(dst, bytes_swapped);
            let bytes = encode_565_bytes(blend_565(word, color), bytes_swapped);
            dst[0] = bytes[0];
            dst[1] = bytes[1];
            return;
        }

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
        // 短路：格式不匹配、宽高为 0 时直接返回
        if src_format != self.info.format || width == 0 || height == 0 {
            return;
        }
        // 每行实际读取 width * bpp 字节，故 src_pitch 不得小于该值，
        // 否则 src_start + copy_bytes 会越过行尾（见下方切片索引）。
        if src_pitch < width.saturating_mul(self.info.format.bytes_per_pixel()) {
            return;
        }
        // 短路：src 长度不足
        if src.len() < src_pitch.saturating_mul(height) {
            return;
        }
        let bpp = self.info.format.bytes_per_pixel();
        let fb_width = self.info.width;
        let dst_x = dst_top_left.x;

        // 逐行处理
        for row in 0..height {
            let dy = dst_top_left.y + row;
            if dy >= self.info.height {
                break;
            }
            // 行级别裁剪
            if dst_x >= fb_width {
                continue;
            }
            let visible_cols = width.min(fb_width - dst_x);
            let copy_bytes = visible_cols * bpp;
            let src_start = row * src_pitch;
            let dst_offset = self.pixel_offset(dst_x, dy);
            let dst_ptr = &mut self.buffer_mut()[dst_offset..dst_offset + copy_bytes];
            let src_ptr = &src[src_start..src_start + copy_bytes];
            #[cfg(feature = "simd")]
            // SAFETY: dst_ptr 和 src_ptr 均至少为 copy_bytes 长度
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
        let bytes_swapped = self.fmt_bytes_swapped;
        let pitch = self.info.pitch;

        let src_r = color.r;
        let src_g = color.g;
        let src_b = color.b;
        let src_alpha = color.a as u16;
        // 短路：颜色完全透明
        if src_alpha == 0 {
            return;
        }

        // 一次性获取缓冲区裸指针，避免逐像素借用
        let buffer = self.buffer_mut();
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
            let row_ptr = unsafe { base_ptr.add(base_off) };

            if bpp == 4 && row_ptr.align_offset(4) == 0 {
                // 4bpp 且行首 4 字节对齐时走 SWAR 批量路径
                //
                // 未对齐的情况见 `fill_span_opaque` 的说明——`pitch` 允许不是
                // 4 的倍数，此时第 2 行起会错位，交给下面的逐像素路径。
                debug_assert!(
                    base_off + visible_cols * bpp <= buf_len,
                    "blit_mask: SWAR 行区间越界"
                );
                unsafe {
                    crate::color::swar::blend_masked_row_4bpp(
                        row_ptr,
                        mask.as_ptr().add(row_mask_off),
                        visible_cols,
                        &color,
                        is_rgb_order,
                        opaque_alpha,
                    )
                };
                continue;
            }
            // 逐像素路径
            for col in 0..visible_cols {
                let m_alpha = mask[row_mask_off + col];
                if m_alpha == 0 {
                    continue;
                }
                let offset = base_off + col * bpp;
                debug_assert!(offset + bpp <= buf_len, "blit_mask: offset overflow");
                // SAFETY: offset + bpp <= buf_len 由裁剪保证
                let dst = unsafe { base_ptr.add(offset) };
                if bpp == 2 {
                    // 565：掩码字节直接作为该像素的绘制 alpha（无 SWAR 批量路径）。
                    // SAFETY: offset + 2 <= buf_len 由上面的 debug_assert 保证。
                    let word = unsafe { load_565(dst, bytes_swapped) };
                    let blended = blend_565(word, &color.mix_alpha(m_alpha));
                    unsafe { store_565(dst, blended, bytes_swapped) };
                } else if m_alpha == 255 {
                    write_rgb_ptr!(dst, src_r, src_g, src_b, is_rgb_order);
                    if bpp == 4 {
                        unsafe { *dst.add(3) = opaque_alpha };
                    }
                } else {
                    // mask byte 直接作为该像素的绘制 alpha
                    let m_alpha_u16 = m_alpha as u16;
                    let m_inv_alpha = 255 - m_alpha_u16;
                    write_rgb_blend_ptr!(
                        dst,
                        src_r,
                        src_g,
                        src_b,
                        m_alpha_u16,
                        m_inv_alpha,
                        is_rgb_order
                    );
                    if bpp == 4 {
                        unsafe { *dst.add(3) = opaque_alpha };
                    }
                }
            }
        }
    }

    /// 用不透明颜色快速填充一行中的水平跨度。
    ///
    /// 对 4bpp 格式，把像素写成 `[u8; 4]` 后按 4 字节块批量填充；
    /// 对 2bpp 的 565 格式按 2 字节块填充；
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
        let start = self.pixel_offset(x_start, y);
        let row_len = (x_end - x_start + 1) * pixel_size;
        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        let bytes_swapped = self.fmt_bytes_swapped;
        let target = self.buffer_mut();
        // SAFETY: 调用方 fill_visible_horizontal_span 保证
        // x_start..=x_end 已裁剪到 [0, width)，y 已裁剪到 [0, height)，
        // 故 start + row_len = pixel_offset(x_end, y) + pixel_size <= buffer.len()。
        let row = unsafe { target.get_unchecked_mut(start..start + row_len) };

        if pixel_size == 4 {
            // 构造 4 字节 pattern，然后按 4 字节块批量填充。
            let mut pattern = [0u8; 4];
            write_rgb!(pattern, color.r, color.g, color.b, is_rgb_order);
            pattern[3] = opaque_alpha;
            row.chunks_exact_mut(4).for_each(|pixel| {
                pixel.copy_from_slice(&pattern);
            });
        } else if pixel_size == 2 {
            // 565：先量化打包成 2 字节 pattern，再按 2 字节块批量填充。
            let pattern = encode_565_bytes(pack_565(color), bytes_swapped);
            row.chunks_exact_mut(2).for_each(|pixel| {
                pixel.copy_from_slice(&pattern);
            });
        } else {
            // 3bpp: 单像素模式 + 倍增复制
            write_rgb!(row, color.r, color.g, color.b, is_rgb_order);
            pattern_fill(row, &[row[0], row[1], row[2]]);
        }
    }

    /// 用半透明颜色快速填充一行中的水平跨度。
    ///
    /// 4bpp 格式优先走 SWAR 批量路径（实现在 [`crate::color::swar`]，因为它与
    /// 像素格式强相关且与逐通道混合共用同一套语义）；3bpp 格式因跨字节不对齐，
    /// 2bpp 的 565 格式因通道不足 8 位，以及 4bpp 下行首未按 4 字节对齐时，
    /// 保留逐像素路径。
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
        let bytes_swapped = self.fmt_bytes_swapped;

        let buffer = self.buffer_mut();
        debug_assert!(start + span_len <= buffer.len());
        let base_ptr = buffer.as_mut_ptr();
        let row = unsafe { base_ptr.add(start) };

        if bpp == 4 && row.align_offset(4) == 0 {
            // SWAR 内核按 u32 读写，要求行首 4 字节对齐。`pitch` 允许不是 4 的
            // 倍数（见 `fill_span_opaque` 的说明），此时第 2 行起会错位，
            // 交给下面的逐像素路径处理。
            unsafe {
                crate::color::swar::blend_span_alpha_4bpp(
                    row,
                    pixel_count,
                    color,
                    self.fmt_rgb_order,
                    self.fmt_opaque_alpha,
                )
            };
            return;
        }

        let is_rgb_order = self.fmt_rgb_order;
        let opaque_alpha = self.fmt_opaque_alpha;
        let alpha = color.a as u16;
        let inv_alpha = 255 - alpha;
        let (src_r, src_g, src_b) = (color.r, color.g, color.b);

        for pixel_idx in 0..pixel_count {
            let dst = unsafe { row.add(pixel_idx * bpp) };
            if bpp == 2 {
                // 565：解码 → blend_channel → 量化（无 SWAR 批量路径）。
                // SAFETY: pixel_idx < pixel_count，故 dst..dst+2 在跨度内。
                let word = unsafe { load_565(dst, bytes_swapped) };
                let blended = blend_565(word, color);
                unsafe { store_565(dst, blended, bytes_swapped) };
                continue;
            }
            write_rgb_blend_ptr!(dst, src_r, src_g, src_b, alpha, inv_alpha, is_rgb_order);
            if bpp == 4 {
                // 4bpp 的第 4 字节恒为该格式的 opaque_alpha，不参与混合
                unsafe { *dst.add(3) = opaque_alpha };
            }
        }
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
        let target = self.buffer();
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

        let pitch = self.info.pitch;
        let total_bytes = pitch * height;

        // 16 字节模式描述符。对 4bpp 可铺满整个缓冲，把每像素一次写入降为
        // 每 16 字节一次写入；对 3bpp 退回单像素模式。
        let (pattern, pattern_len) = self.build_pattern(&color);

        // 缓冲区可能小于一个完整模式（例如 1×1 的 4bpp 只有 4 字节），
        // 此时只能写入实际存在的字节数，否则会越界。
        let init_len = pattern_len.min(total_bytes);

        let buffer = self.buffer_mut();
        let base_ptr = buffer.as_mut_ptr();

        // SAFETY: init_len <= min(pattern_len, total_bytes)，既不越过 16 字节的
        // pattern，也不越过缓冲区（构造期保证 buffer.len() >= pitch * height）；
        // pattern 是栈上数组，与缓冲区不重叠。
        unsafe {
            #[cfg(feature = "simd")]
            simd::memcpy(base_ptr, pattern.as_ptr(), init_len);
            #[cfg(not(feature = "simd"))]
            core::ptr::copy_nonoverlapping(pattern.as_ptr(), base_ptr, init_len);
        }

        if pitch == row_len {
            // 无行间填充：一次铺满整个 buffer
            // SAFETY: total_bytes = pitch * height <= buffer.len()，切片不越过分配。
            let full_buf = unsafe { core::slice::from_raw_parts_mut(base_ptr, total_bytes) };
            pattern_fill(full_buf, &pattern[..init_len]);
        } else {
            // 有行间填充：先铺满第一行，再逐行复制
            // SAFETY: pitch >= row_len（FramebufferInfo 构造期校验）且 height >= 1
            // （守卫已排除 0），故 row_len <= total_bytes <= buffer.len()。
            let first_row = unsafe { core::slice::from_raw_parts_mut(base_ptr, row_len) };
            pattern_fill(first_row, &pattern[..init_len.min(row_len)]);

            for y in 1..height {
                // SAFETY: y < height，故 y * pitch + row_len <= pitch * height <=
                // buffer.len()；目标行与首行（拷贝源）不重叠。
                unsafe {
                    let row_ptr = base_ptr.add(y * pitch);
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

        // 单像素的规范字节布局：565 走量化打包，其余格式通过 write_rgb! 统一字节序。
        let mut pixel = [0u8; 4];
        if bpp == 2 {
            pixel[..2].copy_from_slice(&encode_565_bytes(pack_565(color), self.fmt_bytes_swapped));
        } else {
            write_rgb!(pixel, color.r, color.g, color.b, is_rgb_order);
            if bpp == 4 {
                pixel[3] = if stores_alpha {
                    color_alpha
                } else {
                    opaque_alpha
                };
            }
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

    /// 使用 Bresenham 算法绘制线段，自动裁剪到帧缓冲边界。
    ///
    /// 先用 Cohen–Sutherland 裁剪（`clip_line`）对整条线段做快速拒绝，再交给
    /// `draw_line_internal`。该内部函数会把迭代范围收窄到可见区间，
    /// 因此工作量与**线段总长度无关**，只与可见部分的像素数成正比。
    ///
    /// 收窄不会改变像素覆盖：Bresenham 的递推是确定的，内部函数保留原始端点
    /// 与完整的相位，仅跳过必然越界的迭代。
    pub fn draw_line(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
        if self.info.width == 0 || self.info.height == 0 {
            return;
        }
        // `clip_line` 只用于快速拒绝，**不可**把它的返回值当作新端点：
        // `intersect_axis` 用整数除法截断求交点，得到的端点并不在原直线上，
        // 斜率一旦改变，Bresenham 的像素覆盖就会与原线段不同。
        // 可见区间的裁剪由 `draw_line_internal` 内部的区间收窄完成。
        if clip_line(p1, p2, self.info.width, self.info.height).is_none() {
            return;
        }
        unsafe {
            self.draw_line_unchecked(p1, p2, color);
        }
    }

    /// Bresenham 线段绘制（唯一实现，也是 `draw_line` 与
    /// `draw_line_unchecked` 共享的受限内核）。
    ///
    /// 预取缓冲区裸指针，内循环直接指针写入，避免逐像素的函数调用与格式匹配开销。
    ///
    /// 迭代区间收窄到 `[x_lo, x_hi]`（主轴可见范围），次轴在循环内单独校验上界。
    /// 因此工作量与**线段总长度无关**，只与可见部分的像素数成正比——即便端点是
    /// `usize::MAX` 量级也不会退化成全长步进。
    ///
    /// 收窄不改变像素覆盖：Bresenham 的递推是确定的，本函数保留原始端点与完整
    /// 相位，仅跳过必然越界的迭代。该等价性由
    /// `draw_line_iteration_narrowing_preserves_coverage` 守护。
    #[inline(always)]
    unsafe fn draw_line_unchecked(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
        // 用饱和转换（而非 `as i64`）把 `usize` 坐标转到 `i64`：`as` 会把
        // 大于 `i64::MAX` 的值回绕成负数（如 `usize::MAX as i64 == -1`），
        // 使端点被错误地折到画布另一侧，覆盖结果完全错误。
        let (mut x1, mut y1) = (sat_i64(p1.x), sat_i64(p1.y));
        let (mut x2, mut y2) = (sat_i64(p2.x), sat_i64(p2.y));
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
        let bytes_swapped = self.fmt_bytes_swapped;
        let alpha = color.a as u16;
        let inv_alpha = 255 - alpha;
        let buffer = self.buffer_mut();
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
                } else if bpp == 2 {
                    // 565：解码 → blend_channel → 量化（`blend_565` 内含
                    // a == 255 的短路，故这里不必再分支）。
                    // SAFETY: 主轴已收窄到可见范围、次轴已校验上界，
                    // 故 offset..offset+2 落在缓冲区可写区域内。
                    let word = unsafe { load_565(dst, bytes_swapped) };
                    let blended = blend_565(word, color);
                    unsafe { store_565(dst, blended, bytes_swapped) };
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

    /// 绘制三角形轮廓（由三条线段组成）。
    #[inline]
    pub fn draw_triangle(&mut self, p1: Pixel, p2: Pixel, p3: Pixel, color: &Color) {
        if p1 == p2 || p2 == p3 || p3 == p1 {
            return;
        }
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
        // 理论最优排序
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }
        if pts[1].y > pts[2].y {
            pts.swap(1, 2);
        }
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }

        // 转换为 i64, 防止溢出
        let (x1, y1) = (sat_i64(pts[0].x), sat_i64(pts[0].y));
        let (x2, y2) = (sat_i64(pts[1].x), sat_i64(pts[1].y));
        let (x3, y3) = (sat_i64(pts[2].x), sat_i64(pts[2].y));

        // 如果三个点的 Y 坐标相同，不绘制
        if y1 == y3 {
            return;
        }

        // 计算delta
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

    /// 圆与圆弧共用的参数归一：饱和转换圆心，并把 `radius` 钳到 `width + height`。
    ///
    /// 钳制的依据：任意可见像素到圆心的距离不超过画布对角线，而
    /// `sqrt(width² + height²) < width + height`，故半径超过该上界时可见像素的
    /// 着色结果与恰好等于该上界时完全一致——屏幕外的行由各图元的边界检查丢弃。
    /// 钳制把迭代量从 `O(radius)` 压到 `O(height)`，避免调用方传入 `usize::MAX`
    /// 量级的半径时把渲染线程钉死。
    ///
    /// `MAX_RADIUS` 只用于保证后续 `r²` 在 `u128` 内可表示（`(i64::MAX / 2)²`
    /// 远小于 `u128::MAX`），真实帧缓冲到不了这个量级。
    ///
    /// 半径小于上界时钳制无效果，故不影响既有覆盖。两个方向各由一条测试守护：
    /// `circle_clamps_huge_radius` 抓钳制缺失，`circle_clamp_preserves_radii_within_limit`
    /// 抓钳制过度。
    #[inline(always)]
    fn disk_params(&self, center: Pixel, radius: usize) -> (i64, i64, i64) {
        const MAX_RADIUS: usize = (i64::MAX / 2) as usize;

        let limit = self
            .info
            .width
            .saturating_add(self.info.height)
            .min(MAX_RADIUS);
        // 用 `sat_i64` 而非 `as i64`：后者会把超出 `i64::MAX` 的值回绕成负数，
        // 使半径变成负数而一行都不画（`usize::MAX` 即如此）。
        (
            sat_i64(center.x),
            sat_i64(center.y),
            sat_i64(radius.min(limit)),
        )
    }

    /// 精确圆盘第 `k = |dy|` 行的半宽 `floor(sqrt(r² − k²))`，用**增量收缩**求取。
    ///
    /// `half` 是跨行复用的状态：半宽随 `|dy|` 单调不增，故只需从上一行的半宽
    /// 继续向下收缩，全程总收缩次数 `<= r`，均摊 `O(1)`。这样既不需要浮点
    /// （`no_std` 且关闭 `programmable` 时没有 `f32`），也不需要逐位整数开方。
    ///
    /// 前置条件：`k` 必须**单调递增**地调用，且 `k <= r`（由调用方保证，
    /// 故 `r2 - k2` 不会下溢）。返回值恒 `>= 0`。
    #[inline(always)]
    fn disk_half_at(r2: u128, k: i64, half: &mut i64) -> i64 {
        let k2 = (k as u128) * (k as u128);
        while (*half as u128) * (*half as u128) > r2 - k2 {
            *half -= 1;
        }
        *half
    }

    /// 精确圆盘的逐扫描行遍历：对 `k = 0, 1, …` 回调 `(cx, cy, k, half)`。
    ///
    /// 半宽只取决于 `|dy|`，故 `cy - k` 与 `cy + k` 两行的半宽都是 `half`，
    /// 由调用方自行展开（`k == 0` 时两行重合，需只写一次）。
    ///
    /// **每行恰好回调一次**，填充路径因此天然不会对同一像素重复写入——这是
    /// 「半透明色只混合一次」的结构性保证，不依赖任何去重后处理。
    ///
    /// 迭代量 `k <= min(r, height)`：`|dy| >= height` 的行必在帧缓冲外，
    /// 故与半径无关（半径已由 [`Self::disk_params`] 钳制）。
    #[inline(always)]
    fn walk_disk_rows(
        &mut self,
        center: Pixel,
        radius: usize,
        mut visit: impl FnMut(&mut Self, i64, i64, i64, i64),
    ) {
        let (cx, cy, r) = self.disk_params(center, radius);
        let max_k = r.min(sat_i64(self.info.height));
        let r2 = (r as u128) * (r as u128);
        let mut half = r;

        for k in 0..=max_k {
            let row_half = Self::disk_half_at(r2, k, &mut half);
            visit(self, cx, cy, k, row_half);
        }
    }

    /// 精确圆盘边界的逐像素遍历：回调 `(px, py, dx, dy)`，每个边界像素恰好一次。
    ///
    /// `(px, py)` 是已做饱和转换的绝对像素坐标，`(dx, dy)` 是相对圆心的偏移
    /// （圆弧的楔形判定用后者，因为楔形定义在圆心坐标系里）。
    ///
    /// 边界定义为**落在圆盘 `dx² + dy² <= r²` 内、且存在 4 邻域落在盘外**的像素
    /// （数字圆盘的内边界）：第 `k` 行是 `|dx| ∈ (H(k+1), H(k)]`，外加半宽像素
    /// `H(k)` 本身——当 `H(k+1) == H(k)` 时该区间为空，但 `(H(k) + 1, k)` 已在盘外，
    /// 故 `(H(k), k)` 仍属边界。该定义给出单像素宽、8-连通的轮廓。
    ///
    /// 半径钳制与迭代上界同 [`Self::walk_disk_rows`]。
    #[inline(always)]
    fn walk_disk_boundary(
        &mut self,
        center: Pixel,
        radius: usize,
        mut visit: impl FnMut(&mut Self, i64, i64, i64, i64),
    ) {
        let (cx, cy, r) = self.disk_params(center, radius);
        let max_k = r.min(sat_i64(self.info.height));
        let r2 = (r as u128) * (r as u128);
        let mut half = r;

        // 第 `k` 行的边界需要 `H(k+1)`，故发射滞后一行。
        let mut pending: Option<(i64, i64)> = None;
        for k in 0..=max_k {
            let row_half = Self::disk_half_at(r2, k, &mut half);
            if let Some((prev_k, prev_half)) = pending {
                self.emit_boundary_row(cx, cy, prev_k, prev_half, row_half, &mut visit);
            }
            pending = Some((k, row_half));
        }

        // 收尾：仅当最后一行是圆盘末行（`H(k) == 0`，即 `k == r`）时才需补发，
        // 此时 `H(k+1)` 不存在，边界退化为单像素 `(cx, cy ± k)`。
        // 若 `k == height < r`，该行必在帧缓冲外，无需补发。
        if let Some((k, 0)) = pending {
            self.emit_boundary_row(cx, cy, k, 0, -1, &mut visit);
        }
    }

    /// 发射精确圆盘边界在 `cy ± k` 两行上的像素。
    ///
    /// `half` 是 `H(k)`，`next_half` 是 `H(k+1)`（末行取 `-1`，即视作盘外）。
    /// `next_half <= half` 恒成立，故区间 `(next_half, half]` 与半宽像素本身
    /// 的并集总是非空。
    #[inline(always)]
    fn emit_boundary_row(
        &mut self,
        cx: i64,
        cy: i64,
        k: i64,
        half: i64,
        next_half: i64,
        visit: &mut impl FnMut(&mut Self, i64, i64, i64, i64),
    ) {
        let (lo, hi) = if next_half < half {
            (next_half + 1, half)
        } else {
            // 区间为空：该行与相邻行半宽相同，只有半宽像素本身在边界上。
            (half, half)
        };

        for dx in lo..=hi {
            // 横向与纵向都用饱和运算：圆心可能被 `sat_i64` 顶到 `i64::MAX`，
            // 裸加会在 debug 构建下溢出 panic（越界像素本就会被丢弃）。
            let (xp, xm) = (cx.saturating_add(dx), cx.saturating_sub(dx));
            let (yp, ym) = (cy.saturating_add(k), cy.saturating_sub(k));

            // 行 `cy + k`（`k == 0` 时与 `cy - k` 同行，故只发一次）。
            visit(self, xp, yp, dx, k);
            if dx != 0 {
                visit(self, xm, yp, -dx, k);
            }
            if k != 0 {
                visit(self, xp, ym, dx, -k);
                if dx != 0 {
                    visit(self, xm, ym, -dx, -k);
                }
            }
        }
    }

    /// 使用精确圆盘边界绘制圆形轮廓。
    ///
    /// 轮廓像素定义为「落在圆盘 `dx² + dy² <= radius²` 内、且存在 4 邻域落在盘外」
    /// 的像素，即数字圆盘的内边界：单像素宽、8-连通，且**每个像素只写一次**，
    /// 故半透明色不会被重复混合。
    ///
    /// 与 [`Self::fill_circle`] 共用同一套逐行半宽，故轮廓恰好是填充的最外圈——
    /// `draw_circle` 画出的像素必然也出现在同参数的 `fill_circle` 结果里。
    ///
    /// 自动忽略超出帧缓冲边界的像素。`radius` 被钳制到 `width + height`，
    /// 迭代量因此与半径无关（详见 `disk_params`）。
    pub fn draw_circle(&mut self, center: Pixel, radius: usize, color: &Color) {
        if radius == 0 {
            return;
        }

        self.walk_disk_boundary(center, radius, |renderer, px, py, _dx, _dy| {
            if px >= 0 && py >= 0 {
                renderer.set_pixel(px as usize, py as usize, color);
            }
        });
    }

    /// 使用精确圆盘填充圆形区域：`dx² + dy² <= radius²` 的像素全部着色。
    ///
    /// 每个扫描行只调用一次跨度填充，故**每个像素只写一次**——半透明色不会被
    /// 重复混合，无需任何去重后处理。自动裁剪到帧缓冲边界，`radius` 被钳制到
    /// `width + height`，迭代量因此与半径无关（详见 `disk_params`）。
    ///
    /// # 与中点圆递推的差异
    ///
    /// 本函数直接按圆方程取整求每行半宽，而不是复用中点圆递推的八对称跨度。
    /// 后者在极点附近会把 `cy ± radius` 那一行画成一条宽约 `sqrt(2 · radius)`
    /// 的横线（半径 200 时宽 29 px），面积比真圆盘大 0.5%（半径 200）到 10%
    /// （半径 10）；前者就是精确圆盘。
    pub fn fill_circle(&mut self, center: Pixel, radius: usize, color: Color) {
        if radius == 0 || self.info.width == 0 || self.info.height == 0 {
            return;
        }
        // 完全透明无需写入（与 fill_arc 一致，提前返回避免无谓迭代）。
        if color.a == 0 {
            return;
        }

        self.walk_disk_rows(center, radius, |renderer, cx, cy, k, half| {
            // `k == 0` 时 `cy + 0 == cy - 0` 是同一行，只写一次。
            renderer.fill_visible_horizontal_span(cx - half, cx + half, cy + k, &color);
            if k != 0 {
                renderer.fill_visible_horizontal_span(cx - half, cx + half, cy - k, &color);
            }
        });
    }

    /// 绘制圆弧轮廓：从 `start_angle` 沿角度增大方向扫掠到 `end_angle`。
    ///
    /// 与 [`Self::draw_circle`] 共用同一条精确圆盘边界，只在回调内用
    /// [`Wedge`] 筛掉区间外的像素，因此半径钳制与迭代量上界与
    /// [`Self::draw_circle`] 完全一致，且每个像素只写一次。
    ///
    /// # 角度约定
    ///
    /// 角度沿屏幕坐标增大（Y 轴向下）：`0°` 指向右、`90°` 指向下。
    /// 区间含两端；`start_angle == end_angle` 表示**整圈**。
    /// 圆弧跨过 `0°` 时直接用更小的终止角即可，例如 `350°→10°`。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::{Angle, Pixel, Renderer};
    /// # use rasteron::color::Color;
    /// # use rasteron::framebuffer::{FramebufferInfo, PixelFormat};
    /// let info = FramebufferInfo::new(16, 16, 16 * 4, PixelFormat::Rgba8888);
    /// let mut fb = vec![0u8; info.buffer_size()];
    /// let mut renderer = Renderer::new(info, &mut fb);
    /// // 屏幕右下象限的四分之一圆弧
    /// renderer.draw_arc(
    ///     Pixel::new(8, 8),
    ///     6,
    ///     Angle::ZERO,
    ///     Angle::RIGHT_DOWN,
    ///     &Color::new(255, 0, 0),
    /// );
    /// ```
    pub fn draw_arc(
        &mut self,
        center: Pixel,
        radius: usize,
        start_angle: Angle,
        end_angle: Angle,
        color: &Color,
    ) {
        if radius == 0 {
            return;
        }
        // 整圈时无需逐点判定楔形，直接走整圆轮廓路径。
        let wedge = Wedge::new(start_angle, end_angle);
        if wedge.is_full() {
            self.draw_circle(center, radius, color);
            return;
        }

        self.walk_disk_boundary(center, radius, |renderer, px, py, dx, dy| {
            // 判定用相对圆心的偏移，负坐标同样有效（纯代数，无边界依赖）。
            if wedge.contains(dx, dy) && px >= 0 && py >= 0 {
                renderer.set_pixel(px as usize, py as usize, color);
            }
        });
    }

    /// 填充扇形：从 `center` 出发，介于 `start_angle` 与 `end_angle` 之间的
    /// 圆盘区域。
    ///
    /// 对每一扫描行先求出圆盘在该行的水平跨度，再逐像素做楔形判定。每行只
    /// 遍历一次，故**每个像素只写一次**，半透明色不会被重复混合。
    /// 迭代量取决于 `radius`（已被钳制为 `O(height)`）与跨度宽度，不取决于角度。
    ///
    /// # 角度约定
    ///
    /// 与 [`Self::draw_arc`] 相同：含两端，`start == end` 表示整圈。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::{Angle, Pixel, Renderer};
    /// # use rasteron::color::Color;
    /// # use rasteron::framebuffer::{FramebufferInfo, PixelFormat};
    /// let info = FramebufferInfo::new(16, 16, 16 * 4, PixelFormat::Rgba8888);
    /// let mut fb = vec![0u8; info.buffer_size()];
    /// let mut renderer = Renderer::new(info, &mut fb);
    /// // 屏幕右下方向的 90° 扇形
    /// renderer.fill_arc(
    ///     Pixel::new(8, 8),
    ///     6,
    ///     Angle::ZERO,
    ///     Angle::RIGHT_DOWN,
    ///     Color::new(0, 255, 0),
    /// );
    /// ```
    pub fn fill_arc(
        &mut self,
        center: Pixel,
        radius: usize,
        start_angle: Angle,
        end_angle: Angle,
        color: Color,
    ) {
        if radius == 0 || self.info.width == 0 || self.info.height == 0 {
            return;
        }
        // 完全透明无需写入（与 fill_circle 一致，提前返回避免无谓迭代）。
        if color.a == 0 {
            return;
        }

        let wedge = Wedge::new(start_angle, end_angle);
        // 整圈时退化为 fill_circle，直接走跨度快路径。
        if wedge.is_full() {
            self.fill_circle(center, radius, color);
            return;
        }

        let width = sat_i64(self.info.width);
        let height = sat_i64(self.info.height);

        // 逐行取精确圆盘的跨度，再逐像素做楔形判定：每行只遍历一次，
        // 故同一像素不会被重复混合。
        self.walk_disk_rows(center, radius, |renderer, cx, cy, k, half| {
            // `k == 0` 时 `cy + k` 与 `cy - k` 是同一行，只扫一次。
            let rows = [cy - k, cy + k];
            let row_count = if k == 0 { 1 } else { 2 };

            for &py in rows.iter().take(row_count) {
                if py < 0 || py >= height {
                    continue;
                }
                // 该行的水平范围裁到帧缓冲，内层迭代量因此受 width 约束。
                let lo = (cx - half).max(0);
                let hi = (cx + half).min(width - 1);
                let row_off = py - cy;
                let mut px = lo;
                while px <= hi {
                    if wedge.contains(px - cx, row_off) {
                        renderer.set_pixel(px as usize, py as usize, &color);
                    }
                    px += 1;
                }
            }
        });
    }

    /// 将后台缓冲区呈现（present）到前台。
    ///
    /// 只拷贝每行可见的 `width × bytes_per_pixel` 字节：行间填充不属于可见像素，
    /// 可见区域之外的字节保持前台原样。这一点对
    /// [`sub_view`](crate::utils::sub_view) 构造的子渲染器是必需的——子渲染器的
    /// 切片按父缓冲区的 `pitch` 跨行，整段拷贝会连面板以外的像素一起覆盖。
    ///
    /// 本方法是**单向拷贝**，不是指针交换：调用后后台缓冲区内容保持不变，
    /// 下一帧仍以当前后台内容为起点。需要清屏的调用方自行调用
    /// [`Renderer::clear`]。
    ///
    /// 单缓冲模式下（未提供后台缓冲区）绘制本就落在前台，此方法为空操作。
    pub fn swap_buffers(&mut self) {
        // 先取出后台切片的不可变引用：`back_buffer` 与 `front_buffer` 是不同的
        // 字段，借用检查器允许随后可变借用 `front_buffer`。
        let Some(back) = self.back_buffer.as_deref() else {
            return;
        };

        let row_len = self.info.width * self.info.bytes_per_pixel();
        let height = self.info.height;
        if row_len == 0 || height == 0 {
            return;
        }

        let pitch = self.info.pitch;
        if pitch == row_len {
            // 无行间填充：一次拷贝。切片可能比 `buffer_size()` 更长，故显式取前
            // `pitch × height` 字节，而不是整体 `copy_from_slice`（长度不等会 panic）。
            let total = pitch * height;
            self.front_buffer[..total].copy_from_slice(&back[..total]);
            return;
        }

        // 有行间填充：逐行只拷贝可见部分，填充字节与相邻区域不被触碰。
        for y in 0..height {
            let off = y * pitch;
            self.front_buffer[off..off + row_len].copy_from_slice(&back[off..off + row_len]);
        }
    }

    /// 返回帧缓冲布局信息的引用。
    #[inline]
    pub fn info(&self) -> &FramebufferInfo {
        &self.info
    }
}
