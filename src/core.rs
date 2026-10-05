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

/// 圆图元的种类，供 `tests` 中的参考实现对照使用。
///
/// 显式枚举而非 `bool` 参数，避免调用点出现无法自解释的 `true` / `false`。
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CircleKind {
    /// 只描边（`draw_circle`）。
    Outline,
    /// 实心填充（`fill_circle`）。
    Filled,
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
    ///
    /// 缓冲区由调用方提供，库本身不做任何分配——这正是双缓冲在 `no_std`
    /// 无分配器环境下可用的原因。
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
        let (
            fmt_bpp,
            fmt_rgb_order,
            fmt_opaque_alpha,
            fmt_stores_color_alpha,
            fmt_bytes_swapped,
        ) = match info.format {
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
            let dst_offset = self.pixel_offset(dst_x, dy);
            let dst_ptr = &mut self.buffer_mut()[dst_offset..dst_offset + copy_bytes];
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
        let bytes_swapped = self.fmt_bytes_swapped;
        let pitch = self.info.pitch;

        let src_r = color.r;
        let src_g = color.g;
        let src_b = color.b;
        let src_alpha = color.a as u16;
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
                // 4bpp 且行首 4 字节对齐时走 SWAR 批量路径（实现在 color::swar）：
                // 每个像素一次 32 位读-改-写，替代 3 次逐通道混合。
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
            //
            // 该路径独立成函数：其代码体明显大于逐像素路径，若整体内联进
            // fill_rect，会挤占不透明路径的代码布局。
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

        let buffer = self.buffer_mut();
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
            pattern_fill(full_buf, &pattern[..init_len]);
        } else {
            // 有行间填充：先铺满第一行，再逐行复制
            let first_row = unsafe { core::slice::from_raw_parts_mut(base_ptr, row_len) };
            pattern_fill(first_row, &pattern[..init_len.min(row_len)]);

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
        self.draw_line_internal(p1, p2, color);
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
    fn draw_line_internal(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
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

    /// Bresenham 线段算法，跳过 Cohen–Sutherland 整段拒绝。
    ///
    /// 与 [`Self::draw_line`] 的可见性差异仅在于：跳过 `clip_line` 的整段提前
    /// 拒绝，也跳过 `width == 0 || height == 0` 的前置守卫。二者都是纯粹的
    /// 快速路径——`clip_line` 的拒绝在迭代区间收窄到可见范围后本就是冗余的，
    /// 而退化尺寸下 `x_lo > x_hi` 或 `y <= y_hi_bound` 恒为假，同样不会写入。
    /// 因此像素覆盖与 [`Self::draw_line`] 逐像素一致，由
    /// `draw_line_unchecked_matches_safe_path` 守护。
    ///
    /// 保留 `unsafe` 是为了不破坏既有 API，调用方无需提供任何坐标保证。
    ///
    /// # Safety
    ///
    /// 本函数当前无附加前置条件：`draw_line_internal` 已把迭代区间收窄到可见范围
    /// 并逐轴校验坐标，不会越界写。调用方只需保证渲染器自身持有的帧缓冲切片
    /// 仍然有效（即未经释放或被移出所有权），这与 [`Self::draw_line`] 相同。
    #[inline]
    pub unsafe fn draw_line_unchecked(&mut self, p1: Pixel, p2: Pixel, color: &Color) {
        self.draw_line_internal(p1, p2, color);
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
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }
        if pts[1].y > pts[2].y {
            pts.swap(1, 2);
        }
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }

        let (x1, y1) = (sat_i64(pts[0].x), sat_i64(pts[0].y));
        let (x2, y2) = (sat_i64(pts[1].x), sat_i64(pts[1].y));
        let (x3, y3) = (sat_i64(pts[2].x), sat_i64(pts[2].y));

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

#[cfg(test)]
mod tests {
    use super::{CircleKind, Renderer};
    use crate::angle::Angle;
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
        let color = Color::new(0x12, 0x34, 0x56);
        // 覆盖屏内、恰好越界、次轴整体越界与 usize::MAX 量级端点。
        //
        // `draw_line_unchecked` 曾同时存在两个缺陷：对 `x1..=x2` 全长步进
        // （端点为 usize::MAX 时是 DoS），以及不校验次轴就直接写
        // （`y` 超出屏幕时越界写，实测 SIGSEGV）。两条断言同时守住这两个回归。
        let cases: [(Pixel, Pixel); 8] = [
            (Pixel::new(0, 1), Pixel::new(3, 2)),
            (Pixel::new(0, 0), Pixel::new(usize::MAX, usize::MAX)),
            (Pixel::new(0, 0), Pixel::new(usize::MAX, 0)),
            (Pixel::new(0, 0), Pixel::new(0, usize::MAX)),
            (Pixel::new(usize::MAX, 0), Pixel::new(0, 0)),
            (Pixel::new(2, 0), Pixel::new(usize::MAX, 1)),
            (Pixel::new(0, 100), Pixel::new(3, 100)),
            (Pixel::new(100, 0), Pixel::new(100, usize::MAX)),
        ];

        for (start, end) in cases {
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

            safe.draw_line(start, end, &color);
            unsafe { unchecked.draw_line_unchecked(start, end, &color) };

            assert_eq!(
                safe.buffer(),
                unchecked.buffer(),
                "coverage mismatch for {start:?} -> {end:?}"
            );
        }
    }

    /// 圆测试的固定场景：4×4 帧缓冲、圆心 `(1, 2)`。
    const CIRCLE_W: usize = 4;
    const CIRCLE_H: usize = 4;
    const CIRCLE_CX: usize = 1;
    const CIRCLE_CY: usize = 2;
    /// `radius` 的钳制上界 `width + height`。此半径的填充恰好覆盖整屏，
    /// 故填充侧的等价性断言非平凡。
    const CIRCLE_LIMIT: usize = CIRCLE_W + CIRCLE_H;
    /// `Bgr24` 每像素字节数。
    const BGR24_BYTES: usize = 3;
    /// 固定场景下各半径的填充像素数实测值，作为"钳制不得改变
    /// `radius <= CIRCLE_LIMIT` 行为"的锚点。
    ///
    /// 数值就是欧氏圆盘 `dx² + dy² <= radius²` 的像素数（圆心 `(1, 2)` 偏离
    /// 正中，故不等于 `πr²`）。`radius >= 3` 时该场景已覆盖整屏。
    const CIRCLE_FILL_PIXELS: [(usize, usize); 3] = [(1, 5), (2, 11), (3, 16)];

    /// 统计缓冲中被实际着色（存在非零通道）的像素数。
    fn painted_pixels(fb: &[u8]) -> usize {
        fb.chunks_exact(BGR24_BYTES)
            .filter(|pixel| pixel.iter().any(|byte| *byte != 0))
            .count()
    }

    /// 在全新缓冲上按固定场景渲染一个圆，返回缓冲副本。
    ///
    /// 每次都重新清零并重建 `Renderer`，避免复用残留让相等断言平凡通过。
    fn render_test_circle(radius: usize, kind: CircleKind) -> Vec<u8> {
        let color = Color::new(0x12, 0x34, 0x56);
        let center = Pixel::new(CIRCLE_CX, CIRCLE_CY);
        let mut front = vec![0u8; CIRCLE_W * CIRCLE_H * BGR24_BYTES];
        let mut renderer = Renderer::new(
            FramebufferInfo::new(
                CIRCLE_W,
                CIRCLE_H,
                CIRCLE_W * BGR24_BYTES,
                PixelFormat::Bgr24,
            ),
            &mut front,
        );

        match kind {
            CircleKind::Outline => renderer.draw_circle(center, radius, &color),
            CircleKind::Filled => renderer.fill_circle(center, radius, color),
        }

        Vec::from(renderer.buffer())
    }

    #[test]
    fn circle_clamps_huge_radius() {
        // 半径超过 `width + height` 后，可见像素的着色结果不再变化。
        // 本测试同时守住两件事，缺任何一条都失去意义：
        //   1. 等价性——超大半径与上界半径输出逐字节相同（钳制生效）
        //   2. 不挂死——未钳制时 `1_000_000_000` 会让 `while x <= y` 迭代约 7 亿次
        let filled_at_limit = render_test_circle(CIRCLE_LIMIT, CircleKind::Filled);

        // 基准必须非空，否则上面的相等断言两边都是全 0、无论钳制是否存在都通过。
        assert!(
            painted_pixels(&filled_at_limit) > 0,
            "上界半径 {CIRCLE_LIMIT} 的填充未覆盖任何像素，等价性断言会平凡通过"
        );

        // 顺序有意义：`1_000_000_000` 排在最前，它是唯一会触发 DoS 的输入
        // （无钳制时本测试会挂死）；`usize::MAX` 另需覆盖 `as i64` 把半径回绕成
        // 负数、导致一个点都不画的情形。其余项覆盖刚过界与远超界两侧。
        let radii = [
            1_000_000_000,
            usize::MAX,
            10 * CIRCLE_LIMIT,
            CIRCLE_LIMIT + 1,
            64,
        ];

        for huge in radii {
            for kind in [CircleKind::Outline, CircleKind::Filled] {
                let clamped = render_test_circle(CIRCLE_LIMIT, kind);
                let big = render_test_circle(huge, kind);

                assert_eq!(big, clamped, "{kind:?} differs for r={huge}");
            }
        }
    }

    #[test]
    fn circle_clamp_preserves_radii_within_limit() {
        // 守住"钳制不过度"：`radius < width + height` 时钳制必须是恒等映射，
        // 否则大半径会画出比应得面积更小的图形。
        //
        // 只与另一个半径互比守不住这一点（循环论证：钳到 1 时任意两半径都相等），
        // 故锚定各半径的实测填充像素数。固定场景下 `radius >= 3` 即覆盖整屏，
        // 因此这三个锚点已覆盖全部可观察差异——钳到 1 或 2 都会改变它们。
        for (radius, expected_pixels) in CIRCLE_FILL_PIXELS {
            let filled = render_test_circle(radius, CircleKind::Filled);

            assert_eq!(
                painted_pixels(&filled),
                expected_pixels,
                "半径 {radius} 的填充像素数被钳制改变"
            );
        }

        // 上界半径必须覆盖整屏。钳制收得比 `width + height` 更紧会漏掉可见像素，
        // 直接比对期望字节以同时校验覆盖范围与通道顺序。
        let expected_fill = [0x56u8, 0x34, 0x12].repeat(CIRCLE_W * CIRCLE_H);

        assert_eq!(
            render_test_circle(CIRCLE_LIMIT, CircleKind::Filled),
            expected_fill,
            "上界半径未覆盖整屏，钳制上界过紧"
        );
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

    /// 半透明 `draw_circle` 必须对每个像素**只混合一次**。
    ///
    /// 旧的八对称展开在 `x == 0`（4 个基点像素）与 `x == y`（4 个对角像素）处
    /// 会对同一像素写两次，半透明色因此加深（alpha=128 得 191 而非 128）。
    /// 半径 11 与 20 的递推里 `x == y` 成立、18 不成立，三者合起来覆盖两类重合。
    #[test]
    fn draw_circle_alpha_blends_once() {
        let (w, h) = (61usize, 61usize);
        let center = Pixel::new(30, 30);
        let alpha = 128u8;
        let src = Color::with_alpha(255, 255, 255, alpha);
        let inv = 255 - alpha as u16;
        let expect = PixelFormat::blend_channel(src.r, 0, alpha as u16, inv) as u8;

        for radius in [11usize, 18, 20] {
            let mut front = vec![0u8; w * h * BGR24_BYTES];
            let mut renderer = Renderer::new(
                FramebufferInfo::new(w, h, w * BGR24_BYTES, PixelFormat::Bgr24),
                &mut front,
            );
            renderer.clear(Color::new(0, 0, 0));
            renderer.draw_circle(center, radius, &src);

            let mut painted = 0usize;
            for y in 0..h {
                for x in 0..w {
                    let got = renderer.get_pixel(x, y).expect("像素应可读").r;
                    if got == 0 {
                        continue;
                    }
                    painted += 1;
                    assert_eq!(
                        got, expect,
                        "半径 {radius} 的轮廓像素 ({x},{y}) 被混合了多次：期望 {expect}，实际 {got}"
                    );
                }
            }
            assert!(painted > 8, "半径 {radius} 的轮廓像素过少，断言会失去意义");

            // 4 个基点像素在旧实现里必然被写两次，显式断言它们被画到，
            // 以免测试在"轮廓恰好没覆盖到重合位置"时空转。
            for (x, y) in [
                (center.x + radius, center.y),
                (center.x - radius, center.y),
                (center.x, center.y + radius),
                (center.x, center.y - radius),
            ] {
                assert_eq!(
                    renderer.get_pixel(x, y).expect("基点像素应可读").r,
                    expect,
                    "半径 {radius} 的基点像素 ({x},{y}) 未按单次混合写入"
                );
            }
        }
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

    /// 半透明 `fill_circle` 必须对每个像素**只混合一次**。
    ///
    /// 守护的是四条对称跨度在 `x == 0`（首轮）与 `x == y`（末轮）落到
    /// 同一行造成的重复写入。不透明色重复写是幂等的（看不出来），半透明
    /// 却是累积混合——未修复时圆心行会被叠到 191/223 而非单次的 128。
    ///
    /// 断言方式：纯黑底 + 半透明白覆盖整个圆，圆内**每个**像素都应等于
    /// 单次混合的结果。任何一次重复写都会让该像素变亮而失败。
    #[test]
    fn fill_circle_alpha_blends_once() {
        let (w, h) = (41usize, 41usize);
        let center = Pixel::new(20, 20);
        let radius = 18usize;
        let alpha = 128u8;
        let src = Color::with_alpha(255, 255, 255, alpha);

        let mut front = vec![0u8; w * h * BGR24_BYTES];
        let mut renderer = Renderer::new(
            FramebufferInfo::new(w, h, w * BGR24_BYTES, PixelFormat::Bgr24),
            &mut front,
        );
        renderer.clear(Color::new(0, 0, 0));
        renderer.fill_circle(center, radius, src);

        // 单次混合的期望值（对黑底混合一次）
        let inv = 255 - alpha as u16;
        let expect = PixelFormat::blend_channel(src.r, 0, alpha as u16, inv);

        let mut checked = 0usize;
        for y in 0..h {
            for x in 0..w {
                // 只检查圆盘内部
                let dx = x as i64 - center.x as i64;
                let dy = y as i64 - center.y as i64;
                if dx * dx + dy * dy > (radius as i64) * (radius as i64) {
                    continue;
                }
                let got = renderer.get_pixel(x, y).expect("圆内像素应可读").r;
                assert_eq!(
                    got, expect as u8,
                    "像素 ({x},{y}) 的半透明圆被混合了多次：期望 {expect}，实际 {got}"
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "检查的像素过少，断言会失去意义");
    }

    /// 精确圆盘参考实现：逐像素判定 `dx² + dy² <= radius²`。
    ///
    /// 与被测实现完全独立的朴素写法，用作填充语义的基准。
    fn reference_disk_pixels(
        w: usize,
        h: usize,
        center: Pixel,
        radius: usize,
    ) -> Vec<(usize, usize)> {
        let (cx, cy) = (center.x as i64, center.y as i64);
        let r2 = (radius as i64) * (radius as i64);

        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as i64 - cx, y as i64 - cy);
                if dx * dx + dy * dy <= r2 {
                    out.push((x, y));
                }
            }
        }
        out
    }

    /// 精确圆盘边界参考实现：盘中且存在 4 邻域落在盘外的像素。
    ///
    /// 邻域判定用**纯圆方程**，不把帧缓冲外当作盘外——被测实现先算几何边界再
    /// 裁剪，故屏幕边缘的像素不会因为"邻居在屏幕外"而被算成边界像素。
    fn reference_disk_boundary(
        w: usize,
        h: usize,
        center: Pixel,
        radius: usize,
    ) -> Vec<(usize, usize)> {
        let (cx, cy) = (center.x as i64, center.y as i64);
        let r2 = (radius as i64) * (radius as i64);
        let inside = |x: i64, y: i64| {
            let (dx, dy) = (x - cx, y - cy);
            dx * dx + dy * dy <= r2
        };

        let mut out = Vec::new();
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let is_boundary = inside(x, y)
                    && (!inside(x - 1, y)
                        || !inside(x + 1, y)
                        || !inside(x, y - 1)
                        || !inside(x, y + 1));
                if is_boundary {
                    out.push((x as usize, y as usize));
                }
            }
        }
        out
    }

    /// 圆测试的多组场景 `(width, height, center_x, center_y)`：覆盖圆心居中、
    /// 贴边、贴角与偶数尺寸，避免只在一个对称场景上通过。
    const CIRCLE_SCENES: [(usize, usize, usize, usize); 4] =
        [(9, 9, 4, 4), (11, 7, 2, 5), (8, 8, 0, 0), (13, 13, 6, 6)];

    /// 在给定场景下渲染一次圆，返回被着色像素的集合（行优先序）。
    fn render_circle_pixels(
        scene: (usize, usize, usize, usize),
        radius: usize,
        kind: CircleKind,
    ) -> Vec<(usize, usize)> {
        let (w, h, cx, cy) = scene;
        let color = Color::new(0x12, 0x34, 0x56);
        let mut front = vec![0u8; w * h * BGR24_BYTES];
        let mut renderer = Renderer::new(
            FramebufferInfo::new(w, h, w * BGR24_BYTES, PixelFormat::Bgr24),
            &mut front,
        );

        match kind {
            CircleKind::Outline => renderer.draw_circle(Pixel::new(cx, cy), radius, &color),
            CircleKind::Filled => renderer.fill_circle(Pixel::new(cx, cy), radius, color),
        }

        painted_set(renderer.buffer(), w, h)
    }

    /// `fill_circle` 必须逐像素等于精确圆盘 `dx² + dy² <= radius²`。
    ///
    /// 这是填充语义的基准：实现不再复用中点圆递推的八对称跨度（后者在极点
    /// 附近会把 `cy ± radius` 画成一条宽横线，面积偏大）。
    #[test]
    fn fill_circle_matches_exact_disk_reference() {
        let mut checked = 0usize;
        for scene in CIRCLE_SCENES {
            for radius in 1..=10usize {
                let got = render_circle_pixels(scene, radius, CircleKind::Filled);
                let expected =
                    reference_disk_pixels(scene.0, scene.1, Pixel::new(scene.2, scene.3), radius);

                checked += expected.len();
                assert_eq!(
                    got, expected,
                    "场景 {scene:?} 半径 {radius} 的填充与精确圆盘不一致"
                );
            }
        }
        assert!(checked > 500, "参考圆盘覆盖的像素总数过少，断言会失去意义");
    }

    /// `draw_circle` 必须逐像素等于精确圆盘边界（盘中且有 4 邻域在盘外）。
    ///
    /// 该定义同时保证轮廓单像素宽、8-连通，且不会画到圆盘之外。
    /// 半径大于场景尺寸时边界整个落在屏幕外，参考集与实测集**都**为空，
    /// 故非平凡性由累计像素数把关。
    #[test]
    fn draw_circle_matches_exact_disk_boundary() {
        let mut checked = 0usize;
        for scene in CIRCLE_SCENES {
            for radius in 1..=10usize {
                let got = render_circle_pixels(scene, radius, CircleKind::Outline);
                let expected =
                    reference_disk_boundary(scene.0, scene.1, Pixel::new(scene.2, scene.3), radius);

                checked += expected.len();
                assert_eq!(
                    got, expected,
                    "场景 {scene:?} 半径 {radius} 的轮廓与精确圆盘边界不一致"
                );
            }
        }
        assert!(checked > 200, "参考边界覆盖的像素总数过少，断言会失去意义");
    }

    // ── draw_arc / fill_arc ──

    /// 圆弧与扇形测试用的统一场景：一个远离原点的圆心，避免四边裁剪干扰。
    const ARC_CX: usize = 10;
    const ARC_CY: usize = 10;
    const ARC_R: usize = 6;

    /// 在 ARC 场景的缓冲上执行一次绘制，返回缓冲副本。
    ///
    /// `op` 拿到的是 `&mut Renderer`，因为单缓冲下的绘制直接写 front。
    fn render_arc<F: FnOnce(&mut Renderer)>(op: F) -> Vec<u8> {
        let (w, h) = (21, 21);
        let mut front = vec![0u8; w * h * BGR24_BYTES];
        let mut renderer = Renderer::new(
            FramebufferInfo::new(w, h, w * BGR24_BYTES, PixelFormat::Bgr24),
            &mut front,
        );
        op(&mut renderer);
        Vec::from(renderer.buffer())
    }

    /// 统计缓冲中被实际着色（非零通道）的像素坐标集合。
    fn painted_set(fb: &[u8], w: usize, h: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * BGR24_BYTES;
                if fb[i..i + BGR24_BYTES].iter().any(|b| *b != 0) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    /// 90° 扇形（0°→90°，屏幕右下象限）只填右下象限，其余三象限保持空白。
    #[test]
    fn fill_arc_quarter_covers_only_target_screen_quadrant() {
        let fb = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                Color::new(0x12, 0x34, 0x56),
            );
        });
        let painted = painted_set(&fb, 21, 21);
        assert!(!painted.is_empty(), "扇形未覆盖任何像素");

        let (cx, cy) = (ARC_CX as i64, ARC_CY as i64);
        for (x, y) in &painted {
            let dx = *x as i64 - cx;
            let dy = *y as i64 - cy;
            // 右下闭象限：dx >= 0 且 dy >= 0
            assert!(
                dx >= 0 && dy >= 0,
                "像素 ({x},{y}) 落在楔形外，相对圆心偏移 ({dx},{dy})"
            );
        }
        // 圆心本身必须被填充
        assert!(painted.contains(&(ARC_CX, ARC_CY)), "扇形未包含圆心");
    }

    /// 半圆扇形（0°→180°）恰好填满下半屏：圆心所在行及其下方有像素，
    /// 圆心上方必须一个都没有。
    #[test]
    fn fill_arc_semicircle_fills_exactly_lower_half() {
        let fb = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::DOWN_LEFT,
                Color::new(0x12, 0x34, 0x56),
            );
        });
        let painted = painted_set(&fb, 21, 21);
        assert!(!painted.is_empty());

        for (x, y) in &painted {
            assert!(
                *y >= ARC_CY,
                "像素 ({x},{y}) 在半圆扇形上方（圆心行 = {ARC_CY}）"
            );
        }
        // 圆心行上左右两侧都应被覆盖（0° 与 180° 两条边界射线）
        assert!(
            painted.contains(&(ARC_CX + ARC_R, ARC_CY)),
            "未覆盖 0° 边界"
        );
        assert!(
            painted.contains(&(ARC_CX - ARC_R, ARC_CY)),
            "未覆盖 180° 边界"
        );
        // 圆心正下方（90°）在扇形内
        assert!(
            painted.contains(&(ARC_CX, ARC_CY + ARC_R)),
            "未覆盖 90° 方向"
        );
    }

    /// 大跨度扇形（0°→270°）必须填满除右上象限以外的全部区域。
    ///
    /// 这条守护"大跨度取补集"的错误实现：补集写法会漏掉起点射线附近
    /// 的像素，以及缺口边界上的整条射线。
    #[test]
    fn fill_arc_major_sweep_excludes_only_the_gap() {
        let fb = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::LEFT_UP,
                Color::new(0x12, 0x34, 0x56),
            );
        });
        let painted = painted_set(&fb, 21, 21);
        assert!(!painted.is_empty());

        let (cx, cy) = (ARC_CX as i64, ARC_CY as i64);
        for (x, y) in &painted {
            let dx = *x as i64 - cx;
            let dy = *y as i64 - cy;
            // 缺口是 270°..360°，即右上（dx > 0 且 dy < 0）
            let in_gap = dx > 0 && dy < 0;
            assert!(!in_gap, "像素 ({x},{y}) 落在缺口象限内");
        }
        // 三个保留象限都应有像素（此处用 usize 常量直接比较坐标）
        assert!(
            painted.iter().any(|(x, y)| *x >= ARC_CX && *y >= ARC_CY),
            "右下象限应有像素"
        );
        assert!(
            painted.iter().any(|(x, y)| *x <= ARC_CX && *y >= ARC_CY),
            "左下象限应有像素"
        );
        assert!(
            painted.iter().any(|(x, y)| *x <= ARC_CX && *y <= ARC_CY),
            "左上象限应有像素"
        );
    }

    /// `start == end` 的圆弧与扇形必须与整圆逐字节相同。
    #[test]
    fn arc_full_sweep_matches_draw_and_fill_circle() {
        let a = Angle::from_degrees(45);
        let outlined = render_arc(|r| {
            r.draw_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                a,
                a,
                &Color::new(0x12, 0x34, 0x56),
            )
        });
        let full_outline = render_arc(|r| {
            r.draw_circle(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                &Color::new(0x12, 0x34, 0x56),
            )
        });
        assert_eq!(
            outlined, full_outline,
            "整圈 draw_arc 与 draw_circle 不一致"
        );

        let filled = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                a,
                a,
                Color::new(0x12, 0x34, 0x56),
            )
        });
        let full_fill = render_arc(|r| {
            r.fill_circle(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Color::new(0x12, 0x34, 0x56),
            )
        });
        assert_eq!(filled, full_fill, "整圈 fill_arc 与 fill_circle 不一致");
    }

    /// 圆弧描边只画圆周上的点，不填充扇形内部。
    #[test]
    fn draw_arc_draws_only_outline_not_interior() {
        let fb = render_arc(|r| {
            r.draw_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                &Color::new(0x12, 0x34, 0x56),
            )
        });
        let painted = painted_set(&fb, 21, 21);
        assert!(!painted.is_empty(), "圆弧未画出任何像素");
        // 圆心不应被描边（描边只覆盖圆周）
        assert!(!painted.contains(&(ARC_CX, ARC_CY)), "圆弧不应填充圆心");
        // 描边像素数必须远少于扇形：圆周是 O(r)，扇形是 O(r²)。
        let sector = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                Color::new(0x12, 0x34, 0x56),
            )
        });
        let sector_count = painted_set(&sector, 21, 21).len();
        assert!(
            painted.len() * 3 < sector_count,
            "圆弧像素数 {} 相对扇形 {} 过大，可能误填了内部",
            painted.len(),
            sector_count
        );
    }

    /// 角度按屏幕坐标增大：0°→90° 是右下象限而非数学上的第一象限。
    ///
    /// 这条守护 Y 轴向下这一全库约定不被圆弧实现误用。
    #[test]
    fn arc_uses_screen_coordinate_convention() {
        // 0° 指向右：圆心右侧的像素应在扇形内
        let fb = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                Color::new(0x12, 0x34, 0x56),
            )
        });
        let painted = painted_set(&fb, 21, 21);
        // 90° 指向下：正下方在扇形内，正上方不在
        assert!(
            painted.contains(&(ARC_CX, ARC_CY + ARC_R)),
            "90° 应指向屏幕下方"
        );
        assert!(
            !painted.contains(&(ARC_CX, ARC_CY - ARC_R)),
            "270° 方向不应出现在 0..90 扇形内"
        );
        // 0° 指向右：正右在扇形内，正左不在
        assert!(
            painted.contains(&(ARC_CX + ARC_R, ARC_CY)),
            "0° 应指向屏幕右方"
        );
        assert!(
            !painted.contains(&(ARC_CX - ARC_R, ARC_CY)),
            "180° 方向不应在扇形内"
        );
    }

    /// 跨越 0° 的扇形（350°→10°）应正确覆盖右上方的一条窄楔形。
    #[test]
    fn fill_arc_wraps_across_zero_degrees() {
        let fb = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::from_degrees(350),
                Angle::from_degrees(10),
                Color::new(0x12, 0x34, 0x56),
            )
        });
        let painted = painted_set(&fb, 21, 21);
        assert!(!painted.is_empty(), "跨界扇形未覆盖任何像素");

        let (cx, cy) = (ARC_CX as i64, ARC_CY as i64);
        for (x, y) in &painted {
            let dx = *x as i64 - cx;
            let dy = *y as i64 - cy;
            // 圆心本身（零向量）按约定属于扇形内部，跳过方向判定。
            if dx == 0 && dy == 0 {
                continue;
            }
            // 楔形是右方偏上/偏下的一条窄带：|dy| 小、dx > 0
            assert!(
                dx > 0 && dy.abs() * 2 <= dx.abs() + ARC_R as i64,
                "像素 ({x},{y}) 超出 350°..10° 窄楔形"
            );
        }
        // 正右方必在楔形内
        assert!(
            painted.contains(&(ARC_CX + ARC_R, ARC_CY)),
            "0° 方向应落在跨界扇形内"
        );
    }

    /// 半径为 0 时两个图元都静默不画。
    #[test]
    fn arc_and_sector_reject_zero_radius() {
        let outlined = render_arc(|r| {
            r.draw_arc(
                Pixel::new(ARC_CX, ARC_CY),
                0,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                &Color::new(0x12, 0x34, 0x56),
            )
        });
        assert!(
            painted_set(&outlined, 21, 21).is_empty(),
            "半径 0 的圆弧画了像素"
        );

        let filled = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                0,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                Color::new(0x12, 0x34, 0x56),
            )
        });
        assert!(
            painted_set(&filled, 21, 21).is_empty(),
            "半径 0 的扇形画了像素"
        );
    }

    /// 完全透明的扇形不得写入任何像素。
    #[test]
    fn fill_arc_transparent_color_does_nothing() {
        let fb = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                Color::with_alpha(0x12, 0x34, 0x56, 0),
            )
        });
        assert!(
            painted_set(&fb, 21, 21).is_empty(),
            "完全透明的扇形写入了像素"
        );
    }

    /// 半透明扇形必须走混合路径：结果介于背景与原色之间，而非直接覆盖。
    #[test]
    fn fill_arc_alpha_blends_into_background() {
        let fb = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                Color::with_alpha(255, 255, 255, 128),
            )
        });
        // BGR24 覆盖黑色背景，alpha=128 的白色应得到约一半亮度
        let i = (ARC_CY * 21 + ARC_CX) * BGR24_BYTES;
        assert_eq!(
            &fb[i..i + BGR24_BYTES],
            &[128, 128, 128],
            "半透明扇形未按混合写入"
        );
    }

    /// 半透明 `fill_arc` 必须对**每个**像素只混合一次。
    ///
    /// 旧的逐轮归并只处理同一轮内的重合，跨轮重复（同一个 `y` 横跨多轮、半宽
    /// 递增）会让边缘行被混合 2~5 次。上面的
    /// `fill_arc_alpha_blends_into_background` 只检查圆心像素，恰好落在单轮归并
    /// 覆盖的 `x == 0` 情形上，抓不到这一类。
    #[test]
    fn fill_arc_alpha_blends_once() {
        let (w, h) = (41usize, 41usize);
        let center = Pixel::new(20, 20);
        let radius = 18usize;
        let alpha = 128u8;
        let src = Color::with_alpha(255, 255, 255, alpha);
        let inv = 255 - alpha as u16;
        let expect = PixelFormat::blend_channel(src.r, 0, alpha as u16, inv) as u8;

        // 半径 18 的递推里 `y = 18` 横跨 5 轮、`y = 16` 横跨 2 轮，
        // 正是跨轮重复的两个典型位置。
        for (start, end) in [(0u32, 180u32), (0, 90), (350, 10)] {
            let mut front = vec![0u8; w * h * BGR24_BYTES];
            let mut renderer = Renderer::new(
                FramebufferInfo::new(w, h, w * BGR24_BYTES, PixelFormat::Bgr24),
                &mut front,
            );
            renderer.clear(Color::new(0, 0, 0));
            renderer.fill_arc(
                center,
                radius,
                Angle::from_degrees(start),
                Angle::from_degrees(end),
                src,
            );

            let mut painted = 0usize;
            for y in 0..h {
                for x in 0..w {
                    let got = renderer.get_pixel(x, y).expect("像素应可读").r;
                    if got == 0 {
                        continue;
                    }
                    painted += 1;
                    assert_eq!(
                        got, expect,
                        "扇形 {start}°..{end}° 的像素 ({x},{y}) 被混合了多次：期望 {expect}，实际 {got}"
                    );
                }
            }
            assert!(
                painted > 10,
                "扇形 {start}°..{end}° 覆盖像素过少，断言会失去意义"
            );
        }
    }

    /// 圆弧轮廓的像素必须是同参数扇形填充的子集。
    ///
    /// 两者共用同一条逐行半宽，这条断言守护"轮廓恰好落在填充的最外圈"，
    /// 同时守住轮廓不会画到圆盘之外。
    #[test]
    fn draw_arc_pixels_are_subset_of_fill_arc() {
        for (start, end) in [(0u32, 90u32), (0, 180), (0, 270), (350, 10), (45, 45)] {
            let color = Color::new(0x12, 0x34, 0x56);
            let outlined = render_arc(|r| {
                r.draw_arc(
                    Pixel::new(ARC_CX, ARC_CY),
                    ARC_R,
                    Angle::from_degrees(start),
                    Angle::from_degrees(end),
                    &color,
                )
            });
            let filled = render_arc(|r| {
                r.fill_arc(
                    Pixel::new(ARC_CX, ARC_CY),
                    ARC_R,
                    Angle::from_degrees(start),
                    Angle::from_degrees(end),
                    color,
                )
            });

            let outline_pixels = painted_set(&outlined, 21, 21);
            let fill_pixels = painted_set(&filled, 21, 21);
            assert!(
                !outline_pixels.is_empty(),
                "圆弧 {start}°..{end}° 未画出任何像素"
            );
            for pixel in &outline_pixels {
                assert!(
                    fill_pixels.contains(pixel),
                    "圆弧 {start}°..{end}° 的像素 {pixel:?} 不在同参数扇形填充内"
                );
            }
        }
    }

    /// 超大半径必须被钳制，迭代量与半径无关，且结果与上界半径一致。
    ///
    /// 与 `circle_clamps_huge_radius` 同源，守护圆弧复用同一条钳制路径。
    /// 基准半径取 `width + height`（即 `disk_params` 的钳制上界），
    /// 取更小的值不会触发钳制，超大半径自然与它不同。
    #[test]
    fn arc_clamps_huge_radius() {
        // render_arc 用的是 21x21 缓冲，钳制上界为 21 + 21。
        const LIMIT: usize = 42;
        let at_limit = render_arc(|r| {
            r.fill_arc(
                Pixel::new(ARC_CX, ARC_CY),
                LIMIT,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                Color::new(0x12, 0x34, 0x56),
            )
        });
        assert!(
            !painted_set(&at_limit, 21, 21).is_empty(),
            "上界半径的扇形未覆盖任何像素，等价性断言会平凡通过"
        );

        // `usize::MAX` 若未经钳制，`disk_params` 会算出天文数字的半径，
        // 逐行半宽的收缩循环与 `O(radius)` 的迭代会把线程钉死。
        for huge in [1_000_000_000usize, usize::MAX] {
            let big = render_arc(|r| {
                r.fill_arc(
                    Pixel::new(ARC_CX, ARC_CY),
                    huge,
                    Angle::ZERO,
                    Angle::RIGHT_DOWN,
                    Color::new(0x12, 0x34, 0x56),
                )
            });
            assert_eq!(big, at_limit, "半径 {huge} 与上界半径的扇形输出不一致");
        }
    }

    /// 圆心在屏幕外时不得崩溃，且可见部分仍被正确绘制。
    #[test]
    fn arc_with_center_outside_framebuffer_is_clipped() {
        let fb = render_arc(|r| {
            r.draw_arc(
                Pixel::new(0, 0),
                5,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                &Color::new(0x12, 0x34, 0x56),
            )
        });
        // 圆心在 (0,0)，圆弧向下向右延伸；不能 panic，且确有可见像素
        assert!(!painted_set(&fb, 21, 21).is_empty());
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

    /// 用给定 `pitch` 跑一遍 4bpp 掩码 blit，并逐字节对照 [`PixelFormat::write_color`]。
    ///
    /// `pitch % 4 != 0` 时第 2 行起行首不再对齐，用于覆盖 SWAR 路径的对齐回退；
    /// 掩码交替取 0 / 255 / 中间值，覆盖跳过、不透明直写与混合三条分支。
    fn check_blit_mask_4bpp(pitch: usize, height: usize) {
        const W: usize = 9;
        let color = Color::new(0x12, 0x34, 0x56);
        let mask_row: [u8; W] = [0, 255, 1, 128, 254, 0, 255, 77, 200];
        let mask: Vec<u8> = mask_row.iter().copied().cycle().take(W * height).collect();

        for (format, _opaque_alpha) in [
            (PixelFormat::Xrgb8888, 0u8),
            (PixelFormat::Rgba8888, 255),
            (PixelFormat::Bgra8888, 255),
            (PixelFormat::Argb8888, 255),
        ] {
            let info = FramebufferInfo::new(W, height, pitch, format);
            let mut front = vec![0u8; info.buffer_size()];
            let mut renderer = Renderer::new(info, &mut front);

            // 种入非平凡目标像素，并把第 4 字节置成非零以验证会被覆写
            for y in 0..height {
                for x in 0..W {
                    renderer.set_pixel(x, y, &Color::new(200, 100, 50));
                    renderer.buffer_mut()[y * pitch + x * 4 + 3] = 0x7F;
                }
            }
            let original = renderer.buffer().to_vec();

            renderer.blit_mask(Pixel::new(0, 0), W, height, &mask, W, color);

            for y in 0..height {
                for x in 0..W {
                    let off = y * pitch + x * 4;
                    let mut want = [0u8; 4];
                    want.copy_from_slice(&original[off..off + 4]);
                    // mask == 0 时整像素（含第 4 字节）保持不变
                    if mask[y * W + x] != 0 {
                        format.write_color(&mut want, &color.mix_alpha(mask[y * W + x]));
                    }
                    assert_eq!(
                        &renderer.buffer()[off..off + 4],
                        &want[..],
                        "pitch={pitch} {format:?} ({x}, {y})"
                    );
                }
            }
        }
    }

    /// 4bpp 掩码 blit（SWAR 批量路径）的逐字节正确性。
    #[test]
    fn blit_mask_4bpp_matches_reference() {
        check_blit_mask_4bpp(9 * 4, 3);
    }

    /// `pitch` 不是 4 的倍数时，第 2 行起行首未对齐，掩码 blit 必须回退到
    /// 逐通道路径并仍然写对每个像素。
    #[test]
    fn blit_mask_4bpp_handles_unaligned_pitch() {
        check_blit_mask_4bpp(9 * 4 + 1, 3);
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
            // 565 的填充模式同样是 16 字节，而 1×1 只有 2 字节可用
            (PixelFormat::Rgb565, 2, 2, 1, 1),
            (PixelFormat::Bgr565, 2, 2, 1, 1),
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

    // ── Alpha 填充路径等价性 ──

    /// `fill_rect` 的半透明填充必须与逐通道 `blend_channel` 逐字节一致。
    ///
    /// 覆盖全部 255 个有效 alpha 值 × 广泛的 src/dst 组合，包含 4bpp 下会被
    /// SWAR 4 像素分组与不足 4 像素的尾部两条分支（SWAR 内核自身的等价性在
    /// `color::swar` 的单测中另有穷举校验）。
    #[test]
    fn alpha_fill_matches_per_channel_reference() {
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
        // （不能创建两个 Renderer——同一个 front 切片无法被同时可变借用。）
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
    fn alpha_fill_row_is_uniform_for_uniform_target() {
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
            assert_eq!(
                renderer.get_pixel(x, 0),
                Some(first),
                "列 {x} 与列 0 不一致"
            );
        }
    }

    /// 4bpp 的 alpha 填充必须把第 4 字节强制成该格式的 `opaque_alpha`，
    /// 而不是让它参与插值。
    ///
    /// `Xrgb8888` 的 `opaque_alpha` 是 0，其余 4bpp 格式是 255。批量填充路径
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
            // 宽度 8 = 2 组批量分组，无尾部分支
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

    /// `pitch` 不是 4 的倍数时，第 2 行起的行首不再按 4 字节对齐，
    /// 半透明填充必须回退到逐通道路径并仍然写对每个像素。
    ///
    /// 4bpp 的批量内核按 `u32` 读写，未对齐时解引用属于未定义行为，
    /// 因此这里既校验像素值，也校验第 4 字节被强制成 `opaque_alpha`。
    #[test]
    fn alpha_fill_handles_unaligned_pitch() {
        const W: usize = 5;
        const H: usize = 3;
        const PITCH: usize = W * 4 + 1; // 故意错位：pitch % 4 == 1

        let info = FramebufferInfo::new(W, H, PITCH, PixelFormat::Rgba8888);
        let mut front = [0u8; PITCH * H];
        let mut renderer = Renderer::new(info, &mut front);

        let dst = Color::new(64, 128, 192);
        for y in 0..H {
            for x in 0..W {
                renderer.set_pixel(x, y, &dst);
            }
        }
        let src = Color::with_alpha(200, 30, 90, 77);
        renderer.fill_rect(Pixel::new(0, 0), W, H, src);

        let inv = 255 - src.a as u16;
        let a = src.a as u16;
        let want = Color::with_alpha(
            PixelFormat::blend_channel(src.r, dst.r, a, inv),
            PixelFormat::blend_channel(src.g, dst.g, a, inv),
            PixelFormat::blend_channel(src.b, dst.b, a, inv),
            255,
        );
        for y in 0..H {
            for x in 0..W {
                assert_eq!(
                    renderer.get_pixel(x, y),
                    Some(want),
                    "pitch={PITCH} 时第 ({x}, {y}) 像素错误"
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
        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(0x12, 0x34, 0x56)));
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
                let info = FramebufferInfo::new(
                    w as usize,
                    h as usize,
                    w as usize * 3,
                    PixelFormat::Bgr24,
                );
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

    // ── 双缓冲（调用方提供后台缓冲区）──

    /// 双缓冲下绘制必须落在后台缓冲区，只有 `swap_buffers` 才把可见字节呈现到前台。
    #[test]
    fn double_buffer_draws_to_back_and_present_copies_to_front() {
        let make_info = || FramebufferInfo::new(4, 2, 16, PixelFormat::Rgba8888);
        let mut front = [0u8; 32];
        let mut back = [0u8; 32];

        // 第一段：绘制落在后台，前台保持初值。
        {
            let mut renderer = Renderer::with_back_buffer(make_info(), &mut front, &mut back);
            assert!(renderer.is_double_buffered());
            renderer.set_pixel(1, 1, &Color::new(10, 20, 30));
            // 像素 (1,1) 的偏移是 1 × pitch(16) + 1 × bpp(4) = 20
            assert_eq!(renderer.buffer()[20..23], [10, 20, 30]);
        }
        assert_eq!(front, [0u8; 32], "未 present 时前台不应被触碰");

        // 第二段：重新构造（后台内容保留），present 后前台等于后台。
        {
            let mut renderer = Renderer::with_back_buffer(make_info(), &mut front, &mut back);
            renderer.swap_buffers();
        }
        assert_eq!(front, back);
        assert_eq!(front[20..23], [10, 20, 30]);
    }

    /// 单缓冲下 `swap_buffers` 是空操作——绘制本就落在前台。
    #[test]
    fn single_buffer_swap_buffers_is_noop() {
        let info = FramebufferInfo::new(2, 2, 8, PixelFormat::Rgba8888);
        let mut front = [0u8; 16];
        {
            let mut renderer = Renderer::new(info, &mut front);
            assert!(!renderer.is_double_buffered());
            renderer.set_pixel(1, 1, &Color::new(1, 2, 3));
            renderer.swap_buffers();
        }
        assert_eq!(front[12..15], [1, 2, 3]);
    }

    /// 后台缓冲区短于 `pitch × height` 时必须在构造期 panic。
    #[test]
    #[should_panic(expected = "back_buffer too small")]
    fn with_back_buffer_rejects_short_back_buffer() {
        let info = FramebufferInfo::new(4, 4, 16, PixelFormat::Rgba8888);
        let mut front = [0u8; 64];
        let mut back = [0u8; 63];
        let _ = Renderer::with_back_buffer(info, &mut front, &mut back);
    }

    /// 有行间填充时，present 只覆盖每行可见字节：行尾填充与相邻区域保持前台原值。
    ///
    /// 这正是 `sub_view` 子视口能安全 present 的前提（切片按父缓冲区 `pitch` 跨行，
    /// 整段拷贝会连面板以外的像素一起覆盖）。
    #[test]
    fn swap_buffers_preserves_row_padding() {
        // pitch = 12 > width × bpp = 8，每行末尾 4 字节是填充。
        let info = FramebufferInfo::new(2, 2, 12, PixelFormat::Rgba8888);
        let mut front = [0xAAu8; 24];
        let mut back = [0u8; 24];
        {
            let mut renderer = Renderer::with_back_buffer(info, &mut front, &mut back);
            renderer.set_pixel(0, 0, &Color::new(1, 2, 3));
            renderer.set_pixel(0, 1, &Color::new(4, 5, 6));
            renderer.swap_buffers();
        }
        assert_eq!(&front[0..3], &[1, 2, 3]);
        assert_eq!(&front[12..15], &[4, 5, 6]);
        // 行尾填充字节未被触碰
        assert_eq!(&front[8..12], &[0xAA; 4]);
        assert_eq!(&front[20..24], &[0xAA; 4]);
        // 后台的填充字节仍是 0，说明拷贝范围严格是可见区间
        assert_eq!(&back[8..12], &[0u8; 4]);
    }

    /// 同一批绘制命令在单缓冲与双缓冲下必须产生完全相同的像素（present 之后）。
    #[test]
    fn double_buffer_present_matches_single_buffer_result() {
        fn draw_sample(renderer: &mut Renderer<'_>) {
            renderer.clear(Color::new(9, 8, 7));
            renderer.fill_rect(Pixel::new(1, 0), 3, 2, Color::with_alpha(200, 100, 50, 128));
            renderer.draw_line(Pixel::new(0, 0), Pixel::new(4, 2), &Color::new(1, 1, 1));
            renderer.fill_circle(Pixel::new(2, 1), 2, Color::new(255, 0, 255));
        }

        let make_info = || FramebufferInfo::new(5, 3, 20, PixelFormat::Rgba8888);
        let mut single = [0u8; 60];
        let mut front = [0u8; 60];
        let mut back = [0u8; 60];

        {
            let mut renderer = Renderer::new(make_info(), &mut single);
            draw_sample(&mut renderer);
        }
        {
            let mut renderer = Renderer::with_back_buffer(make_info(), &mut front, &mut back);
            draw_sample(&mut renderer);
            renderer.swap_buffers();
        }

        assert_eq!(single, front);
    }

    // ── Rgb565 / Bgr565 ──

    /// 565 的两个变体（小端存储 / 字节交换存储），用于参数化测试。
    const FORMATS_565: [PixelFormat; 2] = [PixelFormat::Rgb565, PixelFormat::Bgr565];

    /// 565 的不透明跨度（2 字节块填充）与半透明跨度（逐像素解码-混合-量化）
    /// 必须与逐像素参考 [`PixelFormat::write_color`] 逐字节一致，行填充不受影响。
    ///
    /// `pitch` 取 `width × 2` 与 `width × 2 + 1` 两种，后者使第 2 行起行首不再是
    /// 2 字节对齐，用于确认 565 路径不假设 `u16` 对齐。
    fn check_rgb565_fill_rect(format: PixelFormat, pitch: usize) {
        const W: usize = 9;
        const H: usize = 3;
        let info = FramebufferInfo::new(W, H, pitch, format);
        let mut front = vec![0xA5u8; info.buffer_size()];
        let mut renderer = Renderer::new(info, &mut front);

        // 种入非平凡背景，避免「零缓冲恰好正确」
        for y in 0..H {
            for x in 0..W {
                renderer.set_pixel(x, y, &Color::new(200, 100, 50));
            }
        }
        let original = renderer.buffer().to_vec();

        let opaque = Color::new(0x12, 0x34, 0x56);
        let semi = Color::with_alpha(200, 30, 90, 77);
        renderer.fill_rect(Pixel::new(1, 0), 4, 2, opaque);
        renderer.fill_rect(Pixel::new(0, 1), W, 2, semi);

        for y in 0..H {
            for x in 0..W {
                let off = y * pitch + x * 2;
                let mut want = [0u8; 2];
                want.copy_from_slice(&original[off..off + 2]);
                // 参考按实际绘制顺序叠加：先不透明矩形，后半透明矩形
                if (1..5).contains(&x) && y < 2 {
                    format.write_color(&mut want, &opaque);
                }
                if y >= 1 {
                    format.write_color(&mut want, &semi);
                }
                assert_eq!(
                    &renderer.buffer()[off..off + 2],
                    &want[..],
                    "pitch={pitch} {format:?} ({x}, {y})"
                );
            }
        }

        // 行填充字节必须原样保留
        for y in 0..H {
            for byte in W * 2..pitch {
                assert_eq!(
                    renderer.buffer()[y * pitch + byte],
                    0xA5,
                    "pitch={pitch} {format:?} 第 {y} 行填充被改写"
                );
            }
        }
    }

    #[test]
    fn rgb565_fill_rect_matches_per_pixel_reference() {
        for format in FORMATS_565 {
            check_rgb565_fill_rect(format, 9 * 2);
            check_rgb565_fill_rect(format, 9 * 2 + 1);
        }
    }

    /// `clear` 在 565 上走 16 字节模式路径：每个可见像素必须等于参考落盘结果，
    /// 行填充不受影响，且 1×1 的 2 字节缓冲区不得越界。
    fn check_rgb565_clear(format: PixelFormat, pitch: usize, w: usize, h: usize) {
        let info = FramebufferInfo::new(w, h, pitch, format);
        let mut front = vec![0x5Au8; info.buffer_size()];
        let mut renderer = Renderer::new(info, &mut front);

        let color = Color::new(0x12, 0x34, 0x56);
        renderer.clear(color);

        let mut want = [0u8; 2];
        format.write_color(&mut want, &color);
        for y in 0..h {
            for x in 0..w {
                let off = y * pitch + x * 2;
                assert_eq!(
                    &renderer.buffer()[off..off + 2],
                    &want[..],
                    "pitch={pitch} {format:?} ({x}, {y})"
                );
            }
            for byte in w * 2..pitch {
                assert_eq!(
                    renderer.buffer()[y * pitch + byte],
                    0x5A,
                    "pitch={pitch} {format:?} 第 {y} 行填充被改写"
                );
            }
        }
    }

    #[test]
    fn rgb565_clear_matches_reference() {
        for format in FORMATS_565 {
            check_rgb565_clear(format, 9 * 2, 9, 3);
            check_rgb565_clear(format, 9 * 2 + 1, 9, 3);
            // 1×1：缓冲区只有 2 字节，模式写入长度必须收敛到实际容量
            check_rgb565_clear(format, 2, 1, 1);
        }
    }

    /// 565 的掩码 blit 必须与 [`PixelFormat::write_color`] 参考逐字节一致。
    ///
    /// 掩码交替取 0 / 255 / 中间值，覆盖跳过、不透明与混合三条分支；
    /// 565 没有 SWAR 批量路径，全部走逐像素解码-混合-量化。
    fn check_rgb565_blit_mask(format: PixelFormat, pitch: usize, height: usize) {
        const W: usize = 9;
        let color = Color::new(0x12, 0x34, 0x56);
        let mask_row: [u8; W] = [0, 255, 1, 128, 254, 0, 255, 77, 200];
        let mask: Vec<u8> = mask_row.iter().copied().cycle().take(W * height).collect();

        let info = FramebufferInfo::new(W, height, pitch, format);
        let mut front = vec![0u8; info.buffer_size()];
        let mut renderer = Renderer::new(info, &mut front);
        for y in 0..height {
            for x in 0..W {
                renderer.set_pixel(x, y, &Color::new(200, 100, 50));
            }
        }
        let original = renderer.buffer().to_vec();

        renderer.blit_mask(Pixel::new(0, 0), W, height, &mask, W, color);

        for y in 0..height {
            for x in 0..W {
                let off = y * pitch + x * 2;
                let mut want = [0u8; 2];
                want.copy_from_slice(&original[off..off + 2]);
                // mask == 0 时整像素保持不变
                if mask[y * W + x] != 0 {
                    format.write_color(&mut want, &color.mix_alpha(mask[y * W + x]));
                }
                assert_eq!(
                    &renderer.buffer()[off..off + 2],
                    &want[..],
                    "pitch={pitch} {format:?} ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn rgb565_blit_mask_matches_reference() {
        for format in FORMATS_565 {
            check_rgb565_blit_mask(format, 9 * 2, 3);
            check_rgb565_blit_mask(format, 9 * 2 + 1, 3);
        }
    }

    /// 565 的 `set_pixel`：不透明写入等于量化打包，半透明写入等于逐像素参考
    /// 混合，`a == 0` 不触碰任何字节，`get_pixel` 解出规范量化色。
    #[test]
    fn rgb565_set_and_get_pixel_roundtrip() {
        for format in FORMATS_565 {
            let info = FramebufferInfo::new(3, 1, 6, format);
            let mut front = [0u8; 6];
            let mut renderer = Renderer::new(info, &mut front);

            let background = Color::new(200, 100, 50);
            let semi = Color::with_alpha(10, 20, 30, 128);

            // 像素 0：不透明写入 → 与参考落盘字节一致
            renderer.set_pixel(0, 0, &background);
            let mut want_opaque = [0u8; 2];
            format.write_color(&mut want_opaque, &background);
            assert_eq!(&renderer.buffer()[0..2], &want_opaque[..], "{format:?}");

            // 像素 1：先铺背景再叠加半透明 → 与参考混合一致
            renderer.set_pixel(1, 0, &background);
            let mut want_blend = [0u8; 2];
            want_blend.copy_from_slice(&renderer.buffer()[2..4]);
            format.write_color(&mut want_blend, &semi);
            renderer.set_pixel(1, 0, &semi);
            assert_eq!(&renderer.buffer()[2..4], &want_blend[..], "{format:?}");

            // get_pixel 必须解出规范量化色
            assert_eq!(
                renderer.get_pixel(0, 0),
                Some(Color::from_bytes(format, &renderer.buffer()[0..2]))
            );

            // 完全透明：一个字节都不动
            let before = renderer.buffer().to_vec();
            renderer.set_pixel(2, 0, &semi.mix_alpha(0));
            assert_eq!(renderer.buffer(), &before[..], "{format:?} a=0 改动了字节");
        }
    }

    /// 同一线段在 565 与 Bgr24 上的「被改写像素集合」必须完全相同（几何与格式
    /// 无关），且每个被改写像素等于 565 参考混合值。
    ///
    /// 该断言还能抓住「bpp == 2 却写了 3 个字节」这类污染相邻像素的错误——
    /// 多写的字节会额外改变相邻像素，使集合不再相等。
    #[test]
    fn rgb565_draw_line_writes_only_expected_pixels() {
        const W: usize = 8;
        const H: usize = 6;
        let background = Color::new(0x40, 0x80, 0xC0);
        let line_color = Color::with_alpha(0xE0, 0x10, 0x70, 77);

        /// 用同一批命令跑一遍，返回被改写像素的坐标（行优先）。
        fn changed_pixels(
            format: PixelFormat,
            width: usize,
            height: usize,
            background: Color,
            line_color: Color,
        ) -> Vec<(usize, usize)> {
            let pitch = width * format.bytes_per_pixel();
            let info = FramebufferInfo::new(width, height, pitch, format);
            let mut front = vec![0u8; info.buffer_size()];
            let mut renderer = Renderer::new(info, &mut front);
            renderer.clear(background);
            let before = renderer.buffer().to_vec();
            renderer.draw_line(
                Pixel::new(0, 0),
                Pixel::new(width - 1, height - 1),
                &line_color,
            );
            let after = renderer.buffer().to_vec();

            let bpp = format.bytes_per_pixel();
            let mut changed = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    let off = y * pitch + x * bpp;
                    if before[off..off + bpp] != after[off..off + bpp] {
                        changed.push((x, y));
                    }
                }
            }
            changed
        }

        let expected = changed_pixels(PixelFormat::Bgr24, W, H, background, line_color);
        assert!(!expected.is_empty(), "测试线段应当覆盖若干像素");

        for format in FORMATS_565 {
            assert_eq!(
                changed_pixels(format, W, H, background, line_color),
                expected,
                "{format:?} 的像素覆盖与 Bgr24 不一致"
            );
        }

        // 背景是纯色，故所有被改写像素的期望值相同
        for format in FORMATS_565 {
            let pitch = W * 2;
            let info = FramebufferInfo::new(W, H, pitch, format);
            let mut front = vec![0u8; info.buffer_size()];
            let mut renderer = Renderer::new(info, &mut front);
            renderer.clear(background);

            let mut want = [0u8; 2];
            format.write_color(&mut want, &background);
            format.write_color(&mut want, &line_color);

            renderer.draw_line(Pixel::new(0, 0), Pixel::new(W - 1, H - 1), &line_color);
            for &(x, y) in &expected {
                let off = y * pitch + x * 2;
                assert_eq!(
                    &renderer.buffer()[off..off + 2],
                    &want[..],
                    "{format:?} ({x}, {y}) 的混合值错误"
                );
            }
        }
    }

    /// 565 的 `blit` 是同格式原始拷贝：逐字节相等。
    #[test]
    fn rgb565_blit_same_format_copies_bytes() {
        for format in FORMATS_565 {
            let info = FramebufferInfo::new(4, 2, 8, format);
            let mut front = [0u8; 16];
            let mut renderer = Renderer::new(info, &mut front);

            let src: Vec<u8> = (0..12u8).collect();
            renderer.blit(Pixel::new(1, 0), 3, 2, format, &src, 6);

            assert_eq!(&renderer.buffer()[2..8], &src[0..6], "{format:?} 第 0 行");
            assert_eq!(&renderer.buffer()[10..16], &src[6..12], "{format:?} 第 1 行");
        }
    }
}
