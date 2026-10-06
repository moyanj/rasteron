use crate::angle::{Angle, Wedge};
use crate::color::{Color, Layout565, blend_565, decode_565_bytes, encode_565_bytes, pack_565};
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

/// 两个 565 变体（`Rgb565` / `Bgr565`）共用的存储字节序：小端。
///
/// 与 `DRM_FORMAT_RGB565` / `DRM_FORMAT_BGR565` 的定义一致——二者的差异只在
/// 位域里的红蓝位置。渲染器因此无需缓存字节序字段，热路径上 565 的字节序是常量。
const FMT_565_LITTLE_ENDIAN: bool = true;

/// 从裸指针读取一个 16 位打包像素（`Rgb565` / `Bgr565`）。
///
/// 逐字节访问而非按 `u16` 解引用：`pitch` 允许是奇数，行首不保证 2 字节对齐。
///
/// `little_endian` 描述**存储字节序**，与 565 的位域布局无关：两个 565 变体
/// 都是小端，真正的红蓝差异由 `layout_565()` 给出，在 `unpack_565` /
/// `blend_565` 中生效。此处只负责「两字节 → 一个 u16」。
///
/// # Safety
///
/// 调用方必须保证 `dst` 起至少 2 个字节可读。
#[inline(always)]
unsafe fn load_565(dst: *const u8, little_endian: bool) -> u16 {
    // SAFETY: 由调用方保证 dst..dst+2 可读。
    let bytes = unsafe { [*dst, *dst.add(1)] };
    decode_565_bytes(&bytes, little_endian)
}

/// 把一个 16 位打包像素写到裸指针（`Rgb565` / `Bgr565`）。
///
/// 与 [`load_565`] 对称，同样不要求 2 字节对齐。
///
/// # Safety
///
/// 调用方必须保证 `dst` 起至少 2 个字节可写。
#[inline(always)]
unsafe fn store_565(dst: *mut u8, word: u16, little_endian: bool) {
    let bytes = encode_565_bytes(word, little_endian);
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
    /// 16 位打包格式（`Rgb565` / `Bgr565`）的 565 位域布局。
    ///
    /// 8 位通道格式恒为 [`Layout565::RedHigh`]：该值在 2bpp 路径之外不会被
    /// 读取，位域差异由各自的 shift 表达。
    pub(crate) fmt_layout_565: Layout565,
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
        let (fmt_bpp, fmt_rgb_order, fmt_opaque_alpha, fmt_stores_color_alpha, fmt_layout_565) =
            match info.format {
                PixelFormat::Rgb24 => (3, true, 255, false, Layout565::RedHigh),
                PixelFormat::Bgr24 => (3, false, 255, false, Layout565::RedHigh),
                PixelFormat::Xrgb8888 => (4, false, 0, false, Layout565::RedHigh),
                PixelFormat::Argb8888 => (4, false, 255, true, Layout565::RedHigh),
                PixelFormat::Rgba8888 => (4, true, 255, true, Layout565::RedHigh),
                PixelFormat::Bgra8888 => (4, false, 255, true, Layout565::RedHigh),
                // 565 没有逐字节的 RGB 顺序概念，2bpp 路径不使用 `fmt_rgb_order`，
                // 这里填 `true` 仅为占位；位域差异由 `fmt_layout_565` 表达。
                // 两个 565 变体都是小端存储，故不需要字节序缓存字段。
                PixelFormat::Rgb565 => (2, true, 255, false, Layout565::RedHigh),
                PixelFormat::Bgr565 => (2, true, 255, false, Layout565::BlueHigh),
            };
        Self {
            info,
            front_buffer,
            back_buffer,
            fmt_bpp,
            fmt_rgb_order,
            fmt_opaque_alpha,
            fmt_stores_color_alpha,
            fmt_layout_565,
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
        self.info.bytes_per_pixel()
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
    /// `fmt_layout_565`）代替 `PixelFormat::write_color` 的运行时枚举匹配，
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
        let layout_565 = self.fmt_layout_565;
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
            let word = decode_565_bytes(dst, FMT_565_LITTLE_ENDIAN);
            let bytes = encode_565_bytes(blend_565(word, color, layout_565), FMT_565_LITTLE_ENDIAN);
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
        let layout_565 = self.fmt_layout_565;
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
                    let word = unsafe { load_565(dst, FMT_565_LITTLE_ENDIAN) };
                    let blended = blend_565(word, &color.mix_alpha(m_alpha), layout_565);
                    unsafe { store_565(dst, blended, FMT_565_LITTLE_ENDIAN) };
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
        let layout_565 = self.fmt_layout_565;
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
            let pattern = encode_565_bytes(pack_565(color, layout_565), FMT_565_LITTLE_ENDIAN);
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
        let layout_565 = self.fmt_layout_565;

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
                let word = unsafe { load_565(dst, FMT_565_LITTLE_ENDIAN) };
                let blended = blend_565(word, color, layout_565);
                unsafe { store_565(dst, blended, FMT_565_LITTLE_ENDIAN) };
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
    ///
    /// # 端点顺序
    ///
    /// `start_x` / `end_x` **允许反序**：`fill_triangle` 的两条扫描线交点相对大小取决于
    /// 顶点绕向，多数顶点顺序下都会出现 `start_x > end_x`（实测 `(10,2),(1,9),(12,12)`
    /// 的 12 行里有 9 行反序）。本函数先把两个端点各自夹取到 `[0, width - 1]`，
    /// **再**交换成升序，因此反序区间仍能正确填充。
    ///
    /// # 为什么夹取必须在交换之前
    ///
    /// 早先的实现是「各自夹取一端后，若 `sx > ex` 就交换」，但互换不能让越界区间变合法：
    /// `(97, 100)` 在 `width = 4` 下得到 `sx = 97`、`ex = 3`，交换后是 `(3, 97)`，
    /// `ex = 97` 依旧越界，`fill_span_opaque` 随即以 `get_unchecked_mut` 越界写——
    /// 那是内存破坏（release 下没有 UB 检查，会静默写坏内存）。触发路径是「圆心只在
    /// x 方向出屏、y 仍在屏内」，例如 UI 元素横向滚出屏幕；`fill_circle` /
    /// `fill_arc` / `fill_triangle` 都会踩到。
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
        if color.a == 0 {
            // 完全透明，跳过
            return;
        }
        let max_x = self.info.width.saturating_sub(1) as i64;
        // **先排序**：`fill_triangle` 会传反序跨度（顶点绕向所致），端点必须先
        // 规整成升序，否则下面的区间求交会把「反序」误判成「空」。
        let (lo, hi) = if start_x <= end_x {
            (start_x, end_x)
        } else {
            (end_x, start_x)
        };
        // **再求交**：与可见列 `[0, max_x]` 求交。交集为空说明整段落在屏幕外，
        // 必须在夹取之前判空——直接 clamp 两端会把「完全越界」的跨度压成屏幕边缘的
        // 单像素（圆心在 x 方向出屏时，整行跨度都被压到 max_x，凭空画出一整列）。
        let sx = lo.max(0);
        let ex = hi.min(max_x);
        if sx > ex {
            return;
        }
        // 此时 sx、ex 都落在 [0, max_x] 内且 sx <= ex，下面的 `as usize` 安全。
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
        //
        // **还必须收窄到首行的可见长度**：4bpp 的 `pattern_len` 恒为 16，而
        // `row_len = width × bpp` 可能小于 16（例如 width ≤ 3 的 4bpp 帧缓冲）。
        // 此时若把 16 字节整体 memcpy 到 `base_ptr`，会越过首行末尾、污染该行
        // 的行填充字节；随后的 `pattern_fill(first_row, ..)` 只覆盖 `[0, row_len)`，
        // 无法撤销已写入的填充区。行填充字节对硬件有意义（部分面板用行尾字节做
        // 同步/溢出标记），故必须一并收窄。
        let init_len = pattern_len.min(total_bytes);
        let first_row_init_len = init_len.min(row_len);

        let buffer = self.buffer_mut();
        let base_ptr = buffer.as_mut_ptr();

        if pitch == row_len {
            // 无行间填充：一次铺满整个 buffer
            //
            // SAFETY: init_len <= min(pattern_len, total_bytes)，既不越过 16 字节的
            // pattern，也不越过缓冲区（构造期保证 buffer.len() >= pitch * height）；
            // pattern 是栈上数组，与缓冲区不重叠。
            unsafe {
                #[cfg(feature = "simd")]
                simd::memcpy(base_ptr, pattern.as_ptr(), init_len);
                #[cfg(not(feature = "simd"))]
                core::ptr::copy_nonoverlapping(pattern.as_ptr(), base_ptr, init_len);
            }
            // SAFETY: total_bytes = pitch * height <= buffer.len()，切片不越过分配。
            let full_buf = unsafe { core::slice::from_raw_parts_mut(base_ptr, total_bytes) };
            pattern_fill(full_buf, &pattern[..init_len]);
        } else {
            // 有行间填充：**只**铺满第一行的可见字节，再逐行复制。
            //
            // 首行切片长度恰为 row_len，故任何写入都不会触及填充区。
            //
            // SAFETY: pitch >= row_len（FramebufferInfo 构造期校验）且 height >= 1
            // （守卫已排除 0），故 row_len <= total_bytes <= buffer.len()。
            let first_row = unsafe { core::slice::from_raw_parts_mut(base_ptr, row_len) };
            pattern_fill(first_row, &pattern[..first_row_init_len]);

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
            pixel[..2].copy_from_slice(&encode_565_bytes(
                pack_565(color, self.fmt_layout_565),
                FMT_565_LITTLE_ENDIAN,
            ));
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
        let layout_565 = self.fmt_layout_565;
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
                    let word = unsafe { load_565(dst, FMT_565_LITTLE_ENDIAN) };
                    let blended = blend_565(word, color, layout_565);
                    unsafe { store_565(dst, blended, FMT_565_LITTLE_ENDIAN) };
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
            // 横向与纵向都用饱和运算：`cx` / `cy` 可能是 `sat_i64(usize::MAX)`
            // 得到的 `i64::MAX`，裸加会溢出（debug 下 panic，release 下回绕成
            // 负数而使 `fill_visible_horizontal_span` 的夹取失效）。这与
            // `emit_boundary_row` 的处理一致——本行画的是**盘外**像素，
            // 饱和到 i64 边界即可，真正的裁剪在可见区内完成。
            let (left, right) = (cx.saturating_sub(half), cx.saturating_add(half));
            // `k == 0` 时 `cy + 0 == cy - 0` 是同一行，只写一次。
            renderer.fill_visible_horizontal_span(left, right, cy.saturating_add(k), &color);
            if k != 0 {
                renderer.fill_visible_horizontal_span(left, right, cy.saturating_sub(k), &color);
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
                // 同 `fill_circle`：饱和运算避免 `cx ± half` 在
                // `cx == i64::MAX`（圆心为 usize::MAX）时溢出。
                let lo = cx.saturating_sub(half).max(0);
                let hi = cx.saturating_add(half).min(width - 1);
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
    // 测试需要堆分配构造帧缓冲；`no_std` 下必须显式引入 alloc。
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{FMT_565_LITTLE_ENDIAN, Renderer, load_565, store_565};
    use crate::angle::Angle;
    use crate::color::{Color, PixelFormat};
    use crate::framebuffer::FramebufferInfo;
    use crate::pixel::Pixel;
    use crate::utils::sat_i64;

    /// 圆图元的种类。显式枚举而非 `bool` 参数，让调用点自解释。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum CircleKind {
        /// 只描边（`draw_circle` / `draw_arc`）。
        Outline,
        /// 实心填充（`fill_circle` / `fill_arc`）。
        Filled,
    }

    /// 缓冲布局的构造方式。显式枚举代替布尔参数，区分「紧凑」与「带行填充」。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Layout {
        /// `pitch == width × bpp`，行间无填充。
        Tight,
        /// `pitch == width × bpp + 2`，每行末尾多 2 字节填充。
        Padded,
    }

    /// 按布局构造帧缓冲：`width × bpp` 或再多 2 字节填充。
    fn make_info(w: usize, h: usize, format: PixelFormat, layout: Layout) -> FramebufferInfo {
        let bpp = format.bytes_per_pixel();
        let pitch = match layout {
            Layout::Tight => w * bpp,
            Layout::Padded => w * bpp + 2,
        };
        FramebufferInfo::new(w, h, pitch, format)
    }

    /// 全部 8 种像素格式，供表驱动测试复用。
    const ALL_FORMATS: [PixelFormat; 8] = [
        PixelFormat::Rgb24,
        PixelFormat::Bgr24,
        PixelFormat::Xrgb8888,
        PixelFormat::Argb8888,
        PixelFormat::Rgba8888,
        PixelFormat::Bgra8888,
        PixelFormat::Rgb565,
        PixelFormat::Bgr565,
    ];

    /// 565 的两个变体：红在高位与蓝在高位（均为小端存储）。
    const FORMATS_565: [PixelFormat; 2] = [PixelFormat::Rgb565, PixelFormat::Bgr565];

    /// `Bgr24` 每像素字节数——本模块多数像素集合断言用它做固定步长。
    const BGR24_BYTES: usize = 3;

    // ————————————————————————————————— 参考实现 —————————————————————————————————

    /// Alpha 混合的参考公式：`(src × a + dst × (255 − a) + 255) >> 8`。
    ///
    /// 这是全库唯一的混合语义（[`PixelFormat::blend_channel`] 是同一公式），
    /// 参考实现照抄它以保证「参考值 ≠ 实现」的独立性只体现在调用路径上。
    fn blend_channel_ref(src: u8, dst: u8, alpha: u16, inv_alpha: u16) -> u8 {
        ((src as u16 * alpha + dst as u16 * inv_alpha + 255) >> 8) as u8
    }

    /// 把 `src` 以 `alpha` 混合到 `dst` 上的参考结果（三通道，alpha 恒 255）。
    fn blend_ref(src: &Color, dst: &Color, alpha: u8) -> Color {
        let (a, inv) = (alpha as u16, 255 - alpha as u16);
        Color::with_alpha(
            blend_channel_ref(src.r, dst.r, a, inv),
            blend_channel_ref(src.g, dst.g, a, inv),
            blend_channel_ref(src.b, dst.b, a, inv),
            255,
        )
    }

    /// 565 量化的参考实现：`(v × max + 127) / 255`。
    fn quantize_ref(v: u8, bits: u8) -> u16 {
        let max = (1u32 << bits) - 1;
        ((v as u32 * max + 127) / 255) as u16
    }

    /// 565 打包的参考实现：按格式的位域布局写入红蓝两段。
    ///
    /// 与 `color::pack_565` 独立重写：这里显式按 `PixelFormat` 分派，
    /// 使得「位域随格式变化」这一契约在测试侧也有独立表述。
    fn pack_565_ref(format: PixelFormat, c: &Color) -> u16 {
        let r = quantize_ref(c.r, 5);
        let g = quantize_ref(c.g, 6);
        let b = quantize_ref(c.b, 5);
        match format {
            // Bgr565：蓝占高 5 位、红占低 5 位。
            PixelFormat::Bgr565 => (b << 11) | (g << 5) | r,
            // Rgb565 及其它：红占高 5 位、蓝占低 5 位。
            _ => (r << 11) | (g << 5) | b,
        }
    }

    /// 精确圆盘参考实现：逐像素判定 `dx² + dy² <= radius²`。
    ///
    /// 与被测实现的「逐行求半宽」写法完全独立，用作圆盘语义的基准。
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
    /// 邻域判定用**纯圆方程**，不把帧缓冲外当作盘外——被测实现先算几何边界、
    /// 再裁剪到屏幕，故屏幕边缘的像素不会因「邻居在屏幕外」而被误判为边界。
    fn reference_disk_boundary(
        w: usize,
        h: usize,
        center: Pixel,
        radius: usize,
    ) -> Vec<(usize, usize)> {
        let (cx, cy) = (center.x as i64, center.y as i64);
        let r2 = (radius as i64) * (radius as i64);
        let inside = |x: i64, y: i64| (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r2;
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let (ix, iy) = (x as i64, y as i64);
                let is_boundary = inside(ix, iy)
                    && (!inside(ix - 1, iy)
                        || !inside(ix + 1, iy)
                        || !inside(ix, iy - 1)
                        || !inside(ix, iy + 1));
                if is_boundary {
                    out.push((x, y));
                }
            }
        }
        out
    }

    /// Bresenham 参考实现：**朴素地从原始端点整段步进**，仅过滤越界像素。
    ///
    /// 这正是 `draw_line_unchecked` 做区间收窄之前的行为（`11c3e58` 之前），
    /// 也是本测试的独立判据。收窄必须与之逐像素等价——它保留原始端点与
    /// 相位，只跳过必然越界的迭代。
    ///
    /// # 为什么可以提前 break 而不削弱独立性
    ///
    /// 朴素写法在端点取 `usize::MAX` 时会循环约 2^63 次，把测试自己挂死。
    /// 这里在 `x > x_hi_bound` 时 break，**这不是收窄**，而是可证明等价的剪枝：
    ///
    /// * 非 steep 路径 `px = x`，`x` 单调递增，故 `x > width-1` 之后每个
    ///   迭代的 `px` 必越界；
    /// * steep 路径 `py = x`，同理 `x > height-1` 之后每个迭代的 `py` 必越界。
    ///
    /// 因此剪掉的迭代**恒不产出任何可见像素**，与朴素实现逐位等价。
    /// 它与生产代码的结构刻意不同：这里没有 `x_lo`、没有预热重演、
    /// 也没有「先算区间再决定」的两段式写法，故仍能抓住收窄写错相位的 bug。
    ///
    /// 坐标用 `i64` 传入以容纳 `usize::MAX` 端点；生产代码用 `sat_i64` 饱和
    /// 转换，参考实现照搬同一口径（见 `cast_coord`）。
    fn reference_bresenham(p1: (i64, i64), p2: (i64, i64), w: i64, h: i64) -> Vec<(i64, i64)> {
        let (mut x1, mut y1) = p1;
        let (mut x2, mut y2) = p2;
        let steep = (y2 - y1).unsigned_abs() > (x2 - x1).unsigned_abs();
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
        let x_hi_bound = if steep { h - 1 } else { w - 1 };

        let mut error = dx / 2;
        let mut y = y1;
        let mut out = Vec::new();
        for x in x1..=x2 {
            // 可证明的剪枝：此后的迭代恒不可见（见文档）。
            if x > x_hi_bound {
                break;
            }
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

    /// 按饱和口径把 `usize` 坐标转成 `i64`（**不用 `as`**：`usize::MAX as i64 == -1`）。
    fn cast_coord(v: usize) -> i64 {
        sat_i64(v)
    }

    /// 完整 Bresenham 参考的「行优先排序 + 收窄到 `usize` 坐标」包装。
    ///
    /// 被测路径产出的是行优先坐标序，而参考按迭代顺序产出，故必须排序后比较。
    /// 同时把 `i64` 收窄成 `usize`（参考已过滤越界，故转换恒安全）。
    fn reference_line_sorted(
        a: (usize, usize),
        b: (usize, usize),
        w: usize,
        h: usize,
    ) -> Vec<(usize, usize)> {
        let mut out = reference_bresenham(
            (cast_coord(a.0), cast_coord(a.1)),
            (cast_coord(b.0), cast_coord(b.1)),
            w as i64,
            h as i64,
        );
        // 按 `(y, x)` 行优先排序：`sort_unstable` 对 `(x, y)` 元组本就等价于
        // 先比较 x 再比较 y，故显式映射成 `(y, x)` 后排序以对齐被测路径。
        out.sort_by_key(|(x, y)| (*y, *x));
        out.into_iter()
            .map(|(x, y)| (x as usize, y as usize))
            .collect()
    }

    // ————————————————————————————————— 缓冲辅助 —————————————————————————————————

    /// 构造一个全零的单缓冲帧缓冲，返回 `(FramebufferInfo, Vec<u8>)`。
    ///
    /// 调用方拿 Vec 自己造渲染器——**不能**在这里返回借用它的 `Renderer`：
    /// 渲染器借用的正是这个 Vec，二者不可能同时作为返回值交出（所有权自相矛盾）。
    /// 需要复用时，调用方直接 `Renderer::new(info, &mut fb)`。
    fn scratch(
        w: usize,
        h: usize,
        format: PixelFormat,
        layout: Layout,
    ) -> (FramebufferInfo, Vec<u8>) {
        let info = make_info(w, h, format, layout);
        let fb = vec![0u8; info.buffer_size()];
        (info, fb)
    }

    /// 在给定缓冲上建单缓冲渲染器的便捷包装（调用方持有 Vec）。
    fn render_on<'a>(info: FramebufferInfo, fb: &'a mut [u8]) -> Renderer<'a> {
        Renderer::new(info, fb)
    }

    /// 统计缓冲中被实际着色（存在非零通道）的像素数。
    fn painted_pixels(fb: &[u8]) -> usize {
        fb.chunks_exact(BGR24_BYTES)
            .filter(|pixel| pixel.iter().any(|byte| *byte != 0))
            .count()
    }

    /// 收集单缓冲渲染器里被着色为 `color` 的像素坐标（行优先序）。
    fn drawn_pixels(
        renderer: &Renderer<'_>,
        w: usize,
        h: usize,
        color: &Color,
    ) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                if renderer.get_pixel(x, y) == Some(*color) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    // ——————————————————————— S09 · 构造与缓冲管理 ———————————————————————

    /// `Renderer::new` 必须拒绝短于 `pitch × height` 的前台缓冲。
    ///
    /// 守护的是「安全 API 不越界写」的前提：所有底层写入都用
    /// `get_unchecked_mut` 且只以 `info` 为准判界，切片不足时必然越界（UB）。
    #[test]
    #[should_panic(expected = "front_buffer too small")]
    fn new_rejects_front_buffer_shorter_than_pitch_times_height() {
        // 元数据声称 4×4（需要 48 字节），切片却只有一行 12 字节。
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 12];
        let _ = Renderer::new(info, &mut front);
    }

    /// 切片**恰好**等于 `pitch × height` 时必须通过（边界不被误拒）。
    ///
    /// 与上一条成对：断言口径就是 `>= buffer_size()`，不多不少。
    #[test]
    fn new_accepts_front_buffer_exactly_buffer_size() {
        let info = FramebufferInfo::new(4, 4, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 48];
        let mut renderer = Renderer::new(info, &mut front);

        // 写满最后一行的最后一列，确保边界口径不是「留余量」的保守值。
        renderer.set_pixel(3, 3, &Color::new(0x12, 0x34, 0x56));
        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(0x12, 0x34, 0x56)));
        assert_eq!(&renderer.buffer()[45..48], &[0x56, 0x34, 0x12]);
    }

    /// 带行填充的缓冲（`len > pitch × height`）必须合法，末行末列仍写对。
    ///
    /// `pitch = 16 > width × bpp = 12` 是真实硬件布局（行对齐/DWORD 对齐）。
    #[test]
    fn new_accepts_buffer_with_row_padding_and_writes_last_pixel() {
        let info = FramebufferInfo::new(4, 4, 16, PixelFormat::Bgr24);
        // 切片故意留出多余字节：构造期只要求 `>= buffer_size()`。
        let mut front = [0u8; 96];
        let mut renderer = Renderer::new(info, &mut front);

        renderer.set_pixel(3, 3, &Color::new(0x12, 0x34, 0x56));

        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(0x12, 0x34, 0x56)));
        // 末行末像素的偏移是 `(4-1) × 16 + 3 × 3 = 57`，而非紧凑口径的 45。
        assert_eq!(&renderer.buffer()[57..60], &[0x56, 0x34, 0x12]);
        // 行填充字节不得被写入。
        assert_eq!(&renderer.buffer()[60..64], &[0u8; 4]);
    }

    /// `with_back_buffer` 必须拒绝过短的后台缓冲。
    #[test]
    #[should_panic(expected = "back_buffer too small")]
    fn with_back_buffer_rejects_back_buffer_shorter_than_required() {
        let info = FramebufferInfo::new(4, 4, 16, PixelFormat::Rgba8888);
        let mut front = [0u8; 64];
        let mut back = [0u8; 63]; // 差一字节
        let _ = Renderer::with_back_buffer(info, &mut front, &mut back);
    }

    /// 访问器契约：`is_double_buffered` / `buffer` / `buffer_mut` / `pixel_size` / `info`。
    ///
    /// 关键点：`buffer()` 返回的是**当前可写**的那块（双缓冲下是后台），
    /// 单缓冲下才等于前台切片本身。
    #[test]
    fn renderer_accessors_report_mode_size_and_layout() {
        let make_info = || FramebufferInfo::new(7, 5, 7 * 4 + 4, PixelFormat::Rgba8888);
        let mut front = vec![0u8; make_info().buffer_size()];
        let mut back = vec![0u8; make_info().buffer_size()];

        {
            let mut renderer = Renderer::with_back_buffer(make_info(), &mut front, &mut back);
            assert!(renderer.is_double_buffered());
            assert_eq!(renderer.pixel_size(), 4);
            assert_eq!(renderer.info().width, 7);
            assert_eq!(renderer.info().height, 5);
            assert_eq!(renderer.info().pitch, 32);
            assert_eq!(renderer.info().format, PixelFormat::Rgba8888);

            // 双缓冲下 buffer() / buffer_mut() 都指向后台。
            renderer.clear(Color::new(1, 2, 3));
            assert_eq!(renderer.buffer()[0], 1);

            renderer.buffer_mut()[0] = 0x7E;
            assert_eq!(renderer.buffer()[0], 0x7E);
        }
        // 渲染器离开作用域后：后台被写入，前台保持初值 —— 绘制只落后台。
        assert_eq!(back[0], 0x7E, "buffer_mut() 应写入后台");
        assert_eq!(front[0], 0, "绘制必须落在后台，前台尚未被触碰");

        {
            let mut renderer = Renderer::new(make_info(), &mut front);
            assert!(!renderer.is_double_buffered());
            assert_eq!(
                renderer.pixel_size(),
                PixelFormat::Rgba8888.bytes_per_pixel()
            );
            renderer.buffer_mut()[0] = 0x5A;
            assert_eq!(renderer.buffer()[0], 0x5A);
        }
        assert_eq!(front[0], 0x5A, "单缓冲下 buffer_mut() 应写入前台");
    }

    /// `pixel_offset` 含行填充时正确，末行末列不越界。
    ///
    /// 直接对全部 `(x, y)` 断言偏移口径，并对最后一列断言
    /// `offset + bpp <= buffer_size()`。
    #[test]
    fn pixel_offset_accounts_for_row_padding_without_overflow() {
        for format in ALL_FORMATS {
            for layout in [Layout::Tight, Layout::Padded] {
                let (w, h) = (7usize, 5usize);
                let (info, mut fb) = scratch(w, h, format, layout);
                let renderer = render_on(info, &mut fb);
                let bpp = format.bytes_per_pixel();
                let pitch = renderer.info().pitch;

                for y in 0..h {
                    for x in 0..w {
                        assert_eq!(
                            renderer.pixel_offset(x, y),
                            y * pitch + x * bpp,
                            "{format:?} / {layout:?} 的 ({x},{y}) 偏移错误"
                        );
                    }
                }
                // 末行末列：偏移 + 像素字节数恰好落在缓冲末尾内。
                let last = renderer.pixel_offset(w - 1, h - 1);
                assert!(
                    last + bpp <= renderer.info().buffer_size(),
                    "{format:?} / {layout:?} 末行末列越界"
                );
            }
        }
    }

    /// 单缓冲的 `swap_buffers` 必须是空操作。
    #[test]
    fn swap_buffers_on_single_buffer_is_noop() {
        let (info, mut fb) = scratch(4, 2, PixelFormat::Rgba8888, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        renderer.clear(Color::new(9, 9, 9));
        let before = renderer.buffer().to_vec();

        renderer.swap_buffers();

        assert_eq!(
            renderer.buffer(),
            &before[..],
            "单缓冲下 present 不得改动任何字节"
        );
    }

    /// 双缓冲：绘制只落后台，`swap_buffers` 后前台逐字节等于单缓冲直接绘制的结果。
    ///
    /// 这条覆盖整条 present 链路——后台绘制、可见行拷贝、字节序与混合语义
    /// 都必须在两种模式下给出完全相同的像素。
    #[test]
    fn double_buffer_present_matches_single_buffer_byte_for_byte() {
        fn draw_sample(renderer: &mut Renderer<'_>) {
            renderer.clear(Color::new(9, 8, 7));
            renderer.fill_rect(Pixel::new(1, 0), 3, 2, Color::with_alpha(200, 100, 50, 128));
            renderer.draw_line(Pixel::new(0, 0), Pixel::new(4, 2), &Color::new(1, 1, 1));
            renderer.fill_circle(Pixel::new(2, 1), 2, Color::new(255, 0, 255));
        }

        let make_info = || FramebufferInfo::new(5, 3, 20, PixelFormat::Rgba8888);
        let mut single_fb = [0u8; 60];
        let mut front = [0u8; 60];
        let mut back = [0u8; 60];

        {
            let mut renderer = Renderer::new(make_info(), &mut single_fb);
            draw_sample(&mut renderer);
        }
        {
            let mut renderer = Renderer::with_back_buffer(make_info(), &mut front, &mut back);
            assert!(renderer.is_double_buffered());
            draw_sample(&mut renderer);
            // present 之前后台已有内容（缓冲以不同哨兵初始化，故按字节判定）。
            assert!(
                renderer.buffer().iter().any(|b| *b != 0),
                "双缓冲绘制应落在后台"
            );
            renderer.swap_buffers();
        }
        // present 之后前台逐字节等于单缓冲直接绘制的结果。
        assert_eq!(single_fb, front);
        assert!(
            front.iter().any(|b| *b != 0),
            "非平凡性：present 后前台确有内容"
        );
    }

    /// `swap_buffers` 必须保留行填充字节——不得整段 `memcpy` 覆盖填充区。
    ///
    /// 这正是 `sub_view` 子视口能安全 present 的前提：子渲染器切片按父缓冲区的
    /// `pitch` 跨行，整段拷贝会连面板以外的像素一起覆盖。
    #[test]
    fn swap_buffers_preserves_row_padding_bytes() {
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
        // 可见像素已呈现。
        assert_eq!(&front[0..3], &[1, 2, 3]);
        assert_eq!(&front[12..15], &[4, 5, 6]);
        // 行尾填充字节保持前台原值，未被后台的 0 覆盖。
        assert_eq!(&front[8..12], &[0xAA; 4]);
        assert_eq!(&front[20..24], &[0xAA; 4]);
        // 后台的填充字节仍是 0，说明拷贝范围严格等于可见区间。
        assert_eq!(&back[8..12], &[0u8; 4]);
    }

    /// 紧凑布局（`pitch == width × bpp`）下 `swap_buffers` 也要逐字节一致。
    ///
    /// 覆盖 `swap_buffers` 的另一条分支——它走整块 `copy_from_slice`，
    /// 切片长度口径必须是 `pitch × height` 而非 `buffer.len()`。
    #[test]
    fn swap_buffers_copies_whole_block_when_pitch_is_tight() {
        let info = FramebufferInfo::new(3, 2, 12, PixelFormat::Rgba8888);
        // 切片故意比 `buffer_size()` 长：整块拷贝必须只取前 `pitch × height` 字节，
        // 否则 `copy_from_slice` 长度不等会 panic。
        let mut front = vec![0xCDu8; info.buffer_size() + 7];
        let mut back = vec![0u8; info.buffer_size() + 7];
        {
            let mut renderer = Renderer::with_back_buffer(info, &mut front, &mut back);
            renderer.clear(Color::new(3, 2, 1));
            renderer.swap_buffers();
        }
        assert_eq!(&front[..24], &[3, 2, 1, 255].repeat(6)[..]);
        // 多余的 7 字节属于调用方，不得被触碰。
        assert_eq!(&front[24..], &[0xCD; 7]);
    }

    // ——————————————————————— S10 · 像素读写与格式落盘 ———————————————————————

    /// `set_pixel` 在 8 种格式下的字节布局必须精确。
    ///
    /// 这是落盘语义的基准：`fmt_rgb_order` 决定前 3 字节的通道序，
    /// 第 4 字节恒为该格式的 `opaque_alpha`（`Xrgb8888` 为 0，其余 4bpp 为 255），
    /// 565 则按各自的位域布局量化打包（小端存储）。
    #[test]
    fn set_pixel_byte_layout_per_format() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0xFF);

        // (格式, 期望的 4 字节缓冲表示；3bpp 只比较前 3 字节)
        let cases: [(PixelFormat, [u8; 4]); 8] = [
            (PixelFormat::Rgb24, [0x12, 0x34, 0x56, 0]),
            (PixelFormat::Bgr24, [0x56, 0x34, 0x12, 0]),
            (PixelFormat::Xrgb8888, [0x56, 0x34, 0x12, 0x00]),
            (PixelFormat::Argb8888, [0x56, 0x34, 0x12, 0xFF]),
            (PixelFormat::Rgba8888, [0x12, 0x34, 0x56, 0xFF]),
            (PixelFormat::Bgra8888, [0x56, 0x34, 0x12, 0xFF]),
            // 565：0x12/0x34/0x56 → Rgb565 word 0x11AA（红高位）→ 小端 [0xAA,0x11]；
            // Bgr565 蓝占高位 → word 0x51A2 → 小端 [0xA2,0x51]。
            // 二者都是小端；差异在位域而非字节序。
            (PixelFormat::Rgb565, [0xAA, 0x11, 0, 0]),
            (PixelFormat::Bgr565, [0xA2, 0x51, 0, 0]),
        ];

        for (format, expected) in cases {
            let (info, mut fb) = scratch(1, 1, format, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.set_pixel(0, 0, &color);
            let bpp = format.bytes_per_pixel();
            assert_eq!(
                &renderer.buffer()[..bpp],
                &expected[..bpp],
                "{format:?} 的落盘字节布局错误"
            );
        }
    }

    /// `Xrgb8888` 写入时第 4 字节必须为 0，且**不写出源 alpha**。
    ///
    /// 回归点：曾经把 `Color.a` 直接写进保留字节，导致半透明绘制污染 XRGB 面板。
    ///
    /// 扫描从 `alpha = 1` 起：`alpha == 0` 走的是 `write_color_at_offset` 的
    /// **提前返回**分支（完全不写），与「写入 pad=0」是不同语义，
    /// 另由 `fully_transparent_color_writes_nothing` 守护。
    #[test]
    fn xrgb8888_discards_source_alpha_and_forces_zero_pad_byte() {
        for alpha in [1u8, 2, 128, 254, 255] {
            let (info, mut fb) = scratch(1, 1, PixelFormat::Xrgb8888, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.set_pixel(0, 0, &Color::with_alpha(0x12, 0x34, 0x56, alpha));
            assert_eq!(
                renderer.buffer()[3],
                0,
                "alpha={alpha} 时 Xrgb8888 的保留字节应为 0"
            );
            // 读取时 alpha 恒为不透明（格式无 alpha 通道），RGB 按 alpha 混合。
            // 低 alpha 下混合结果确实接近黑——那是 `blend_channel` 的正确结果，
            // 本测试的护栏点是第 4 字节而非 RGB。
            assert_eq!(renderer.get_pixel(0, 0).map(|c| c.a), Some(255));
        }
    }

    /// 完全透明（`a == 0`）的颜色在所有绘制入口都**不写任何字节**。
    ///
    /// 这是「像素级零成本跳过」的契约：它是提前返回而非「写入原值」，
    /// 故被跳过的区域必须逐字节保持调用前的样子。
    #[test]
    fn fully_transparent_color_writes_nothing() {
        for format in ALL_FORMATS {
            let (info, mut fb) = scratch(3, 2, format, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            // 先铺一个可辨识的背景，再试图用全透明色覆盖。
            renderer.clear(Color::new(0x77, 0x88, 0x99));
            let before = renderer.buffer().to_vec();

            let ghost = Color::with_alpha(0x12, 0x34, 0x56, 0);
            renderer.set_pixel(1, 1, &ghost);
            renderer.set_pixel_at(Pixel::new(0, 0), &ghost);
            renderer.draw_line(Pixel::new(0, 0), Pixel::new(2, 1), &ghost);
            renderer.fill_rect(Pixel::new(0, 0), 3, 2, ghost);
            renderer.fill_circle(Pixel::new(1, 1), 2, ghost);
            renderer.fill_triangle(Pixel::new(0, 0), Pixel::new(2, 0), Pixel::new(1, 2), &ghost);

            assert_eq!(
                renderer.buffer(),
                &before[..],
                "{format:?}：a=0 的颜色写入了字节"
            );
        }
    }

    /// 越界 `get_pixel` 返回 `None`；越界 `set_pixel` 静默忽略且不写内存。
    #[test]
    fn out_of_bounds_pixel_access_is_rejected_without_writing() {
        let info = FramebufferInfo::new(4, 3, 12, PixelFormat::Bgr24);
        let mut front = [0u8; 36];
        let mut renderer = Renderer::new(info, &mut front);

        // 读越界
        assert_eq!(renderer.get_pixel(4, 0), None);
        assert_eq!(renderer.get_pixel(0, 3), None);
        assert_eq!(renderer.get_pixel(usize::MAX, usize::MAX), None);

        // 写越界后内存必须逐字节不变。
        renderer.set_pixel(4, 0, &Color::new(255, 255, 255));
        renderer.set_pixel(0, 3, &Color::new(255, 255, 255));
        renderer.set_pixel(usize::MAX, 0, &Color::new(255, 255, 255));
        renderer.set_pixel_at(Pixel::new(0, usize::MAX), &Color::new(255, 255, 255));
        assert_eq!(renderer.buffer(), &[0u8; 36], "越界写入污染了帧缓冲");
    }

    /// `*_at`（`Pixel` 版）与坐标版在界内与越界时都逐点等价。
    #[test]
    fn pixel_variant_api_matches_coordinate_api() {
        let (info, mut fb) = scratch(6, 4, PixelFormat::Rgba8888, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        let c1 = Color::new(11, 22, 33);
        let c2 = Color::with_alpha(44, 55, 66, 128);

        renderer.set_pixel_at(Pixel::new(2, 1), &c1);
        renderer.set_pixel(2, 1, &c2);
        assert_eq!(
            renderer.get_pixel_at(Pixel::new(2, 1)),
            renderer.get_pixel(2, 1)
        );
        assert_eq!(
            renderer.get_pixel_at(Pixel::new(2, 1)),
            Some(blend_ref(&c2, &c1, c2.a))
        );

        // 越界时两者同为 None / 静默忽略。
        assert_eq!(renderer.get_pixel_at(Pixel::new(6, 0)), None);
        renderer.set_pixel_at(Pixel::new(6, 0), &Color::new(255, 255, 255));
        assert_eq!(renderer.get_pixel_at(Pixel::new(0, 4)), None);
        renderer.set_pixel_at(Pixel::new(0, 4), &Color::new(255, 255, 255));
        // 界内对照：越界写入没有污染任何像素。
        // 注意未写入过的 4bpp 像素，其第 4 字节保持缓冲初值 0，
        // 故 `read_color` 解出的 alpha 是 0 而不是 255 —— 契约只保证
        // 「写过的像素第 4 字节是格式的 opaque_alpha」，不保证未写的像素。
        assert_eq!(
            renderer.get_pixel(0, 0),
            Some(Color::with_alpha(0, 0, 0, 0))
        );
        assert_eq!(
            renderer.get_pixel(5, 3),
            Some(Color::with_alpha(0, 0, 0, 0))
        );
    }

    /// `*_unchecked` 在界内必须与安全路径逐像素一致。
    ///
    /// **不测越界**：越界是调用方 UB，不是可断言的契约。
    #[test]
    fn unchecked_pixel_api_matches_safe_path_within_bounds() {
        let (info, mut fb) = scratch(5, 3, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        let color = Color::new(0x12, 0x34, 0x56);
        let semi = Color::with_alpha(200, 100, 50, 77);

        for y in 0..3 {
            for x in 0..5 {
                // SAFETY: 测试显式用 `info` 的 width/height 约束循环范围。
                unsafe { renderer.set_pixel_unchecked(x, y, &color) };
                assert_eq!(renderer.get_pixel(x, y), Some(color));
                // SAFETY: 同上，坐标在界内。
                let got = unsafe { renderer.get_pixel_unchecked(x, y) };
                assert_eq!(got, color);

                unsafe { renderer.set_pixel_at_unchecked(Pixel::new(x, y), &semi) };
                assert_eq!(
                    renderer.get_pixel_at(Pixel::new(x, y)),
                    renderer.get_pixel(x, y)
                );
                // SAFETY: 同上。
                let unchecked = unsafe { renderer.get_pixel_at_unchecked(Pixel::new(x, y)) };
                assert_eq!(Some(unchecked), renderer.get_pixel(x, y));
            }
        }
        // 混合结果确实落盘（不是被覆盖成源色）。
        assert_eq!(
            renderer.get_pixel(0, 0),
            Some(blend_ref(&semi, &color, semi.a))
        );
    }

    /// `set_pixel` 的 alpha 混合落盘结果必须与逐通道参考一致（`Rgba8888` / `Bgr24`）。
    #[test]
    fn set_pixel_alpha_blend_matches_per_channel_reference() {
        // Bgr24：字节序为 B,G,R，故 buffer[..3] 是 [blend(B), blend(G), blend(R)]。
        {
            let (info, mut fb) = scratch(1, 1, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            let dst = Color::new(10, 20, 30);
            let src = Color::with_alpha(110, 120, 130, 128);
            renderer.set_pixel(0, 0, &dst);
            renderer.set_pixel(0, 0, &src);

            let want = blend_ref(&src, &dst, src.a);
            assert_eq!(
                renderer.buffer()[..3],
                [want.b, want.g, want.r],
                "Bgr24 的混合字节序错误"
            );
        }

        // Rgba8888：字节序为 R,G,B,A，第 4 字节恒 255。
        {
            let (info, mut fb) = scratch(1, 1, PixelFormat::Rgba8888, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            let dst = Color::new(20, 40, 60);
            let src = Color::with_alpha(120, 140, 160, 128);
            renderer.set_pixel(0, 0, &dst);
            renderer.set_pixel(0, 0, &src);

            let want = blend_ref(&src, &dst, src.a);
            assert_eq!(renderer.buffer()[..4], [want.r, want.g, want.b, 255]);
        }

        // 565：解码 → 逐通道混合 → 量化回写，误差只来自量化。
        for format in FORMATS_565 {
            let (info, mut fb) = scratch(1, 1, format, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            // 先种一个可精确表示的目标色（白），避免量化误差干扰比较。
            renderer.set_pixel(0, 0, &Color::new(255, 255, 255));
            let src = Color::with_alpha(255, 255, 255, 128);
            renderer.set_pixel(0, 0, &src);

            // 参考：解出目标色、按 blend_channel 混合、再量化打包。
            let dst = Color::new(255, 255, 255);
            let want = pack_565_ref(format, &blend_ref(&src, &dst, src.a));
            // 两个 565 变体都是小端：字节 0 是位域低字节。
            let bytes = [(want & 0xFF) as u8, ((want >> 8) & 0xFF) as u8];
            assert_eq!(
                renderer.buffer()[..2],
                bytes,
                "{format:?} 的 565 混合落盘错误"
            );
        }
    }

    /// 混合写入时 alpha 通道按格式契约落盘，绝不写出源 alpha。
    ///
    /// 3bpp 无 alpha 字节；4bpp 的第 4 字节恒为该格式的 `opaque_alpha`；
    /// 565 无 alpha 通道。逐个半透明 alpha 扫描以覆盖混合路径的全部分支。
    #[test]
    fn blended_write_never_persists_source_alpha_byte() {
        for format in ALL_FORMATS {
            let bpp = format.bytes_per_pixel();
            let (info, mut fb) = scratch(1, 1, format, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            // 先把第 4 字节种成非 0，确保后续写入会覆写它。
            if bpp == 4 {
                renderer.buffer_mut()[3] = 0x7F;
            }

            for alpha in 1u8..255 {
                let (info, mut fb) = scratch(1, 1, format, Layout::Tight);
                let mut renderer = render_on(info, &mut fb);
                renderer.set_pixel(0, 0, &Color::new(200, 100, 50));
                renderer.set_pixel(0, 0, &Color::with_alpha(10, 20, 30, alpha));

                match format {
                    // Xrgb8888 的保留字节恒 0。
                    PixelFormat::Xrgb8888 => {
                        assert_eq!(renderer.buffer()[3], 0, "Xrgb8888 alpha={alpha}")
                    }
                    // 其余 4bpp 的第 4 字节恒 255，绝不是源的 alpha。
                    PixelFormat::Argb8888 | PixelFormat::Rgba8888 | PixelFormat::Bgra8888 => {
                        assert_eq!(renderer.buffer()[3], 255, "{format:?} alpha={alpha}")
                    }
                    // 3bpp 与 565 没有第 4 字节可写，alpha 必然落不了盘。
                    _ => {}
                }
            }
        }
    }

    /// 私有 565 内核 `load_565` / `store_565` 必须是 `decode`/`encode` 的对称封装。
    ///
    /// `pitch` 允许是奇数，行首不保证 2 字节对齐，故两个内核都逐字节访问；
    /// 这里专门用**奇数偏移**的裸指针确认它们不解引用未对齐的 `u16`。
    #[test]
    fn load_565_and_store_565_roundtrip_at_odd_offset() {
        let word = pack_565_ref(PixelFormat::Rgb565, &Color::new(0x12, 0x34, 0x56));
        // 565 恒为小端存储，故字节序参数只有一种取值。
        assert!(FMT_565_LITTLE_ENDIAN, "两个 565 变体都必须是小端存储");
        // 故意让 `src` 的起始偏移为奇数：任何 `u16` 解引用都是 UB。
        let mut storage = [0u8; 8];
        unsafe {
            let p = storage.as_mut_ptr().add(1);
            store_565(p, word, FMT_565_LITTLE_ENDIAN);
            assert_eq!(load_565(p, FMT_565_LITTLE_ENDIAN), word, "往返失败");
        }
        // 存储字节序与 `encode_565_bytes` 一致（小端：字节 0 是低字节）。
        let expected = [(word & 0xFF) as u8, (word >> 8) as u8];
        assert_eq!(storage[1..3], expected);
    }

    // ——————————————————————— S11 · clear ———————————————————————

    /// `clear` 的单像素规范字节布局参考（与 `build_pattern` 独立重写）。
    ///
    /// **关键语义**：`Argb8888` / `Rgba8888` / `Bgra8888` 的第 4 字节写的是
    /// `color.a` 本身（`fmt_stores_color_alpha == true`），只有 `Xrgb8888` 写 0。
    /// `clear` 与 `set_pixel` 在这一点上**故意不同**：`set_pixel` 永远写格式的
    /// `opaque_alpha`，而 `clear` 保留调用方给定的 alpha。
    fn clear_pixel_ref(format: PixelFormat, c: &Color) -> Vec<u8> {
        match format {
            PixelFormat::Rgb24 => vec![c.r, c.g, c.b],
            PixelFormat::Bgr24 => vec![c.b, c.g, c.r],
            PixelFormat::Xrgb8888 => vec![c.b, c.g, c.r, 0],
            PixelFormat::Argb8888 | PixelFormat::Bgra8888 => vec![c.b, c.g, c.r, c.a],
            PixelFormat::Rgba8888 => vec![c.r, c.g, c.b, c.a],
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                // 两个变体都是小端：字节 0 是位域低字节。
                let word = pack_565_ref(format, c);
                vec![(word & 0xFF) as u8, (word >> 8) as u8]
            }
        }
    }

    /// 全部 8 种格式下，`clear` 后每个可见像素都等于参考字节布局。
    ///
    /// 紧凑与带填充两种 `pitch` 都覆盖：`clear` 走 `pitch == row_len` 与
    /// 「先铺首行再逐行复制」两条不同分支。
    #[test]
    fn clear_matches_per_pixel_reference_for_all_formats() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x9A);

        for format in ALL_FORMATS {
            for layout in [Layout::Tight, Layout::Padded] {
                let (w, h) = (5usize, 4usize);
                let (info, mut fb) = scratch(w, h, format, layout);
                let mut renderer = render_on(info, &mut fb);
                let pitch = renderer.info().pitch;
                let bpp = format.bytes_per_pixel();
                let pixel = clear_pixel_ref(format, &color);

                renderer.clear(color);

                for y in 0..h {
                    for x in 0..w {
                        let off = y * pitch + x * bpp;
                        assert_eq!(
                            &fb[off..off + bpp],
                            &pixel[..],
                            "{format:?} / {layout:?} 的 ({x},{y}) 像素错误"
                        );
                    }
                }
            }
        }
    }

    /// `clear` 必须逐字节保持行填充区不变。
    ///
    /// # 回归说明（曾经的 bug）
    ///
    /// `clear` 早期版本把 `build_pattern` 的 16 字节模式**无条件** `memcpy` 到
    /// `base_ptr`，之后才用 `pattern_fill` 重写首行的 `row_len` 个可见字节。
    /// 触发条件是 `pitch > row_len`（有行填充）**且** `row_len < 16`：
    ///
    /// * 4bpp 的 `pattern_len` 恒为 16，故 `width ≤ 3` 触发；
    /// * 2bpp 的 565 同理，`width ≤ 7` 触发；
    /// * 3bpp 的 `pattern_len = 3 ≤ row_len`，**从不触发**。
    ///
    /// 症状：那次 16 字节 memcpy 写穿若干整行，把被跨过各行的行填充字节涂成
    /// 像素数据；随后逐行拷贝的长度只有 `row_len`，无法撤销。
    /// 实测（哨兵 `0xEE`）：`Xrgb8888` 3×3 pitch=14 → 首行 2 字节被污染；
    /// `Rgba8888` 2×3 pitch=12 → 4 字节；`Rgb565` 4×3 pitch=10 → 2 字节。
    /// `Rgb24` 3×3 与 `Xrgb8888` 8×3（`row_len ≥ 16`）均不触发。
    ///
    /// 行填充字节对硬件有实际意义（部分面板用行尾字节做同步/溢出标记），
    /// 被 `clear` 涂掉是真实故障，故本测试是回归护栏而非风格偏好。
    /// 它与 `swap_buffers_preserves_row_padding_bytes` 构成同一契约的两端：
    /// 清屏与呈现都不得触碰填充区。
    #[test]
    fn clear_leaves_row_padding_untouched() {
        for format in ALL_FORMATS {
            // 覆盖 bug 的触发区间：`row_len < 16` 触发、不触发两种都要测。
            for w in [1usize, 3, 8] {
                let h = 3usize;
                let (info, mut fb) = scratch(w, h, format, Layout::Padded);
                let pitch = info.pitch;
                let row_visible = w * format.bytes_per_pixel();
                // 整块缓冲预置哨兵：只有被 clear 覆盖的可见区才允许改变。
                for b in fb.iter_mut() {
                    *b = 0xEE;
                }

                let mut renderer = render_on(info, &mut fb);
                renderer.clear(Color::new(1, 2, 3));

                let pad = vec![0xEE; pitch - row_visible];
                for y in 0..h {
                    let off = y * pitch;
                    // 可见区已被 clear 覆盖。
                    assert!(
                        fb[off..off + row_visible].iter().all(|b| *b != 0xEE),
                        "{format:?} / width={w} 第 {y} 行可见区未被 clear 覆盖"
                    );
                    // 行尾填充逐字节保持哨兵。
                    assert_eq!(
                        &fb[off + row_visible..off + pitch],
                        &pad[..],
                        "{format:?} / width={w}（row_len={row_visible}, pitch={pitch}）\
                         第 {y} 行的行填充被 clear 改写了"
                    );
                }
            }
        }
    }

    /// 缓冲**恰好**等于 `pitch × height` 时 `clear` 不越界写（tiny buffer 护栏）。
    ///
    /// `build_pattern` 固定返回 16 字节模式，1×1 的 4bpp 只有 4 字节可用；
    /// `clear` 必须把 `init_len` 收到 `min(pattern_len, total_bytes)`，
    /// 否则会写穿缓冲区。
    #[test]
    fn clear_does_not_write_past_exact_sized_buffer() {
        for format in ALL_FORMATS {
            // 1×1：各种 bpp 下缓冲都远小于 16 字节模式。
            let info = FramebufferInfo::new(1, 1, format.bytes_per_pixel(), format);
            let mut fb = vec![0u8; info.buffer_size()];
            let mut renderer = Renderer::new(info, &mut fb);

            renderer.clear(Color::new(0x12, 0x34, 0x56));

            assert_eq!(
                renderer.buffer(),
                &clear_pixel_ref(format, &Color::new(0x12, 0x34, 0x56))[..],
                "{format:?} 的 1×1 clear 越界或内容错误"
            );
        }
    }

    /// `clear` 时每种格式的 alpha 字节符合各自契约。
    ///
    /// 显式区分两组：存 alpha 的三种 4bpp 写 `color.a`，不存 alpha 的
    /// `Xrgb8888` 写 0。这与 `set_pixel` 的「永远写 `opaque_alpha`」**不同**，
    /// 两条路径都各有测试守护。
    #[test]
    fn clear_alpha_byte_follows_per_format_contract() {
        for (format, pad) in [
            (PixelFormat::Xrgb8888, 0u8),
            (PixelFormat::Argb8888, 0x9A),
            (PixelFormat::Rgba8888, 0x9A),
            (PixelFormat::Bgra8888, 0x9A),
        ] {
            let (w, h) = (4usize, 2usize);
            let (info, mut fb) = scratch(w, h, format, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.clear(Color::with_alpha(0x12, 0x34, 0x56, 0x9A));

            for x in 0..w {
                assert_eq!(
                    fb[x * 4 + 3],
                    pad,
                    "{format:?} 第 {x} 列的 alpha 字节应为 {pad}（clear 保留 color.a）"
                );
            }
        }
    }

    /// 零尺寸帧缓冲（`width`/`height` 为 0）下 `clear` 必须是空操作。
    ///
    /// 守卫 `clear` 开头的 `row_len == 0 || height == 0` 提前返回——
    /// 否则 0 尺寸会走进 `pattern_fill` 并除以零或写出界。
    #[test]
    fn clear_on_zero_sized_framebuffer_is_noop() {
        for (w, h) in [(0usize, 4usize), (4, 0), (0, 0)] {
            let info = FramebufferInfo::new(w, h, 16, PixelFormat::Bgr24);
            let mut fb = [0xEEu8; 16 * 4];
            let mut renderer = Renderer::new(info, &mut fb);
            renderer.clear(Color::new(1, 2, 3));
            assert_eq!(
                renderer.buffer(),
                &[0xEE; 16 * 4][..],
                "{w}×{h} 的 clear 不是空操作"
            );
        }
    }

    // ——————————————————————— S12 · 线段 ———————————————————————

    /// 在固定场景上画一条线，返回被着色像素的坐标集合（行优先序）。
    ///
    /// 用可辨识的颜色（纯白）作为标记色，故「画到的像素」即 `get_pixel` 等于
    /// 该色的像素——透明底上的零值不会混淆判定。
    fn line_pixels(
        renderer: &mut Renderer<'_>,
        w: usize,
        h: usize,
        a: (usize, usize),
        b: (usize, usize),
    ) -> Vec<(usize, usize)> {
        renderer.draw_line(
            Pixel::new(a.0, a.1),
            Pixel::new(b.0, b.1),
            &Color::new(0xFF, 0xFF, 0xFF),
        );
        drawn_pixels(renderer, w, h, &Color::new(0xFF, 0xFF, 0xFF))
    }

    /// 水平 / 垂直 / 45° / 反向端点的覆盖集合必须精确。
    ///
    /// 端点对称性：`(p1,p2)` 与 `(p2,p1)` 必须画出**同一个像素集合**
    /// （顺序可能不同，故按集合比较）。这是 Bresenham 端点排序步骤的契约。
    #[test]
    fn draw_line_covers_expected_pixels_and_is_endpoint_symmetric() {
        // (p1, p2, 期望覆盖集合)
        let cases: [((usize, usize), (usize, usize), &[(usize, usize)]); 5] = [
            // 水平线：覆盖端点之间的每一列。
            ((1, 3), (5, 3), &[(1, 3), (2, 3), (3, 3), (4, 3), (5, 3)]),
            // 垂直线：覆盖端点之间的每一行。
            ((2, 1), (2, 5), &[(2, 1), (2, 2), (2, 3), (2, 4), (2, 5)]),
            // 45°：每个 x 恰��一个 y。
            ((0, 0), (3, 3), &[(0, 0), (1, 1), (2, 2), (3, 3)]),
            // 陡峭斜率（y 跨度 > x 跨度）：x 保持 0，y 逐行前进；末步 x 进到 1。
            ((0, 0), (1, 4), &[(0, 0), (0, 1), (0, 2), (1, 3), (1, 4)]),
            // 平缓斜率（x 跨度 > y 跨度）：y 在第 3 次递推时才进到 1。
            ((0, 0), (4, 1), &[(0, 0), (1, 0), (2, 0), (3, 1), (4, 1)]),
        ];

        for (a, b, expected) in cases {
            let (info, mut fb) = scratch(8, 8, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            assert_eq!(
                line_pixels(&mut renderer, 8, 8, a, b),
                expected.to_vec(),
                "{a:?} -> {b:?} 覆盖集合错误"
            );

            // 反向端点：同一像素集合。
            let (info, mut fb_rev) = scratch(8, 8, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer_rev = render_on(info, &mut fb_rev);
            let forward = line_pixels(&mut renderer, 8, 8, a, b);
            let backward = line_pixels(&mut renderer_rev, 8, 8, b, a);
            assert_eq!(forward, backward, "{a:?} -> {b:?} 的端点反序结果不同");
        }
    }

    /// 退化线段（两端点相同）只画 1 个像素。
    #[test]
    fn draw_line_degenerate_segment_paints_single_pixel() {
        let (info, mut fb) = scratch(8, 8, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        assert_eq!(line_pixels(&mut renderer, 8, 8, (3, 4), (3, 4)), [(3, 4)]);
    }

    /// 区间收窄必须与「从原始端点跑完整 Bresenham 再过滤越界」逐像素一致。
    ///
    /// # 与 TEST_PLAN S12 的偏差（重要）
    ///
    /// 计划条目写「端点超出屏幕：裁剪后覆盖集合 == 把端点钳到边界后重画」。
    /// **该断言在数学上不成立**，已用脚本枚举反例：
    /// 8×8 下 `(5,1) → (9,9)` 收窄后得 6 个像素，而把端点钳到边界
    /// （`(5,1) → (7,7)`）重画得 7 个，5 个样本不一致。
    ///
    /// 原因是 `core.rs:940` 明确禁止把 `clip_line` 的返回值当新端点：
    /// `intersect_axis` 用整数除法截断求交点，端点不在原直线上，斜率一变，
    /// Bresenham 的相位随之改变。实现改为**只收窄迭代区间**、保留原始端点与
    /// 完整相位，因此正确的等价关系是对「完整递推 + 过滤越界」，即本测试。
    #[test]
    fn draw_line_iteration_narrowing_preserves_coverage() {
        let (w, h) = (16usize, 16usize);
        // 覆盖水平、垂直、两方向对角、极陡 / 极缓，以及大量越界终点。
        let pts: [(usize, usize); 12] = [
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

        for &a in &pts {
            for &b in &pts {
                let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let mut renderer = render_on(info, &mut fb);

                let drawn = line_pixels(&mut renderer, w, h, a, b);
                let expected = reference_line_sorted(a, b, w, h);
                assert_eq!(
                    drawn, expected,
                    "{a:?} -> {b:?} 的覆盖与完整 Bresenham 参考不一致"
                );
            }
        }
    }

    /// 越界端点必须被静默裁剪，且不得写内存。
    ///
    /// 只断言「可见部分画对」与「缓冲没被写坏」——不假设具体的裁剪端点，
    /// 裁剪口径由上一条测试的完整递推等价性守护。
    #[test]
    fn draw_line_clips_out_of_screen_endpoints() {
        let (w, h) = (4usize, 4usize);
        let color = Color::new(0x12, 0x34, 0x56);

        // 右边界：水平线越界，只画到最后一列。
        {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_line(Pixel::new(2, 0), Pixel::new(6, 0), &color);
            assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
            assert_eq!(renderer.get_pixel(1, 0), Some(Color::new(0, 0, 0)));
            assert_eq!(renderer.get_pixel(2, 0), Some(color));
            assert_eq!(renderer.get_pixel(3, 0), Some(color));
        }

        // 端点在屏幕下方：陡峭线被裁到屏幕内。
        // 期望值不写死：从完整递推参考推出，4×4 下只有 x=0 那一列被画到。
        {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_line(Pixel::new(0, 0), Pixel::new(1, 8), &color);

            let expected = reference_line_sorted((0, 0), (1, 8), w, h);
            // 非平凡性：该端点对确实覆盖了若干像素。
            assert!(!expected.is_empty(), "参考集为空，断言会失去意义");
            for y in 0..h {
                for x in 0..w {
                    let want = if expected.contains(&(x, y)) {
                        Some(color)
                    } else {
                        Some(Color::new(0, 0, 0))
                    };
                    assert_eq!(renderer.get_pixel(x, y), want, "({x},{y}) 与参考不符");
                }
            }
        }

        // 完全在屏幕外的线段：不画任何像素，且不 panic。
        {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; w * h * 3];
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_line(Pixel::new(50, 50), Pixel::new(100, 100), &color);
            assert_eq!(renderer.buffer(), &blank[..]);
        }
    }

    /// 巨大端点（`usize::MAX` 附近）不得 panic，且**迭代量有界**。
    ///
    /// 守护的是 `11c3e58` 的迭代收窄：未收窄时 `(0,0) → (usize::MAX, usize::MAX)`
    /// 会让主循环跑约 2^63 步，把渲染线程钉死（DoS）。
    ///
    /// 不断言墙钟时间（会随 CI 抖动），改用**像素集合等价**作等价断言：
    /// 收窄后的结果必须等于「饱和坐标下完整递推再过滤越界」的参考集合。
    /// 该参考的主循环长度被 `x_hi_bound` 限制，与线段总长无关。
    #[test]
    fn draw_line_with_huge_endpoints_is_bounded_and_equivalent() {
        let (w, h) = (16usize, 16usize);
        let color = Color::new(0x12, 0x34, 0x56);
        // (p1, p2)：端点取 usize::MAX 量级。
        let cases: [(Pixel, Pixel); 4] = [
            (Pixel::new(0, 0), Pixel::new(usize::MAX, usize::MAX)),
            (Pixel::new(usize::MAX, usize::MAX), Pixel::new(0, 0)),
            (Pixel::new(0, 0), Pixel::new(usize::MAX, 3)),
            (Pixel::new(usize::MAX, 2), Pixel::new(1, 0)),
        ];

        for (p1, p2) in cases {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);

            renderer.draw_line(p1, p2, &color);

            // 参考：从饱和坐标跑**完整**递推再过滤越界。
            // 注意其主循环长度 = min(x2, x_hi_bound) + 1 ≤ max(width, height)，
            // 与 usize::MAX 无关——这正是收窄要达到的复杂度。
            let expected = reference_line_sorted((p1.x, p1.y), (p2.x, p2.y), w, h);

            let drawn = drawn_pixels(&renderer, w, h, &color);
            assert_eq!(
                drawn,
                expected,
                "({:?},{:?}) 的覆盖与饱和坐标下的参考不一致",
                (p1.x, p1.y),
                (p2.x, p2.y)
            );
        }
    }

    /// 私有 `draw_line_unchecked` 在界内必须与 `draw_line` 逐像素一致。
    ///
    /// `draw_line` 只多一步 `clip_line` 快速拒绝与零尺寸守卫；
    /// 内核本身是同一个函数，故界内结果必然相同。
    /// **不测越界**：`draw_line_unchecked` 不做边界检查，越界是调用方 UB。
    #[test]
    fn draw_line_unchecked_matches_safe_path_within_bounds() {
        let (w, h) = (12usize, 10usize);
        let color = Color::new(0x12, 0x34, 0x56);

        let pts: [(usize, usize); 7] = [(0, 0), (11, 0), (0, 9), (11, 9), (5, 5), (1, 8), (8, 2)];

        for &a in &pts {
            for &b in &pts {
                // 两条路径各画一次，比较像素集合。
                let (info, mut fb_safe) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let mut safe = render_on(info, &mut fb_safe);
                safe.draw_line(Pixel::new(a.0, a.1), Pixel::new(b.0, b.1), &color);

                let (info, mut fb_unsafe) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let mut unchecked = render_on(info, &mut fb_unsafe);
                // SAFETY: 测试显式用 `w`/`h` 约束全部端点在界内。
                unsafe {
                    unchecked.draw_line_unchecked(
                        Pixel::new(a.0, a.1),
                        Pixel::new(b.0, b.1),
                        &color,
                    )
                };

                assert_eq!(
                    unchecked.buffer(),
                    safe.buffer(),
                    "{a:?} -> {b:?} 的 unchecked 与安全路径结果不同"
                );
            }
        }
    }

    /// 半透明线段必须与逐像素参考一致，且**重叠像素只混合一次**。
    ///
    /// 守护的是 `draw_line_unchecked` 主循环的「每迭代至多写一个像素」结构：
    /// 半透明色被写两次会累积成更亮的值，而 Bresenham 每步只对应一个像素。
    #[test]
    fn draw_line_alpha_blends_once_per_pixel() {
        let (w, h) = (24usize, 20usize);
        let alpha = 128u8;
        let src = Color::with_alpha(255, 255, 255, alpha);
        let dst = Color::new(0, 0, 0);
        let expect = blend_ref(&src, &dst, alpha);

        let pts: [(usize, usize); 6] = [(0, 0), (23, 0), (0, 19), (23, 19), (2, 17), (21, 3)];

        // 非平凡性在循环外把关：这些端点对的覆盖像素数下限。
        let mut checked = 0usize;

        for &a in &pts {
            for &b in &pts {
                let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let mut renderer = render_on(info, &mut fb);
                // 纯黑底 + 半透明白覆盖：盘内每个像素都应等于单次混合的结果。
                for y in 0..h {
                    for x in 0..w {
                        renderer.set_pixel(x, y, &dst);
                    }
                }

                renderer.draw_line(Pixel::new(a.0, a.1), Pixel::new(b.0, b.1), &src);

                let covered = drawn_pixels(&renderer, w, h, &expect);
                let expected = reference_line_sorted(a, b, w, h);

                assert_eq!(covered, expected, "{a:?} -> {b:?} 的半透明覆盖集合错误");
                checked += covered.len();
            }
        }

        // 画到线上的像素必须是**单次**混合值；若某像素被混合两次，
        // 其值会偏离 `expect`，上面的集合比较即会失败（该像素不在 covered 里）。
        // 这里只确认检查量足够，避免断言在退化样本上空转。
        assert!(
            checked > 500,
            "累计检查的像素过少（{checked}），断言会失去意义"
        );
    }

    // ——————————————————————— S13 · 矩形 ———————————————————————

    /// 在给定场景上执行一次 `fill_rect`，返回被着色像素的坐标集合（行优先）。
    fn fill_rect_pixels(
        renderer: &mut Renderer<'_>,
        w: usize,
        h: usize,
        origin: Pixel,
        rw: usize,
        rh: usize,
        color: &Color,
    ) -> Vec<(usize, usize)> {
        renderer.fill_rect(origin, rw, rh, *color);
        drawn_pixels(renderer, w, h, color)
    }

    /// `fill_rect` 的逐像素参考：矩形与帧缓冲求交后逐点着色。
    ///
    /// 口径对齐实现：`x_max = min(x + width, W)`、`y_max = min(y + height, H)`，
    /// 且尺寸计算用 `saturating_add`（故超大尺寸自动饱和到帧缓冲边界）。
    fn reference_fill_rect(
        origin: Pixel,
        rw: usize,
        rh: usize,
        w: usize,
        h: usize,
    ) -> Vec<(usize, usize)> {
        let x_max = origin.x.saturating_add(rw).min(w);
        let y_max = origin.y.saturating_add(rh).min(h);
        if origin.x >= x_max || origin.y >= y_max {
            return Vec::new();
        }
        let mut out = Vec::new();
        for y in origin.y..y_max {
            for x in origin.x..x_max {
                out.push((x, y));
            }
        }
        out
    }

    /// `fill_rect` 内部精确填充；`width`/`height` 为 0 时不写任何像素。
    #[test]
    fn fill_rect_fills_interior_and_zero_size_writes_nothing() {
        let color = Color::new(0x12, 0x34, 0x56);

        // 正常矩形：逐像素等于参考。
        {
            let (info, mut fb) = scratch(8, 8, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            let got = fill_rect_pixels(&mut renderer, 8, 8, Pixel::new(1, 1), 3, 3, &color);
            assert_eq!(got, reference_fill_rect(Pixel::new(1, 1), 3, 3, 8, 8));
            assert_eq!(got.len(), 9);
        }

        // 零尺寸：完全落空。
        for (rw, rh) in [(0usize, 0usize), (0, 5), (5, 0)] {
            let (info, mut fb) = scratch(8, 8, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; 8 * 8 * 3];
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_rect(Pixel::new(2, 2), rw, rh, color);
            assert_eq!(
                renderer.buffer(),
                &blank[..],
                "fill_rect({rw},{rh}) 写入了像素"
            );
        }
    }

    /// `fill_rect` 越界裁剪到帧缓冲边界，可见部分精确。
    #[test]
    fn fill_rect_clips_to_framebuffer_bounds() {
        let color = Color::new(0x12, 0x34, 0x56);

        // 起点在右下角：只画可见的一小块。
        {
            let (w, h) = (4usize, 4usize);
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            let got = fill_rect_pixels(&mut renderer, w, h, Pixel::new(2, 2), 9, 9, &color);
            assert_eq!(got, reference_fill_rect(Pixel::new(2, 2), 9, 9, w, h));
            assert_eq!(got, [(2, 2), (3, 2), (2, 3), (3, 3)]);
        }

        // 起点完全在屏幕外：什么都不画。
        {
            let (info, mut fb) = scratch(4, 4, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; 4 * 4 * 3];
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_rect(Pixel::new(50, 50), 2, 2, color);
            assert_eq!(renderer.buffer(), &blank[..]);
        }
    }

    /// `fill_rect` 超大尺寸（`usize::MAX`）饱和处理：不 panic、不越界、结果等于整屏。
    ///
    /// 守护的是 `saturating_add` + `min(width)` 两级收敛：
    /// `2 + usize::MAX` 若裸加会回绕成 1，导致画出错误的小矩形。
    #[test]
    fn fill_rect_saturates_huge_dimensions_without_panic() {
        let color = Color::new(0xAA, 0xBB, 0xCC);

        for (ox, oy) in [(0usize, 0usize), (2, 2), (3, 3)] {
            let (w, h) = (4usize, 4usize);
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);

            renderer.fill_rect(Pixel::new(ox, oy), usize::MAX, usize::MAX, color);

            // 期望 = 从 (ox,oy) 到屏幕右/下边界的整块。
            let expected = reference_fill_rect(Pixel::new(ox, oy), usize::MAX, usize::MAX, w, h);
            assert!(!expected.is_empty(), "超大矩形应覆盖到屏幕边缘");
            for y in 0..h {
                for x in 0..w {
                    let want = if expected.contains(&(x, y)) {
                        Some(color)
                    } else {
                        Some(Color::new(0, 0, 0))
                    };
                    assert_eq!(
                        renderer.get_pixel(x, y),
                        want,
                        "起点 ({ox},{oy}) 的超大 fill_rect 在 ({x},{y}) 错误"
                    );
                }
            }
        }
    }

    /// `fill_rect` 在非对齐 `pitch` 下不污染相邻行。
    ///
    /// `pitch = width × bpp + 1` 使第 2 行起的行首不是 4 字节对齐，
    /// 4bpp 的批量内核必须回退到逐像素路径；同时行填充字节不得被写。
    #[test]
    fn fill_rect_does_not_corrupt_neighbouring_rows_on_unaligned_pitch() {
        let w = 8usize;
        let h = 4usize;
        let pitch = w * 4 + 1; // 故意错位：pitch % 4 == 1

        let info = FramebufferInfo::new(w, h, pitch, PixelFormat::Rgba8888);
        let mut fb = vec![0u8; info.buffer_size()];
        for b in fb.iter_mut() {
            *b = 0xEE;
        }
        let color = Color::new(0x12, 0x34, 0x56);
        let mut renderer = Renderer::new(info, &mut fb);

        renderer.fill_rect(Pixel::new(0, 0), w, h, color);

        // 缓冲被 renderer 独占借用，故经由 `renderer.buffer()` 读取。
        let buf = renderer.buffer();
        let row_visible = w * 4;
        let expected_row = [0x12u8, 0x34, 0x56, 0xFF].repeat(w);
        for y in 0..h {
            let off = y * pitch;
            assert_eq!(
                &buf[off..off + row_visible],
                &expected_row[..],
                "pitch={pitch} 第 {y} 行的可见区填充错误"
            );
            // 行尾那 1 字节填充未被写入。
            assert_eq!(
                buf[off + row_visible],
                0xEE,
                "pitch={pitch} 第 {y} 行填充被污染"
            );
        }
        assert_eq!(renderer.get_pixel(0, 0), Some(color));
        assert_eq!(renderer.get_pixel(7, 3), Some(color));
    }

    /// `fill_rect` 的 alpha 混合与逐通道参考一致，且每像素只混合一次。
    ///
    /// 全 `alpha` 值 × 多组 `src`/`dst` 扫描，覆盖跨度填充的全部 alpha 分支
    /// （`a == 0` 跳过 / `a == 255` 直写 / 中间值走混合）。
    #[test]
    fn fill_rect_alpha_blends_once_per_pixel() {
        // 宽度 5：4 像素走批量路径、1 像素走尾部回退，两条分支都要覆盖。
        const W: usize = 5;
        const H: usize = 1;

        let srcs = [
            Color::new(0, 0, 0),
            Color::new(255, 255, 255),
            Color::new(255, 0, 0),
            Color::new(17, 201, 99),
        ];
        let dsts = [
            Color::new(0, 0, 0),
            Color::new(255, 255, 255),
            Color::new(1, 128, 254),
            Color::new(200, 33, 77),
        ];

        let mut checked = 0usize;
        for alpha in 1u8..255 {
            for src in srcs {
                for dst in dsts {
                    let (info, mut fb) = scratch(W, H, PixelFormat::Rgba8888, Layout::Tight);
                    let mut renderer = render_on(info, &mut fb);
                    // 先铺满目标色。
                    for x in 0..W {
                        renderer.set_pixel(x, 0, &dst);
                    }

                    let draw = src.mix_alpha(alpha);
                    renderer.fill_rect(Pixel::new(0, 0), W, 1, draw);

                    let want = blend_ref(&src, &dst, alpha);
                    for x in 0..W {
                        let got = renderer.get_pixel(x, 0).expect("列在范围内");
                        assert_eq!(
                            (got.r, got.g, got.b),
                            (want.r, want.g, want.b),
                            "alpha={alpha} src={src:?} dst={dst:?} 列 {x}"
                        );
                        // 4bpp 混合后第 4 字节必须被强制为 opaque_alpha，
                        // 而不是参与插值（曾因此漏掉一个真实 bug）。
                        assert_eq!(
                            renderer.buffer()[x * 4 + 3],
                            255,
                            "Rgba8888 第 4 字节应为 255（alpha={alpha} 列 {x}）"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 10_000, "累计检查的像素过少（{checked}）");
    }

    /// `fill_rect` 在各 8bpp 格式与 565 上与逐像素参考一致。
    ///
    /// 逐像素参考直接调用 `PixelFormat::write_color`——那是与 `Renderer`
    /// 独立的编码路径，两者必须逐字节一致（不能分叉）。
    #[test]
    fn fill_rect_matches_per_pixel_write_color_reference() {
        for format in ALL_FORMATS {
            for layout in [Layout::Tight, Layout::Padded] {
                let (w, h) = (5usize, 3usize);
                let bpp = format.bytes_per_pixel();
                let (info, mut fb) = scratch(w, h, format, layout);
                // `info` 随后的所有权交给了 renderer，故先取出需要的派生值。
                let pitch = info.pitch;
                let buf_size = info.buffer_size();
                let mut renderer = render_on(info, &mut fb);

                // 目标底色
                let dst = Color::new(60, 120, 180);
                for y in 0..h {
                    for x in 0..w {
                        renderer.set_pixel(x, y, &dst);
                    }
                }
                // 半透明覆盖（触发混合路径，暴露两套编码的分叉）
                let src = Color::with_alpha(200, 30, 90, 77);
                renderer.fill_rect(Pixel::new(0, 0), w, h, src);

                // 逐像素参考：对同一 dst 缓冲重复调用 write_color。
                let mut ref_buf = vec![0u8; buf_size];
                for y in 0..h {
                    for x in 0..w {
                        let off = y * pitch + x * bpp;
                        format.write_color(&mut ref_buf[off..off + bpp], &dst);
                    }
                }
                for y in 0..h {
                    for x in 0..w {
                        let off = y * pitch + x * bpp;
                        let mut px = ref_buf[off..off + bpp].to_vec();
                        format.write_color(&mut px, &src);
                        assert_eq!(
                            &fb[off..off + bpp],
                            &px[..],
                            "{format:?} / {layout:?} 的 ({x},{y}) 与逐像素参考分叉"
                        );
                    }
                }
            }
        }
    }

    /// `draw_rect` 只画四条边，内部像素保持背景。
    ///
    /// 注意 `draw_rect` 的边用 `x2 = x + width`、`y2 = y + height`，
    /// 故轮廓坐标范围是 `[x, x+width] × [y, y+height]`（**闭区间**，比
    /// `fill_rect` 的 `[x, x+width) × [y, y+height)` 多一列一行）。
    #[test]
    fn draw_rect_draws_only_the_four_edges() {
        let (w, h) = (8usize, 8usize);
        let color = Color::new(0xFF, 0x00, 0x00);
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);

        renderer.draw_rect(Pixel::new(1, 1), 3, 3, &color);

        let edges = [
            (1, 1),
            (1, 2),
            (1, 3),
            (1, 4),
            (4, 1),
            (4, 2),
            (4, 3),
            (4, 4),
            (2, 1),
            (3, 1),
            (2, 4),
            (3, 4),
        ];
        for y in 0..h {
            for x in 0..w {
                let want = if edges.contains(&(x, y)) {
                    Some(color)
                } else {
                    Some(Color::new(0, 0, 0))
                };
                assert_eq!(renderer.get_pixel(x, y), want, "({x},{y}) 边/内判定错误");
            }
        }
        // 内部两格必须保持背景。
        assert_eq!(renderer.get_pixel(2, 2), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(0, 0, 0)));
    }

    /// `draw_rect` 越界裁剪：可见部分精确，不 panic、不越界。
    #[test]
    fn draw_rect_clips_out_of_bounds_rectangles() {
        let color = Color::new(0xFF, 0x00, 0x00);

        // 矩形跨越右/下边界：只画可见的边。
        {
            let (w, h) = (5usize, 5usize);
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_rect(Pixel::new(3, 3), 9, 9, &color);

            // 可见像素逐点比对：任何越界写都会污染不该画的像素。
            for y in 0..h {
                for x in 0..w {
                    let got = renderer.get_pixel(x, y).expect("界内");
                    // 参考：四条边各自的 Bresenham 覆盖，裁到屏幕内。
                    let mut expect = false;
                    for (a, b) in [
                        ((3i64, 3i64), (12, 3)),
                        ((12, 3), (12, 12)),
                        ((12, 12), (3, 12)),
                        ((3, 12), (3, 3)),
                    ] {
                        let cov = reference_bresenham(a, b, w as i64, h as i64);
                        if cov.contains(&(x as i64, y as i64)) {
                            expect = true;
                        }
                    }
                    assert_eq!(
                        got,
                        if expect { color } else { Color::new(0, 0, 0) },
                        "({x},{y}) 与四条边的参考覆盖不符"
                    );
                }
            }
        }

        // 起点完全在屏幕外：什么都不画。
        {
            let (info, mut fb) = scratch(5, 5, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; 5 * 5 * 3];
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_rect(Pixel::new(100, 100), 4, 4, &color);
            assert_eq!(renderer.buffer(), &blank[..]);
        }
    }

    // ——————————————————————— S14 · 三角形 ———————————————————————

    /// `fill_triangle` 的逐扫描行参考：按实现口径拆分上下两半。
    ///
    /// 返回 `Vec<(y, x_lo, x_hi)>`，已按行优先排序。`x_lo`/`x_hi` 未裁剪，
    /// 由调用方与帧缓冲求交。
    ///
    /// # 重要：y2 行会被发射两次
    ///
    /// 实现对 `y in y1..=y2` 与 `y in y2..=y3` 各扫一遍，故当 `y1 == y2`
    /// （顶边水平）时第 `y2` 行出现**两次**。这对不透明色无害（写两次同值），
    /// 但半透明色会被混合两次——故 `fill_triangle_alpha_*` 两条测试据此断言，
    /// 而不是笼统地要求「只混合一次」。
    fn reference_triangle_spans(p: [Pixel; 3]) -> Vec<(usize, i64, i64)> {
        let mut pts = p;
        // 按 Y 排序（与实现的三次 swap 等价）。
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }
        if pts[1].y > pts[2].y {
            pts.swap(1, 2);
        }
        if pts[0].y > pts[1].y {
            pts.swap(0, 1);
        }
        let (x1, y1) = (cast_coord(pts[0].x), cast_coord(pts[0].y));
        let (x2, y2) = (cast_coord(pts[1].x), cast_coord(pts[1].y));
        let (x3, y3) = (cast_coord(pts[2].x), cast_coord(pts[2].y));
        if y1 == y3 {
            return Vec::new();
        }

        // 扫描线插值：与实现的 `interpolate_scan_x` 同口径（i64 向零截断）。
        let scan = |base: i64, dx: i64, dy: i64, off: i64| -> i64 {
            if dy == 0 {
                return base;
            }
            base + (dx.saturating_mul(off) + dy / 2) / dy
        };

        let long_dx = x3 - x1;
        let long_dy = y3 - y1;
        let upper_dx = x2 - x1;
        let upper_dy = y2 - y1;
        let mut out = Vec::new();
        for y in y1..=y2 {
            let dy = y - y1;
            let a = scan(x1, long_dx, long_dy, dy);
            let b = scan(x1, upper_dx, upper_dy, dy);
            out.push((y as usize, a.min(b), a.max(b)));
        }
        let lower_dx = x3 - x2;
        let lower_dy = y3 - y2;
        for y in y2..=y3 {
            let a = scan(x1, long_dx, long_dy, y - y1);
            let b = scan(x2, lower_dx, lower_dy, y - y2);
            out.push((y as usize, a.min(b), a.max(b)));
        }
        out
    }

    /// 把参考跨度裁到帧缓冲并展开成像素集合（行优先）。
    ///
    /// 结果**排序并去重**：实现对 `y2` 行发射两次（见上），但重复写同一像素
    /// 对像素**集合**没有影响，故比较集合时必须去重。半透明下的双写效果
    /// 由 `fill_triangle_alpha_...` 单独断言。
    fn triangle_pixels_ref(p: [Pixel; 3], w: usize, h: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (y, lo, hi) in reference_triangle_spans(p) {
            if y >= h {
                continue;
            }
            let xs = lo.max(0).min(hi);
            let xe = hi.max(0).min(w as i64 - 1);
            for x in xs..=xe {
                if x >= 0 {
                    out.push((x as usize, y));
                }
            }
        }
        // 行优先（先 y 后 x），与 `drawn_pixels` 的产出顺序一致。
        out.sort_by_key(|(x, y)| (*y, *x));
        out.dedup();
        out
    }

    /// `fill_triangle` 内部填充精确，且两种绕向填出**同一**像素集合。
    #[test]
    fn fill_triangle_matches_reference_and_is_winding_invariant() {
        let color = Color::new(0x12, 0x34, 0x56);
        let (w, h) = (10usize, 10usize);

        // 覆盖：水平顶边（y1==y2，触发 y2 行双写）、两个顶点同高、
        // 纯竖直边、锐角与钝角。
        let tris: [[Pixel; 3]; 6] = [
            [Pixel::new(0, 0), Pixel::new(8, 0), Pixel::new(2, 5)],
            [Pixel::new(0, 0), Pixel::new(2, 5), Pixel::new(8, 0)],
            [Pixel::new(1, 2), Pixel::new(7, 2), Pixel::new(4, 7)],
            [Pixel::new(0, 0), Pixel::new(0, 6), Pixel::new(5, 6)],
            [Pixel::new(0, 0), Pixel::new(9, 9), Pixel::new(0, 9)],
            [Pixel::new(2, 1), Pixel::new(8, 3), Pixel::new(3, 8)],
        ];

        for tri in tris {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_triangle(tri[0], tri[1], tri[2], &color);

            let drawn = drawn_pixels(&renderer, w, h, &color);
            let expected = triangle_pixels_ref(tri, w, h);
            assert_eq!(drawn, expected, "三角形 {tri:?} 的填充与参考不一致");

            // 反绕向（交换后两个顶点）必须给出同一像素集合。
            let flipped = [tri[0], tri[2], tri[1]];
            let (info, mut fb_rev) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer_rev = render_on(info, &mut fb_rev);
            renderer_rev.fill_triangle(flipped[0], flipped[1], flipped[2], &color);
            let drawn_rev = drawn_pixels(&renderer_rev, w, h, &color);
            assert_eq!(drawn, drawn_rev, "三角形 {tri:?} 的两种绕向填充结果不同");
            assert!(!drawn.is_empty(), "三角形 {tri:?} 未覆盖任何像素");
        }
    }

    /// 退化三角形（三点共线 / 两点重合 / 三点同高）不 panic。
    ///
    /// 三点 Y 相同即被 `y1 == y3` 提前返回；共线但 Y 不同会退化成零宽跨度，
    /// 仍需保证不 panic 且只画出那条线本身。
    #[test]
    fn fill_triangle_degenerate_cases_do_not_panic() {
        let color = Color::new(0x12, 0x34, 0x56);
        let (w, h) = (8usize, 8usize);

        // 三点 Y 相同：提前返回，一个像素都不画。
        {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; w * h * 3];
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_triangle(Pixel::new(0, 3), Pixel::new(3, 3), Pixel::new(7, 3), &color);
            assert_eq!(renderer.buffer(), &blank[..], "三点同高不应画任何像素");
        }

        // 两点重合：退化成一条线段。
        {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_triangle(Pixel::new(0, 0), Pixel::new(0, 0), Pixel::new(5, 5), &color);
            // 期望 == 退化成 (0,0)→(5,5) 的单条线。
            let expected = reference_line_sorted((0, 0), (5, 5), w, h);
            assert_eq!(drawn_pixels(&renderer, w, h, &color), expected);
        }

        // 三点共线（不在同一水平线）：不 panic 即可，覆盖集与零宽跨度一致。
        {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_triangle(Pixel::new(0, 0), Pixel::new(2, 2), Pixel::new(4, 4), &color);
            let expected =
                triangle_pixels_ref([Pixel::new(0, 0), Pixel::new(2, 2), Pixel::new(4, 4)], w, h);
            assert_eq!(drawn_pixels(&renderer, w, h, &color), expected);
        }
    }

    /// 三角形与 `draw_triangle` 轮廓：只画三条边，内部保持背景。
    #[test]
    fn draw_triangle_draws_only_three_edges() {
        let color = Color::new(0xFF, 0x00, 0x00);
        let (w, h) = (8usize, 8usize);
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);

        let tri = [Pixel::new(0, 0), Pixel::new(6, 0), Pixel::new(3, 5)];
        renderer.draw_triangle(tri[0], tri[1], tri[2], &color);

        // 参考：三条边各自的 Bresenham 覆盖的并集。
        let mut expect: Vec<(usize, usize)> = Vec::new();
        for (a, b) in [(0usize, 1usize), (1, 2), (2, 0)] {
            expect.extend(reference_line_sorted(
                (tri[a].x, tri[a].y),
                (tri[b].x, tri[b].y),
                w,
                h,
            ));
        }
        // 三条边在公共顶点处重合，去重后按行优先排序以对齐 `drawn_pixels`。
        expect.sort_by_key(|(x, y)| (*y, *x));
        expect.dedup();

        assert_eq!(drawn_pixels(&renderer, w, h, &color), expect);

        // 内部点必须保持背景。
        let interior = drawn_pixels(&renderer, w, h, &color);
        let centroid = (3, 1);
        assert!(
            !interior.contains(&centroid),
            "三角形内部 ({},{}) 不应被描边",
            centroid.0,
            centroid.1
        );
    }

    /// `draw_triangle` 在两点重合时提前返回，不画任何像素。
    #[test]
    fn draw_triangle_with_coincident_vertices_is_noop() {
        let color = Color::new(0xFF, 0x00, 0x00);
        let (w, h) = (8usize, 8usize);
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let blank = vec![0u8; w * h * 3];
        let mut renderer = render_on(info, &mut fb);

        renderer.draw_triangle(Pixel::new(1, 1), Pixel::new(1, 1), Pixel::new(5, 5), &color);

        assert_eq!(renderer.buffer(), &blank[..]);
    }

    /// 大 `usize` 顶点不因坐标收窄而错位（饱和转换护栏）。
    ///
    /// 守护的是 `usize::MAX as i64 == -1` 陷阱：若用 `as` 转换，
    /// 巨大顶点会折成负数，`p.x.min(max_x)` 之前就已被破坏，几何完全错位。
    /// 这里断言：把顶点钳到屏幕边界后重画，结果与原调用一致。
    #[test]
    fn fill_triangle_clamps_huge_vertices_without_narrowing_bug() {
        let color = Color::new(0x12, 0x34, 0x56);
        let (w, h) = (8usize, 8usize);

        let huge = [
            [
                Pixel::new(0, 0),
                Pixel::new(usize::MAX, 0),
                Pixel::new(usize::MAX, usize::MAX),
            ],
            [
                Pixel::new(usize::MAX, usize::MAX),
                Pixel::new(0, 0),
                Pixel::new(usize::MAX, 0),
            ],
            [
                Pixel::new(1, 1),
                Pixel::new(usize::MAX - 1, 3),
                Pixel::new(4, usize::MAX),
            ],
            [
                Pixel::new(usize::MAX, 0),
                Pixel::new(usize::MAX, usize::MAX),
                Pixel::new(0, 7),
            ],
        ];

        for tri in huge {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_triangle(tri[0], tri[1], tri[2], &color);

            // 期望：顶点先被 `min(max_x/max_y)` 钳到屏幕内，再走同一算法。
            let clamped = [
                Pixel::new(tri[0].x.min(w - 1), tri[0].y.min(h - 1)),
                Pixel::new(tri[1].x.min(w - 1), tri[1].y.min(h - 1)),
                Pixel::new(tri[2].x.min(w - 1), tri[2].y.min(h - 1)),
            ];
            assert_eq!(
                drawn_pixels(&renderer, w, h, &color),
                triangle_pixels_ref(clamped, w, h),
                "巨大顶点三角形 {tri:?} 的结果与钳位后重画不一致"
            );
        }
    }

    /// 越界三角形裁剪：可见像素精确。
    #[test]
    fn fill_triangle_clips_to_visible_pixels() {
        let color = Color::new(0x12, 0x34, 0x56);
        let (w, h) = (6usize, 6usize);

        // 大部分顶点在屏幕外的三角形。
        let tri = [Pixel::new(20, 0), Pixel::new(40, 1), Pixel::new(30, 30)];
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        renderer.fill_triangle(tri[0], tri[1], tri[2], &color);

        // 参考：先把顶点钳到屏幕内再展开（实现的口径）。
        let clamped = [
            Pixel::new(tri[0].x.min(w - 1), tri[0].y.min(h - 1)),
            Pixel::new(tri[1].x.min(w - 1), tri[1].y.min(h - 1)),
            Pixel::new(tri[2].x.min(w - 1), tri[2].y.min(h - 1)),
        ];
        assert_eq!(
            drawn_pixels(&renderer, w, h, &color),
            triangle_pixels_ref(clamped, w, h)
        );

        // 三个顶点都在同一行（钳位后 y 相同）：提前返回，不画任何像素。
        {
            let (info, mut fb2) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; w * h * 3];
            let mut r2 = render_on(info, &mut fb2);
            r2.fill_triangle(
                Pixel::new(9, 5),
                Pixel::new(20, 5),
                Pixel::new(30, 5),
                &color,
            );
            assert_eq!(r2.buffer(), &blank[..]);
        }
    }

    /// 半透明三角形与逐像素参考一致。
    ///
    /// # 与 TEST_PLAN S14 的偏差
    ///
    /// 计划条目写「重叠像素只混合一次」，这对 `fill_triangle` **不成立**。
    /// 实现对 `y in y1..=y2` 与 `y in y2..=y3` 各扫一遍，故 `y1 == y2`
    /// （水平顶边）时第 `y2` 行被**发射两次**；两个跨度都覆盖到的像素
    /// 于是被混合两次。已用脚本核实精确范围：
    ///
    /// | 三角形 | 被写 2 次的像素 | 被写 1 次的像素 |
    /// |--------|-----------------|----------------|
    /// | `(0,0),(8,0),(2,5)` | 仅 `(0,0)` | 33 |
    /// | `(1,2),(7,2),(4,7)` | 仅 `(1,2)` | 27 |
    /// | `(0,0),(9,9),(0,9)` | `y=9` 整行 10 个 | 45 |
    ///
    /// 可见「被写两次」的是**两跨度交集**，不是整行——第三例里 y2 行只被
    /// 部分覆盖，故只有交集像素才双写。
    ///
    /// 本测试因此断言：每个被画到的像素都等于 `reference_triangle_write_counts`
    /// 给出的**精确次数**的混合结果（1 次或 2 次）。这比笼统的「只混合一次」
    /// 更强：实现若改变任一像素的写入次数，断言即失败。
    #[test]
    fn fill_triangle_alpha_matches_reference_with_documented_y2_double_write() {
        let dst = Color::new(0, 0, 0);
        let src = Color::with_alpha(255, 255, 255, 128);
        let (w, h) = (10usize, 10usize);
        let once = blend_ref(&src, &dst, src.a);
        let twice = blend_ref(&src, &once, src.a);
        assert_ne!(once, twice, "单次与两次混合的结果必须可区分");

        let tris: [[Pixel; 3]; 4] = [
            [Pixel::new(0, 0), Pixel::new(8, 0), Pixel::new(2, 5)],
            [Pixel::new(0, 0), Pixel::new(2, 5), Pixel::new(8, 0)],
            [Pixel::new(1, 2), Pixel::new(7, 2), Pixel::new(4, 7)],
            [Pixel::new(0, 0), Pixel::new(9, 9), Pixel::new(0, 9)],
        ];

        let mut total_doubled = 0usize;
        let mut total_once = 0usize;

        for tri in tris {
            // 参考：逐像素的**写入次数**（保留重复，不去重）。
            let counts = reference_triangle_write_counts(tri, w, h);
            assert!(!counts.is_empty(), "三角形 {tri:?} 的参考集为空");
            // 非平凡性：至少要包含一个被写两次的像素，否则本测试退化。
            assert!(
                counts.values().any(|c| *c == 2),
                "三角形 {tri:?} 没有被写两次的像素，覆盖不到 y2 双写语义"
            );

            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            for y in 0..h {
                for x in 0..w {
                    renderer.set_pixel(x, y, &dst);
                }
            }
            renderer.fill_triangle(tri[0], tri[1], tri[2], &src);

            for y in 0..h {
                for x in 0..w {
                    let times = counts.get(&(x, y)).copied().unwrap_or(0);
                    let got = renderer.get_pixel(x, y).expect("界内");
                    if times == 0 {
                        assert_eq!(got, dst, "三角形 {tri:?} 的 ({x},{y}) 不应被画到");
                        continue;
                    }
                    assert!(
                        times <= 2,
                        "三角形 {tri:?} 的 ({x},{y}) 被写 {times} 次，\
                         说明出现了计划外的三重覆盖"
                    );
                    let want = if times == 1 { once } else { twice };
                    assert_eq!(
                        got, want,
                        "三角形 {tri:?} 的 ({x},{y})：参考写入 {times} 次，\
                         期望 {want:?}，实际 {got:?}"
                    );
                    if times == 2 {
                        total_doubled += 1
                    } else {
                        total_once += 1
                    }
                }
            }
        }

        // 非平凡性把关：两种情形都要覆盖到。
        assert!(total_once > 100, "单次混合的像素过少（{total_once}）");
        assert!(total_doubled > 0, "双写像素一个都没覆盖到，断言失去意义");
    }

    // ——————————————————————— S15 · 圆 ———————————————————————

    /// 在给定场景上画一个圆，返回被着色像素的坐标集合（行优先）。
    fn circle_pixels(
        renderer: &mut Renderer<'_>,
        w: usize,
        h: usize,
        center: Pixel,
        radius: usize,
        kind: CircleKind,
        color: &Color,
    ) -> Vec<(usize, usize)> {
        match kind {
            CircleKind::Outline => renderer.draw_circle(center, radius, color),
            CircleKind::Filled => renderer.fill_circle(center, radius, *color),
        }
        drawn_pixels(renderer, w, h, color)
    }

    /// 圆测试场景 `(width, height, center_x, center_y)`：
    /// 覆盖圆心居中、贴边、贴角与奇数尺寸，避免只在一个对称场景上通过。
    const CIRCLE_SCENES: [(usize, usize, usize, usize); 4] =
        [(9, 9, 4, 4), (11, 7, 2, 5), (8, 8, 0, 0), (13, 13, 6, 6)];

    /// 在给定场景下渲染一个圆并返回被着色像素集合。
    fn render_circle_pixels(
        scene: (usize, usize, usize, usize),
        radius: usize,
        kind: CircleKind,
    ) -> Vec<(usize, usize)> {
        let (w, h, cx, cy) = scene;
        let color = Color::new(0x12, 0x34, 0x56);
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        circle_pixels(
            &mut renderer,
            w,
            h,
            Pixel::new(cx, cy),
            radius,
            kind,
            &color,
        )
    }

    /// `fill_circle` 必须逐像素等于精确圆盘 `dx² + dy² <= radius²`。
    ///
    /// 这是填充语义的基准：实现不再复用中点圆递推的八对称跨度（后者在极点
    /// 附近会把 `cy ± radius` 画成一条宽横线，半径 10 时面积偏大 10%）。
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
        assert!(
            checked > 2000,
            "参考圆盘覆盖的像素过少（{checked}），断言失去意义"
        );
    }

    /// `draw_circle` 必须逐像素等于精确圆盘边界（盘中且有 4 邻域在盘外）。
    ///
    /// 该定义同时保证轮廓单像素宽、8-连通，且不会画到圆盘之外。
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
        assert!(
            checked > 300,
            "参考边界覆盖的像素过少（{checked}），断言失去意义"
        );
    }

    /// `draw_circle` 的像素必须是 `fill_circle` 的子集，且不含内部像素。
    ///
    /// 契约：轮廓恰好是填充的最外圈——`draw_circle` 画出的每个像素都必然
    /// 也出现在同参数的 `fill_circle` 结果里。
    #[test]
    fn draw_circle_is_subset_of_fill_circle_and_has_no_interior() {
        for scene in CIRCLE_SCENES {
            for radius in 1..=8usize {
                let filled = render_circle_pixels(scene, radius, CircleKind::Filled);
                let outline = render_circle_pixels(scene, radius, CircleKind::Outline);

                for p in &outline {
                    assert!(
                        filled.contains(p),
                        "场景 {scene:?} 半径 {radius}：轮廓点 {p:?} 不在填充结果内"
                    );
                }
                // 半径足够大时轮廓应当有「内部」概念：轮廓点不能整圆都是内部。
                if radius >= 4 {
                    let disk = reference_disk_pixels(
                        scene.0,
                        scene.1,
                        Pixel::new(scene.2, scene.3),
                        radius,
                    );
                    // 存在既非轮廓、又在盘内的像素（即真正的内部像素）。
                    let has_interior = disk.iter().any(|p| !outline.contains(p));
                    assert!(
                        has_interior,
                        "场景 {scene:?} 半径 {radius} 没有任何内部像素，断言失去意义"
                    );
                }
            }
        }
    }

    /// 半径 0 的语义按代码契约断言：`fill_circle` 直接返回不画，`draw_circle` 同样。
    ///
    /// 注意这与「精确圆盘 `dx²+dy² <= 0` 只有圆心」**不同**——实现把
    /// `radius == 0` 当作退化输入提前返回（守卫在 `core.rs:1368` / `:1392`），
    /// 所以圆心不会被画到。这里显式断言该契约，防止有人「顺手修正」成画圆心。
    #[test]
    fn circle_radius_zero_paints_nothing() {
        for kind in [CircleKind::Outline, CircleKind::Filled] {
            let (w, h) = (7usize, 7usize);
            let color = Color::new(0x12, 0x34, 0x56);
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; w * h * 3];
            let mut renderer = render_on(info, &mut fb);

            let got = circle_pixels(&mut renderer, w, h, Pixel::new(3, 3), 0, kind, &color);

            assert_eq!(
                got,
                Vec::<(usize, usize)>::new(),
                "{kind:?} 半径 0 画了像素"
            );
            assert_eq!(renderer.buffer(), &blank[..]);
        }
    }

    /// 半透明 `fill_circle` 必须对每个像素**只混合一次**。
    ///
    /// 守护的是 `33f951b`：旧的八对称展开在 `x == 0`（4 个基点像素）与
    /// `x == y`（4 个对角像素）处会对同一像素写两次，半透明色因此加深
    /// （alpha=128 得 191 而非 128）。
    ///
    /// 断言方式：纯黑底 + 半透明白覆盖整个圆，圆内**每个**像素都应等于
    /// 单次混合的结果。任何一次重复写都会让该像素变亮而失败。
    #[test]
    fn fill_circle_alpha_blends_once_per_disk_pixel() {
        let (w, h) = (41usize, 41usize);
        let center = Pixel::new(20, 20);
        let radius = 18usize;
        let alpha = 128u8;
        let src = Color::with_alpha(255, 255, 255, alpha);
        let dst = Color::new(0, 0, 0);

        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        renderer.clear(dst);
        renderer.fill_circle(center, radius, src);

        // 单次混合的期望值。
        let expect = blend_ref(&src, &dst, alpha);

        let mut checked = 0usize;
        for y in 0..h {
            for x in 0..w {
                let dx = x as i64 - center.x as i64;
                let dy = y as i64 - center.y as i64;
                if dx * dx + dy * dy > (radius as i64) * (radius as i64) {
                    continue;
                }
                let got = renderer.get_pixel(x, y).expect("圆内像素应可读");
                assert_eq!(
                    got, expect,
                    "像素 ({x},{y}) 被混合了多次：期望 {expect:?}，实际 {got:?}"
                );
                checked += 1;
            }
        }
        assert!(checked > 900, "检查的像素过少（{checked}），断言失去意义");
    }

    /// 半透明 `draw_circle` 必须对每个轮廓像素**只混合一次**。
    ///
    /// 旧的八对称展开在 4 个基点像素上必然写两次。半径 11、18、20 的递推里
    /// `x == y` 的成立情况不同，三者合起来覆盖两类重合位置。
    #[test]
    fn draw_circle_alpha_blends_once_per_outline_pixel() {
        let (w, h) = (61usize, 61usize);
        let center = Pixel::new(30, 30);
        let alpha = 128u8;
        let src = Color::with_alpha(255, 255, 255, alpha);
        let dst = Color::new(0, 0, 0);
        let expect = blend_ref(&src, &dst, alpha);

        for radius in [11usize, 18, 20] {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.clear(dst);
            renderer.draw_circle(center, radius, &src);

            let mut painted = 0usize;
            for y in 0..h {
                for x in 0..w {
                    let got = renderer.get_pixel(x, y).expect("像素应可读");
                    if got == dst {
                        continue;
                    }
                    painted += 1;
                    assert_eq!(
                        got, expect,
                        "半径 {radius} 的轮廓像素 ({x},{y}) 被混合了多次"
                    );
                }
            }
            assert!(painted > 40, "半径 {radius} 的轮廓像素过少（{painted}）");

            // 4 个基点像素在旧实现里必然被写两次，显式断言它们被画到，
            // 以免测试在「轮廓恰好没覆盖到重合位置」时空转。
            for (x, y) in [
                (center.x + radius, center.y),
                (center.x - radius, center.y),
                (center.x, center.y + radius),
                (center.x, center.y - radius),
            ] {
                assert_eq!(
                    renderer.get_pixel(x, y),
                    Some(expect),
                    "半径 {radius} 的基点像素 ({x},{y}) 未按单次混合写入"
                );
            }
        }
    }

    /// 半径钳制：超过上界的半径结果必须等于恰好等于上界的半径（`11c3e58`）。
    ///
    /// 依据：任意可见像素到圆心的距离不超过画布对角线，而
    /// `sqrt(w²+h²) < w+h`，故半径超过该上界时可见像素的结果完全一致。
    ///
    /// 本测试同时守住两件事，缺任何一条都失去意义：
    ///
    /// 1. 等价性——超大半径与上界半径输出逐像素相同（钳制生效）；
    /// 2. 不挂死——未钳制时 `1_000_000_000` 会让 `disk_half_at` 的收缩循环
    ///    迭代约十亿次。不断言墙钟时间，而用「与上界半径等价」作等价断言。
    #[test]
    fn circle_clamps_huge_radius_to_canvas_diagonal() {
        for scene in CIRCLE_SCENES {
            let (w, h) = (scene.0, scene.1);
            let limit = w + h;
            // 上界半径必须覆盖整屏，否则等价性断言两边都是空集、会平凡通过。
            let at_limit = render_circle_pixels(scene, limit, CircleKind::Filled);
            assert_eq!(
                at_limit.len(),
                w * h,
                "场景 {scene:?}：上界半径 {limit} 未覆盖整屏，钳制上界过紧"
            );

            // 覆盖：`usize::MAX`（`as i64` 回绕成 -1 的陷阱）、远超界、刚过界。
            let huge_radii = [usize::MAX, 1_000_000_000, 10 * limit, limit + 1];
            for huge in huge_radii {
                for kind in [CircleKind::Outline, CircleKind::Filled] {
                    let clamped = render_circle_pixels(scene, limit, kind);
                    let big = render_circle_pixels(scene, huge, kind);
                    assert_eq!(
                        big, clamped,
                        "场景 {scene:?} {kind:?}：半径 {huge} 未被钳到 {limit}"
                    );
                }
            }
        }
    }

    /// 钳制不得过度：`radius <= width + height` 时钳制必须是恒等映射。
    ///
    /// 只与另一个半径互比守不住这一点（循环论证：钳到 1 时任意两半径都相等），
    /// 故锚定各半径的实测填充像素数。
    #[test]
    fn circle_clamp_preserves_radii_within_limit() {
        for scene in CIRCLE_SCENES {
            let (w, h, cx, cy) = scene;
            let limit = w + h;
            // 半径 1 的精确圆盘像素数由参考实现给出，与被测实现无关。
            for radius in 1..=limit {
                let got = render_circle_pixels(scene, radius, CircleKind::Filled);
                let expected = reference_disk_pixels(w, h, Pixel::new(cx, cy), radius);
                assert_eq!(
                    got, expected,
                    "场景 {scene:?} 半径 {radius} 的像素数被钳制改变"
                );
            }
            // 上界半径必须覆盖整屏。
            assert_eq!(
                render_circle_pixels(scene, limit, CircleKind::Filled).len(),
                w * h,
                "场景 {scene:?}：上界半径 {limit} 未覆盖整屏"
            );
        }
    }

    /// 圆心在屏幕外 / 坐标为负中间量：正确裁剪，不 panic、不越界。

    /// 各像素格式下圆盘像素集合相同（格式无关性）。
    ///
    /// 圆的几何遍历与像素格式无关：`walk_disk_rows` 只按 `radius` 与
    /// `info.height` 决定迭代量，格式只影响落盘字节。
    ///
    /// 判定方式按格式分两类，避免颜色量化引入假差异：
    ///
    /// * **可无损读回的格式**（3bpp 与 4bpp）：直接按 `get_pixel == color` 判定；
    /// * **565**（量化到 5/6/5）：不能按颜色判定，改为「与纯背景缓冲逐字节
    ///   比较」——先铺一遍背景色、存下快照，再画圆，统计与快照不同的像素。
    ///   这样与颜色的量化损失无关，只反映「哪些像素被写过」。
    #[test]
    fn fill_circle_pixel_set_is_format_independent() {
        let color = Color::new(0x12, 0x34, 0x56);
        let bg = Color::new(1, 2, 3);
        let (w, h) = (11usize, 9usize);
        let center = Pixel::new(5, 4);

        for radius in 1..=7usize {
            for kind in [CircleKind::Outline, CircleKind::Filled] {
                // 基准：Bgr24（可无损读回颜色）。
                let (info, mut fb_ref) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let mut ref_renderer = render_on(info, &mut fb_ref);
                let reference =
                    circle_pixels(&mut ref_renderer, w, h, center, radius, kind, &color);
                assert!(!reference.is_empty(), "{kind:?} 半径 {radius} 基准集为空");

                for format in ALL_FORMATS {
                    let bpp = format.bytes_per_pixel();
                    let (info, mut fb) = scratch(w, h, format, Layout::Tight);
                    let pitch = info.pitch;
                    let mut renderer = render_on(info, &mut fb);

                    // 先铺背景色，保存「未画圆」时的字节快照。
                    for y in 0..h {
                        for x in 0..w {
                            renderer.set_pixel(x, y, &bg);
                        }
                    }
                    let snapshot = renderer.buffer().to_vec();

                    circle_pixels(&mut renderer, w, h, center, radius, kind, &color);

                    let painted: Vec<(usize, usize)> = match format {
                        // 可无损读回：按颜色判定。
                        PixelFormat::Rgb24
                        | PixelFormat::Bgr24
                        | PixelFormat::Xrgb8888
                        | PixelFormat::Argb8888
                        | PixelFormat::Rgba8888
                        | PixelFormat::Bgra8888 => {
                            let mut out = Vec::new();
                            for y in 0..h {
                                for x in 0..w {
                                    if renderer.get_pixel(x, y) == Some(color) {
                                        out.push((x, y));
                                    }
                                }
                            }
                            out
                        }
                        // 565：按「与快照逐字节不同」判定，绕开量化。
                        PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                            let buf = renderer.buffer();
                            let mut out = Vec::new();
                            for y in 0..h {
                                for x in 0..w {
                                    let off = y * pitch + x * bpp;
                                    if buf[off..off + bpp] != snapshot[off..off + bpp] {
                                        out.push((x, y));
                                    }
                                }
                            }
                            out
                        }
                    };
                    assert_eq!(
                        painted, reference,
                        "{format:?} / {kind:?} 半径 {radius} 的像素集合与 Bgr24 基准不同"
                    );
                }
            }
        }
    }

    // ——————————————————————— S16 · 圆弧与扇形 ———————————————————————

    /// 圆弧/扇形测试的固定场景：一个远离原点的圆心，避免四边裁剪干扰。
    const ARC_CX: usize = 10;
    const ARC_CY: usize = 10;
    const ARC_R: usize = 6;

    /// 圆弧测试场景尺寸。
    const ARC_SIZE: usize = 21;

    /// 在 ARC 场景上执行一次绘制，返回被着色像素集合（行优先）。
    fn arc_pixels(
        renderer: &mut Renderer<'_>,
        start: Angle,
        end: Angle,
        kind: CircleKind,
        color: &Color,
    ) -> Vec<(usize, usize)> {
        let center = Pixel::new(ARC_CX, ARC_CY);
        match kind {
            CircleKind::Outline => renderer.draw_arc(center, ARC_R, start, end, color),
            CircleKind::Filled => renderer.fill_arc(center, ARC_R, start, end, *color),
        }
        drawn_pixels(renderer, ARC_SIZE, ARC_SIZE, color)
    }

    /// 新建 ARC 场景缓冲上的渲染器与底层缓冲（调用方持缓冲）。
    fn arc_scratch() -> (FramebufferInfo, Vec<u8>) {
        scratch(ARC_SIZE, ARC_SIZE, PixelFormat::Bgr24, Layout::Tight)
    }

    /// `fill_arc` 的四分之一只覆盖**目标屏幕象限**（Y 轴向下约定）。
    ///
    /// 坐标系契约：`0°` 指向 `+x`（屏幕右）、`90°` 指向 `+y`（屏幕下），
    /// 故 `0° → 90°` 恰是右下象限。四个象限各测一遍，任何一个搞反都会失败。
    #[test]
    fn fill_arc_quarter_covers_only_the_target_screen_quadrant() {
        let color = Color::new(0x12, 0x34, 0x56);
        // (起始角, 终止角, 期望的符号约束)
        let quarters: [(Angle, Angle, fn(i64, i64) -> bool); 4] = [
            // 右下：dx >= 0 且 dy >= 0
            (Angle::ZERO, Angle::RIGHT_DOWN, |dx, dy| dx >= 0 && dy >= 0),
            // 左下：dx <= 0 且 dy >= 0
            (Angle::RIGHT_DOWN, Angle::DOWN_LEFT, |dx, dy| {
                dx <= 0 && dy >= 0
            }),
            // 左上：dx <= 0 且 dy <= 0
            (Angle::DOWN_LEFT, Angle::LEFT_UP, |dx, dy| {
                dx <= 0 && dy <= 0
            }),
            // 右上：dx >= 0 且 dy <= 0
            (Angle::LEFT_UP, Angle::ZERO, |dx, dy| dx >= 0 && dy <= 0),
        ];

        for (start, end, in_quadrant) in quarters {
            let (info, mut fb) = arc_scratch();
            let mut renderer = render_on(info, &mut fb);
            let pixels = arc_pixels(&mut renderer, start, end, CircleKind::Filled, &color);

            assert!(!pixels.is_empty(), "{start:?}->{end:?} 未覆盖任何像素");
            for (x, y) in &pixels {
                let dx = *x as i64 - ARC_CX as i64;
                let dy = *y as i64 - ARC_CY as i64;
                // 圆心本身无方向，任何扇形都包含它，豁免。
                if dx == 0 && dy == 0 {
                    continue;
                }
                assert!(
                    in_quadrant(dx, dy),
                    "{start:?}->{end:?} 的像素 ({x},{y}) 偏移 ({dx},{dy}) 落在错误象限"
                );
            }
        }
    }

    /// `fill_arc` 半圆恰好填出下半 / 上半。
    ///
    /// `0° → 180°` 是 y ≥ 0 的下半（Y 轴向下），`180° → 360°` 是上半。
    #[test]
    fn fill_arc_semicircle_fills_exactly_one_half() {
        let color = Color::new(0x12, 0x34, 0x56);
        let disk = reference_disk_pixels(ARC_SIZE, ARC_SIZE, Pixel::new(ARC_CX, ARC_CY), ARC_R);
        let center_y = ARC_CY as i64;

        for (start, end, lower) in [
            (Angle::ZERO, Angle::DOWN_LEFT, true),
            (Angle::DOWN_LEFT, Angle::ZERO, false),
        ] {
            let (info, mut fb) = arc_scratch();
            let mut renderer = render_on(info, &mut fb);
            let pixels = arc_pixels(&mut renderer, start, end, CircleKind::Filled, &color);

            let expected: Vec<(usize, usize)> = disk
                .iter()
                .copied()
                .filter(|(_, y)| {
                    let dy = *y as i64 - center_y;
                    if lower { dy >= 0 } else { dy <= 0 }
                })
                .collect();

            assert_eq!(pixels, expected, "{start:?}->{end:?} 的半圆覆盖不符");
            assert!(pixels.len() > ARC_R, "半圆覆盖过少，断言失去意义");
        }
    }

    /// `fill_arc` 跨 `0°` 回绕连续：`350° → 10°` 必须连续且只缺 gap 区域。
    ///
    /// major sweep（`10° → 350°`）恰好是它的补集（圆心除外），
    /// 两者并集 = 整圆、交集 = 空。这条同时验证「回绕」与「major 只缺 gap」。
    #[test]
    fn fill_arc_wraps_across_zero_degrees_and_major_sweep_excludes_gap() {
        let color = Color::new(0x12, 0x34, 0x56);
        let minor_sweep = (Angle::from_degrees(350), Angle::from_degrees(10));
        let major_sweep = (Angle::from_degrees(10), Angle::from_degrees(350));

        let (info, mut fb_minor) = arc_scratch();
        let minor = arc_pixels(
            &mut render_on(info, &mut fb_minor),
            minor_sweep.0,
            minor_sweep.1,
            CircleKind::Filled,
            &color,
        );
        let (info, mut fb_major) = arc_scratch();
        let major = arc_pixels(
            &mut render_on(info, &mut fb_major),
            major_sweep.0,
            major_sweep.1,
            CircleKind::Filled,
            &color,
        );

        let disk = reference_disk_pixels(ARC_SIZE, ARC_SIZE, Pixel::new(ARC_CX, ARC_CY), ARC_R);

        // 两个 sweep 的并集恰为整圆盘（圆心只出现一次，故先并后比）。
        // `disk` 已是行优先；并集按 `(y, x)` 排序后对齐。
        let mut union: Vec<(usize, usize)> = minor.iter().chain(major.iter()).copied().collect();
        union.sort_by_key(|(x, y)| (*y, *x));
        union.dedup();
        assert_eq!(union, disk, "major 与 minor sweep 的并集不是整圆盘");

        // 交集只有圆心（圆心属于任何扇形）。
        let overlap: Vec<(usize, usize)> = minor
            .iter()
            .filter(|p| major.contains(p))
            .copied()
            .collect();
        assert!(
            overlap.len() <= 1 && overlap.first() == Some(&(ARC_CX, ARC_CY)),
            "两个 sweep 的交集应只有圆心，实际 {overlap:?}"
        );

        // minor sweep 覆盖了 +x 方向（0° 附近），major 不含。
        assert!(
            minor.contains(&(ARC_CX + ARC_R, ARC_CY)),
            "跨 0° 的 minor sweep 应包含 0° 方向的基点"
        );
        assert!(
            !major.contains(&(ARC_CX + ARC_R, ARC_CY)),
            "major sweep 不应包含 0° 方向的基点"
        );
        // 反向：major 覆盖 180° 方向。
        assert!(
            major.contains(&(ARC_CX - ARC_R, ARC_CY)),
            "major sweep 应包含 180° 方向的基点"
        );
    }

    /// 全扫角（`start == end`）时 `fill_arc == fill_circle`、`draw_arc == draw_circle`。
    #[test]
    fn full_sweep_arc_matches_fill_and_draw_circle() {
        let color = Color::new(0x12, 0x34, 0x56);
        let center = Pixel::new(ARC_CX, ARC_CY);

        // fill_arc(整圈) == fill_circle
        {
            let (info, mut fb_arc) = arc_scratch();
            let mut arc = render_on(info, &mut fb_arc);
            let via_arc = arc_pixels(
                &mut arc,
                Angle::from_degrees(30),
                Angle::from_degrees(30),
                CircleKind::Filled,
                &color,
            );
            let (info, mut fb_circle) = arc_scratch();
            let mut circle = render_on(info, &mut fb_circle);
            circle.fill_circle(center, ARC_R, color);
            let via_circle = drawn_pixels(&circle, ARC_SIZE, ARC_SIZE, &color);
            assert_eq!(via_arc, via_circle, "整圈 fill_arc 与 fill_circle 不一致");
        }

        // draw_arc(整圈) == draw_circle
        {
            let (info, mut fb_arc) = arc_scratch();
            let mut arc = render_on(info, &mut fb_arc);
            let via_arc = arc_pixels(
                &mut arc,
                Angle::from_degrees(30),
                Angle::from_degrees(30),
                CircleKind::Outline,
                &color,
            );
            let (info, mut fb_circle) = arc_scratch();
            let mut circle = render_on(info, &mut fb_circle);
            circle.draw_circle(center, ARC_R, &color);
            let via_circle = drawn_pixels(&circle, ARC_SIZE, ARC_SIZE, &color);
            assert_eq!(via_arc, via_circle, "整圈 draw_arc 与 draw_circle 不一致");
        }
    }

    /// `draw_arc` 的像素 ⊆ `fill_arc` 的像素，且不含内部像素。
    #[test]
    fn draw_arc_pixels_are_subset_of_fill_arc_and_exclude_interior() {
        let color = Color::new(0x12, 0x34, 0x56);
        let wedges = [
            (Angle::ZERO, Angle::RIGHT_DOWN),
            (Angle::from_degrees(20), Angle::from_degrees(80)),
            (Angle::from_degrees(350), Angle::from_degrees(10)),
            (Angle::from_degrees(100), Angle::from_degrees(260)),
        ];

        for (start, end) in wedges {
            let (info, mut fb_fill) = arc_scratch();
            let filled = arc_pixels(
                &mut render_on(info, &mut fb_fill),
                start,
                end,
                CircleKind::Filled,
                &color,
            );
            let (info, mut fb_draw) = arc_scratch();
            let outlined = arc_pixels(
                &mut render_on(info, &mut fb_draw),
                start,
                end,
                CircleKind::Outline,
                &color,
            );

            for p in &outlined {
                assert!(
                    filled.contains(p),
                    "{start:?}->{end:?} 的轮廓点 {p:?} 不在扇形填充内"
                );
            }
            // 轮廓不应包含圆心（圆心是内部点）。
            assert!(
                !outlined.contains(&(ARC_CX, ARC_CY)),
                "{start:?}->{end:?} 的轮廓包含圆心"
            );
        }
    }

    /// 透明色（`a = 0`）的 `fill_arc` / `draw_arc` 不写任何像素。
    #[test]
    fn transparent_color_arc_writes_nothing() {
        for kind in [CircleKind::Outline, CircleKind::Filled] {
            let ghost = Color::with_alpha(0x12, 0x34, 0x56, 0);
            let (info, mut fb) = arc_scratch();
            let blank = vec![0u8; ARC_SIZE * ARC_SIZE * 3];
            let mut renderer = render_on(info, &mut fb);

            let got = arc_pixels(&mut renderer, Angle::ZERO, Angle::RIGHT_DOWN, kind, &ghost);

            assert_eq!(got, Vec::<(usize, usize)>::new(), "{kind:?} 透明色画了像素");
            assert_eq!(renderer.buffer(), &blank[..]);
        }
    }

    /// 半透明 `fill_arc` 与参考一致，且每个像素**只混合一次**。
    ///
    /// `fill_arc` 逐行只扫一次（`walk_disk_rows` 每行回调一次），故不存在
    /// 圆盘那种「对称跨度重合」导致的重复写。
    #[test]
    fn fill_arc_alpha_blends_once_per_pixel() {
        let dst = Color::new(0, 0, 0);
        let src = Color::with_alpha(255, 255, 255, 128);
        let twice = blend_ref(&src, &blend_ref(&src, &dst, src.a), src.a);
        let once = blend_ref(&src, &dst, src.a);
        assert_ne!(once, twice, "单次与两次混合必须可区分");

        for (start, end) in [
            (Angle::ZERO, Angle::RIGHT_DOWN),
            (Angle::from_degrees(350), Angle::from_degrees(10)),
            (Angle::from_degrees(30), Angle::from_degrees(330)),
        ] {
            let (info, mut fb) = arc_scratch();
            let mut renderer = render_on(info, &mut fb);
            renderer.clear(dst);
            renderer.fill_arc(Pixel::new(ARC_CX, ARC_CY), ARC_R, start, end, src);

            // 逐像素：非背景即被画到，且必须是单次混合值。
            let mut painted = 0usize;
            for y in 0..ARC_SIZE {
                for x in 0..ARC_SIZE {
                    let got = renderer.get_pixel(x, y).expect("界内");
                    if got == dst {
                        continue;
                    }
                    painted += 1;
                    assert_eq!(
                        got, once,
                        "{start:?}->{end:?} 的像素 ({x},{y}) 被混合了多次"
                    );
                }
            }
            // 350°→10° 只有 20° 宽，半径 6 时天然只有 ~7 个像素，
            // 故门槛按「非空 + 至少 5 个」设定，够挡住空转即可。
            assert!(painted >= 5, "{start:?}->{end:?} 覆盖像素过少（{painted}）");
        }
    }

    /// 半径为 0 / 圆心在屏幕外的弧与扇形：静默裁剪，不 panic。
    #[test]
    fn arc_with_zero_radius_or_outside_center_is_clipped() {
        let color = Color::new(0x12, 0x34, 0x56);

        // 半径 0：什么都不画。
        for kind in [CircleKind::Outline, CircleKind::Filled] {
            let (info, mut fb) = arc_scratch();
            let blank = vec![0u8; ARC_SIZE * ARC_SIZE * 3];
            let mut renderer = render_on(info, &mut fb);
            match kind {
                CircleKind::Outline => renderer.draw_arc(
                    Pixel::new(ARC_CX, ARC_CY),
                    0,
                    Angle::ZERO,
                    Angle::RIGHT_DOWN,
                    &color,
                ),
                CircleKind::Filled => renderer.fill_arc(
                    Pixel::new(ARC_CX, ARC_CY),
                    0,
                    Angle::ZERO,
                    Angle::RIGHT_DOWN,
                    color,
                ),
            }
            assert_eq!(renderer.buffer(), &blank[..], "{kind:?} 半径 0 画了像素");
        }

        // 圆心完全在屏幕外：什么都不画（沿 y 越界即可，`fill_arc` 的行检查先丢弃）。
        for (cx, cy) in [(100usize, 100usize), (0, 100), (100, 0)] {
            let (info, mut fb) = arc_scratch();
            let blank = vec![0u8; ARC_SIZE * ARC_SIZE * 3];
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_arc(
                Pixel::new(cx, cy),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                &color,
            );
            renderer.fill_arc(
                Pixel::new(cx, cy),
                ARC_R,
                Angle::ZERO,
                Angle::RIGHT_DOWN,
                color,
            );
            assert_eq!(
                renderer.buffer(),
                &blank[..],
                "圆心 ({cx},{cy}) 在屏幕外却画了像素"
            );
        }
    }

    /// 弧与扇形与大半径：结果等于钳制上界半径，不 DoS。
    ///
    /// 守护的是 `disk_params` 的半径钳制在弧/扇形路径上同样生效。
    /// 不断言墙钟时间，用「与上界半径逐像素等价」作等价断言。
    #[test]
    fn arc_clamps_huge_radius() {
        let color = Color::new(0x12, 0x34, 0x56);
        // 上界 = width + height。
        let limit = ARC_SIZE + ARC_SIZE;

        for (start, end, kind) in [
            (Angle::ZERO, Angle::RIGHT_DOWN, CircleKind::Outline),
            (Angle::ZERO, Angle::RIGHT_DOWN, CircleKind::Filled),
            (
                Angle::from_degrees(350),
                Angle::from_degrees(10),
                CircleKind::Filled,
            ),
        ] {
            let (info, mut fb_limit) = arc_scratch();
            let at_limit = arc_pixels(
                &mut render_on(info, &mut fb_limit),
                start,
                end,
                kind,
                &color,
            );
            assert!(!at_limit.is_empty(), "{kind:?} 上界半径未覆盖任何像素");

            for huge in [usize::MAX, 1_000_000_000, 10 * limit, limit + 1] {
                let (info, mut fb_big) = arc_scratch();
                let big = arc_pixels(&mut render_on(info, &mut fb_big), start, end, kind, &color);
                assert_eq!(big, at_limit, "{kind:?} 半径 {huge} 未被钳到 {limit}");
            }
        }
    }

    /// 圆心在屏幕外 / 贴边时正确裁剪，不 panic、不越界。
    ///
    /// # 回归说明（曾经的 bug）
    ///
    /// `fill_visible_horizontal_span` 早先是「`sx` 只夹下界、`ex` 只夹上界，
    /// 若 `sx > ex` 就交换」。互换不能让越界区间变合法：`fill_circle((100,0), r=3)`
    /// 在 4×4 下会算出 `sx = 3, ex = 97`，随后 `fill_span_opaque` 以
    /// `get_unchecked_mut` **越界写**（debug 下 UB abort，release 下静默写坏内存）。
    ///
    /// **触发条件的关键**：圆心必须**只在 x 方向出屏、y 仍在屏内**。
    /// 圆心 `(100,100)` 这类两侧都出屏的用例会被 `y >= height` 提前丢弃，
    /// 永远碰不到出问题的那条路径——旧测试正是因此漏掉了这个 bug。
    /// 故本测试显式覆盖 `(100, 0)` / `(0, 100)`。
    #[test]
    fn circle_with_center_outside_or_at_edge_clips_correctly() {
        let color = Color::new(0x12, 0x34, 0x56);

        // 圆心在左上角：与负中间量混用，但完全在屏内。
        {
            let (w, h) = (4usize, 4usize);
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_circle(Pixel::new(0, 0), 2, &color);
            assert_eq!(renderer.get_pixel(0, 2), Some(color));
            assert_eq!(renderer.get_pixel(2, 0), Some(color));
            assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(0, 0, 0)));
        }

        // 圆心完全在屏幕外：什么都不画，不 panic、不越界。
        for (cx, cy) in [(100usize, 100usize), (0, 100), (100, 0)] {
            let (w, h) = (4usize, 4usize);
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let blank = vec![0u8; w * h * 3];
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_circle(Pixel::new(cx, cy), 3, &color);
            renderer.fill_circle(Pixel::new(cx, cy), 3, color);
            assert_eq!(
                renderer.buffer(),
                &blank[..],
                "圆心 ({cx},{cy}) 在屏幕外却画了像素"
            );
        }

        // 圆心恰在右下角：对称于左上角的情形。
        {
            let (w, h) = (4usize, 4usize);
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.draw_circle(Pixel::new(3, 3), 2, &color);
            assert_eq!(renderer.get_pixel(1, 3), Some(color));
            assert_eq!(renderer.get_pixel(3, 1), Some(color));
        }
    }

    /// # 回归说明：`fill_visible_horizontal_span` 越界写的三分发路径
    ///
    /// 该函数曾把「各自夹一端 + 反序时互换」误当作安全裁剪，导致「区间整体
    /// 越界」时产出越界索引，由 `fill_span_opaque` 以 `get_unchecked_mut` 写穿。
    /// 三个调用方（`fill_circle` / `fill_arc` / `fill_triangle`）都会踩到，
    /// 且必须「跨度两端同在屏幕外同一侧」才触发。
    ///
    /// 这里对三个调用方统一用 `(100, 0)` —— 圆心/顶点在 x 方向远出屏、y 在屏内，
    /// 正是能穿过 `y` 检查、抵达出错分支的形态。
    #[test]
    fn fill_span_regression_centers_offscreen_in_x_only() {
        let color = Color::new(0x12, 0x34, 0x56);
        let (w, h) = (8usize, 8usize);
        // 每一组都必须画不出东西，且不 panic（越界写由 UB 检查器拦下）。
        let offscreen: [(usize, usize); 3] = [(100, 0), (0, 100), (100, 100)];

        for (cx, cy) in offscreen {
            let blank = vec![0u8; w * h * 3];

            // fill_circle / draw_circle
            {
                let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let mut renderer = render_on(info, &mut fb);
                renderer.fill_circle(Pixel::new(cx, cy), 3, color);
                renderer.draw_circle(Pixel::new(cx, cy), 3, &color);
                assert_eq!(renderer.buffer(), &blank[..], "圆 ({cx},{cy})：圆图元越界");
            }

            // fill_arc / draw_arc
            {
                let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let mut renderer = render_on(info, &mut fb);
                renderer.fill_arc(Pixel::new(cx, cy), 3, Angle::ZERO, Angle::RIGHT_DOWN, color);
                renderer.draw_arc(
                    Pixel::new(cx, cy),
                    3,
                    Angle::ZERO,
                    Angle::RIGHT_DOWN,
                    &color,
                );
                assert_eq!(renderer.buffer(), &blank[..], "圆 ({cx},{cy})：弧图元越界");
            }

            // fill_triangle / draw_triangle
            //
            // 注意语义差异：`fill_triangle` 会把顶点 `min(max_x/max_y)` 钳到屏幕内
            // （core.rs:1145），故出屏顶点被拉回边界后**合法地**画出边缘像素，
            // 不能断言「什么都没画」。这里断言「不越界」：画出的像素必须落在
            // 钳位后三角形的逐扫描线参考内，且缓冲末字节不被写穿。
            {
                let tri = [
                    Pixel::new(cx, cy),
                    Pixel::new(cx + 3, cy),
                    Pixel::new(cx, cy + 3),
                ];
                let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
                let buf_len = info.buffer_size();
                let mut renderer = render_on(info, &mut fb);
                renderer.fill_triangle(tri[0], tri[1], tri[2], &color);
                let painted = drawn_pixels(&renderer, w, h, &color);

                // 钳位到屏幕内的顶点即为实际几何。
                let clamped = [
                    Pixel::new(tri[0].x.min(w - 1), tri[0].y.min(h - 1)),
                    Pixel::new(tri[1].x.min(w - 1), tri[1].y.min(h - 1)),
                    Pixel::new(tri[2].x.min(w - 1), tri[2].y.min(h - 1)),
                ];
                assert_eq!(
                    painted,
                    triangle_pixels_ref(clamped, w, h),
                    "顶点 ({cx},{cy})：三角形填充与钳位后参考不符"
                );
                // 缓冲末尾未被写穿（越界写的直接证据）。
                assert_eq!(fb.len(), buf_len, "缓冲区长度被改动");
            }
        }
    }

    /// # 回归说明：`fill_triangle` 的反序跨度必须照常填充
    ///
    /// `fill_visible_horizontal_span` 的端点顺序契约是「**允许反序**」：
    /// `fill_triangle` 两条扫描线交点的相对大小取决于顶点绕向，**多数**顶点
    /// 顺序下都会出现 `start_x > end_x`（例如 `(10,2),(1,9),(12,12)` 的 12 行
    /// 里有 9 行反序）。修复越界写时若把「反序即丢弃」当成安全裁剪，这些
    /// 三角形会大面积漏填——从「越界写」变成「静默渲染错误」，后者更难发现。
    ///
    /// 本测试断言：`fill_triangle` 在反序绕向下与正序绕向填出**同一像素集合**，
    /// 且两者都等于逐扫描线参考。
    ///
    /// # 用例选取：排除水平底边
    ///
    /// 用例集刻意**不含** `y2 == y3` 的水平底边三角形——那类输入存在**既有的**
    /// 几何不对称（成因见 `fill_triangle_horizontal_bottom_edge_is_winding_asymmetric`），
    /// 与「反序跨度被丢弃」是两回事，混在一起会让本护栏的红绿含义不清。
    #[test]
    fn fill_triangle_fills_reversed_spans_same_as_forward_winding() {
        let color = Color::new(0x12, 0x34, 0x56);
        let (w, h) = (16usize, 16usize);

        // 这些三角形会产生大量反序跨度（实测比例 8/10 ~ 9/12），且无水平底边。
        // 全部用例必须**无水平底边**（即排序后 `y2 != y3`），否则会落进
        // `fill_triangle_horizontal_bottom_edge_is_winding_asymmetric` 记录的既有缺陷。
        let tris: [[Pixel; 3]; 4] = [
            [Pixel::new(10, 2), Pixel::new(1, 9), Pixel::new(12, 12)],
            [Pixel::new(0, 0), Pixel::new(8, 8), Pixel::new(0, 8)],
            [Pixel::new(2, 2), Pixel::new(14, 3), Pixel::new(1, 14)],
            [Pixel::new(1, 2), Pixel::new(7, 2), Pixel::new(4, 7)],
        ];

        for tri in tris {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            renderer.fill_triangle(tri[0], tri[1], tri[2], &color);
            let forward = drawn_pixels(&renderer, w, h, &color);

            // 交换后两个顶点 = 反序绕向。
            let flipped = [tri[0], tri[2], tri[1]];
            let (info, mut fb_rev) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
            let mut rev = render_on(info, &mut fb_rev);
            rev.fill_triangle(flipped[0], flipped[1], flipped[2], &color);
            let backward = drawn_pixels(&rev, w, h, &color);

            assert_eq!(
                forward, backward,
                "三角形 {tri:?} 的反序绕向填充结果不同（反序跨度被丢弃了？）"
            );
            assert_eq!(
                forward,
                triangle_pixels_ref(tri, w, h),
                "三角形 {tri:?} 与逐扫描线参考不符"
            );
            assert!(!forward.is_empty(), "三角形 {tri:?} 未覆盖任何像素");
        }
    }

    /// # 已知缺陷：`fill_triangle` 在水平底边下对顶点绕向不对称
    ///
    /// 本测试**记录当前缺陷现状**，不是认可该行为。修复后它会以明确信息失败，
    /// 届时应把断言改成「两种绕向完全一致」。
    ///
    /// ## 现象
    ///
    /// 三角形 `(8,0), (0,8), (8,8)` 与其反绕向 `(8,0), (8,8), (0,8)` 填出的
    /// 像素集合相差**恰好 1 个像素**（`(0,8)`），即同一个三角形换个顶点顺序
    /// 轮廓就差一个像素。
    ///
    /// ## 成因
    ///
    /// 底边水平，故按 Y 排序后 `y2 == y3`：

    /// | | 按 y 排序后 | `x2` | `lower_dy = y3 - y2` | `interpolate_scan_x` | y=8 跨度 |
    /// |---|---|---|---|---|---|
    /// | 正序 | `(8,0),(0,8),(8,8)` | `(0,8)` | `0` | 命中 `delta_y == 0` 短路 → `return x2 = 0` | `[0, 8]` |
    /// | 反序 | `(8,0),(8,8),(0,8)` | `(8,8)` | `-8` | 正常插值（`off = 0`）→ `8` | `[1, 8]` |
    ///
    /// `interpolate_scan_x` 的短路本身是对的（水平边不该插值）；不对称来自
    /// **三顶点 Y 排序在 `y2 == y3` 时不唯一**——`x2` 是 `lower` 段的 `base_x`，
    /// 而 `x2` 取决于输入顶点顺序。
    ///
    /// ## 与越界写修复的关系
    ///
    /// 已用实验确认这是**既有缺陷**、与 `fill_visible_horizontal_span` 的
    /// 越界写修复无关：把该函数改回修复前版本（`max` / `min` / `swap`），
    /// 本用例产出完全相同的结果。
    ///
    /// ## 正确的修法方向
    ///
    /// `lower` 段在 `y2 == y3` 时应取 `x3`（水平边的另一个端点）而非 `x2`，
    /// 或改用与顶点顺序无关的边选择。等几何语义变更时再动，勿在本次顺手改。
    #[test]
    fn fill_triangle_horizontal_bottom_edge_is_winding_asymmetric() {
        let color = Color::new(0x12, 0x34, 0x56);
        let (w, h) = (16usize, 16usize);
        let tri = [Pixel::new(8, 0), Pixel::new(0, 8), Pixel::new(8, 8)];
        let flipped = [tri[0], tri[2], tri[1]];

        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        renderer.fill_triangle(tri[0], tri[1], tri[2], &color);
        let forward = drawn_pixels(&renderer, w, h, &color);

        let (info, mut fb_rev) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut rev = render_on(info, &mut fb_rev);
        rev.fill_triangle(flipped[0], flipped[1], flipped[2], &color);
        let backward = drawn_pixels(&rev, w, h, &color);

        // 若此断言失败，说明不对称已被修复——请把本测试改写成「两者应当完全一致」。
        assert_ne!(
            forward, backward,
            "水平底边的不对称似乎已被修复：应更新本测试为强断言 \
             （两种绕向应填出同一像素集合）"
        );

        // 差异恰好一个像素，且位于下边界行。
        let only_forward: Vec<(usize, usize)> = forward
            .iter()
            .copied()
            .filter(|p| !backward.contains(p))
            .collect();
        assert_eq!(
            only_forward,
            [(0, 8)],
            "差异应恰为下边界行的 (0,8)，实际 {only_forward:?}"
        );
        // 其余像素完全一致。
        let only_backward: Vec<(usize, usize)> = backward
            .iter()
            .copied()
            .filter(|p| !forward.contains(p))
            .collect();
        assert!(
            only_backward.is_empty(),
            "反序不应多画出像素：{only_backward:?}"
        );
    }

    /// 私有内核 `disk_params` 的半径钳制口径。
    ///
    /// 单独测内核是为了把「钳到 `width + height`」这条契约钉死：
    /// 半径小于上界时钳制是恒等映射，超过时收敛到上界。
    #[test]
    fn disk_params_clamps_radius_to_width_plus_height() {
        let w = 10usize;
        let h = 6usize;
        let limit = (w + h) as i64;

        for layout in [Layout::Tight, Layout::Padded] {
            let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, layout);
            let renderer = render_on(info, &mut fb);

            // 半径 ≤ 上界：恒等。
            for radius in [0usize, 1, 5, limit as usize] {
                let (cx, cy, r) = renderer.disk_params(Pixel::new(3, 2), radius);
                assert_eq!(
                    (cx, cy, r),
                    (3, 2, radius as i64),
                    "{layout:?} 半径 {radius}"
                );
            }
            // 半径 > 上界：收敛到上界。
            for radius in [limit as usize + 1, 1000, 1_000_000_000, usize::MAX] {
                let (_, _, r) = renderer.disk_params(Pixel::new(3, 2), radius);
                assert_eq!(r, limit, "{layout:?} 半径 {radius} 未钳到 {limit}");
            }
            // 圆心走饱和转换：`usize::MAX as i64 == -1` 的陷阱。
            let (cx, cy, _) = renderer.disk_params(Pixel::new(usize::MAX, usize::MAX), 3);
            assert_eq!(
                (cx, cy),
                (i64::MAX, i64::MAX),
                "{layout:?} 圆心未被饱和转换"
            );
        }
    }

    /// 私有内核 `disk_half_at` 的半宽语义：增量收缩等于 `floor(sqrt(r² − k²))`。
    ///
    /// `disk_half_at` 是逐行求精确圆盘半宽的核心，用**状态复用**（`half` 跨行
    /// 递减）代替逐行开方。参考值用整数平方根逐行独立求出，与实现写法无关。
    #[test]
    fn disk_half_at_shrinks_monotonically_to_exact_sqrt() {
        /// 整数平方根（牛顿法），与被测实现的「增量收缩」写法完全不同。
        fn isqrt(n: u128) -> i64 {
            if n == 0 {
                return 0;
            }
            let mut x = (n as f64).sqrt() as i128;
            // 牛顿法向下收敛，避免浮点略高导致多 1。
            while x * x > n as i128 {
                x -= 1;
            }
            while (x + 1) * (x + 1) <= n as i128 {
                x += 1;
            }
            x as i64
        }

        for r in 0i64..=40 {
            let r2 = (r as u128) * (r as u128);
            let mut half = r;
            for k in 0..=r {
                let got = Renderer::disk_half_at(r2, k, &mut half);
                let want = isqrt(r2 - (k as u128) * (k as u128));
                assert_eq!(got, want, "半径 {r} 第 {k} 行的半宽错误");
                // 单调不增：半宽随 |dy| 增大只会收缩。
                assert!(got <= r, "半径 {r} 第 {k} 行半宽超过半径");
            }
            // 半宽必须已收敛到 0（末行是圆盘极点）。
            assert_eq!(half, 0, "半径 {r} 的末行半宽应为 0");
        }
    }

    /// `fill_triangle` 每个像素的**写入次数**参考（保留重复计数，不去重）。
    ///
    /// 实现对 `y1..=y2` 与 `y2..=y3` 各扫一遍，两段都覆盖到的像素会被写两次。
    /// 半透明混合的结果取决于这个次数，故参考必须逐像素记数而非只记集合。
    fn reference_triangle_write_counts(
        tri: [Pixel; 3],
        w: usize,
        h: usize,
    ) -> alloc::collections::BTreeMap<(usize, usize), usize> {
        let mut counts = alloc::collections::BTreeMap::new();
        for (y, lo, hi) in reference_triangle_spans(tri) {
            if y >= h {
                continue;
            }
            let xs = lo.max(0).min(hi);
            let xe = hi.max(0).min(w as i64 - 1);
            for x in xs..=xe {
                if x >= 0 {
                    *counts.entry((x as usize, y)).or_insert(0usize) += 1;
                }
            }
        }
        counts
    }

    // ——————————————————————— S17 · blit / blit_mask ———————————————————————

    /// `blit` 的逐行拷贝参考：与实现同口径（逐行只拷贝 `visible_cols × bpp`）。
    ///
    /// 返回目标缓冲的新副本，便于逐字节比对。
    fn reference_blit(
        fb: &[u8],
        pitch: usize,
        w: usize,
        h: usize,
        dst_x: usize,
        dst_y: usize,
        bpp: usize,
        src: &[u8],
        src_pitch: usize,
        width: usize,
        height: usize,
    ) -> Vec<u8> {
        let mut out = fb.to_vec();
        for row in 0..height {
            let dy = dst_y + row;
            if dy >= h {
                break;
            }
            if dst_x >= w {
                continue;
            }
            let visible_cols = width.min(w - dst_x);
            let copy_bytes = visible_cols * bpp;
            let src_start = row * src_pitch;
            let dst_offset = dy * pitch + dst_x * bpp;
            out[dst_offset..dst_offset + copy_bytes]
                .copy_from_slice(&src[src_start..src_start + copy_bytes]);
        }
        out
    }

    /// 构造一块可辨识的源像素块（每像素 R = 10 + y、G = 20 + x、B = 30、A = 40）。
    fn make_src_bgr24(w: usize, h: usize, src_pitch: usize) -> Vec<u8> {
        let mut src = vec![0u8; src_pitch * h];
        for y in 0..h {
            for x in 0..w {
                let off = y * src_pitch + x * 3;
                src[off] = 20 + x as u8; // B
                src[off + 1] = 30; // G
                src[off + 2] = 10 + y as u8; // R
            }
        }
        src
    }

    /// `blit` 同格式拷贝源像素到目标（含偏移与裁剪）。
    #[test]
    fn blit_copies_source_pixels_with_offset_and_clipping() {
        let (w, h) = (8usize, 8usize);
        let bpp = PixelFormat::Bgr24.bytes_per_pixel();
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let pitch = info.pitch;

        // 把 4×2 的源块 blit 到 (1,1)。
        let (sw, sh) = (4usize, 2usize);
        let src_pitch = sw * bpp;
        let src = make_src_bgr24(sw, sh, src_pitch);

        {
            let mut renderer = render_on(info, &mut fb);
            renderer.blit(
                Pixel::new(1, 1),
                sw,
                sh,
                PixelFormat::Bgr24,
                &src,
                src_pitch,
            );
        }
        let expected = reference_blit(&fb, pitch, w, h, 1, 1, bpp, &src, src_pitch, sw, sh);
        assert_eq!(fb, expected, "blit 结果与逐行拷贝参考不符");

        // 逐像素核对：源块的 R/G 通道应落在 (1+x, 1+y)。
        for y in 0..sh {
            for x in 0..sw {
                let off = (1 + y) * pitch + (1 + x) * bpp;
                let s = y * src_pitch + x * bpp;
                assert_eq!(
                    &fb[off..off + bpp],
                    &src[s..s + bpp],
                    "blit 的 ({x},{y}) 像素不符"
                );
            }
        }
        // 目标区域外不得被触碰（缓冲初始全 0）。
        assert_eq!(&fb[..3], &[0u8, 0, 0], "blit 污染了目标区域之外的像素");
    }

    /// `blit` 目标越界时按可见区裁剪，不 panic、不越界。
    #[test]
    fn blit_clips_destination_to_visible_area() {
        let (w, h) = (5usize, 5usize);
        let bpp = PixelFormat::Bgr24.bytes_per_pixel();
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let pitch = info.pitch;

        let (sw, sh) = (8usize, 8usize);
        let src_pitch = sw * bpp;
        let src = make_src_bgr24(sw, sh, src_pitch);

        {
            let mut renderer = render_on(info, &mut fb);
            // 从 (3,3) 开始画 8×8，只有 2×2 可见。
            renderer.blit(
                Pixel::new(3, 3),
                sw,
                sh,
                PixelFormat::Bgr24,
                &src,
                src_pitch,
            );
        }
        let expected = reference_blit(&fb, pitch, w, h, 3, 3, bpp, &src, src_pitch, sw, sh);
        assert_eq!(fb, expected, "越界 blit 的裁剪结果不符");

        // 可见的只有 (3,3)/(4,3)/(3,4)/(4,4) 四格。
        let painted = painted_pixels(&fb);
        assert_eq!(painted, 4, "越界 blit 应只画 4 个像素，实际 {painted}");
    }

    /// `blit` 格式不匹配 → 不写任何像素（静默拒绝）。
    #[test]
    fn blit_format_mismatch_does_nothing() {
        let (info, mut fb) = scratch(4, 2, PixelFormat::Bgr24, Layout::Tight);
        let src = [0xFFu8; 12];
        let mut renderer = render_on(info, &mut fb);
        let blank = renderer.buffer().to_vec();

        // 帧缓冲是 Bgr24，用 Rgb24 的格式参数调用应被拒绝。
        renderer.blit(Pixel::new(0, 0), 4, 1, PixelFormat::Rgb24, &src, 12);
        assert_eq!(renderer.buffer(), &blank[..], "格式不匹配却写入了像素");

        // 565 与 8bpp 之间也不兼容。
        let (info2, mut fb2) = scratch(4, 2, PixelFormat::Rgba8888, Layout::Tight);
        let mut r2 = render_on(info2, &mut fb2);
        let blank2 = r2.buffer().to_vec();
        r2.blit(Pixel::new(0, 0), 4, 1, PixelFormat::Rgb565, &[0xFF; 8], 8);
        assert_eq!(r2.buffer(), &blank2[..], "565 不匹配却写入了像素");
    }

    /// `blit` 源 `pitch` 小于行宽 → 静默拒绝（越界读的防护）。
    #[test]
    fn blit_rejects_src_pitch_smaller_than_row_width() {
        let (info, mut fb) = scratch(4, 2, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        // 先在 (1,0) 放一个哨兵，验证它不被改写。
        renderer.set_pixel(1, 0, &Color::new(255, 255, 255));
        let before = renderer.buffer().to_vec();

        // width=4 × bpp=3 = 每行需 12 字节，但 src_pitch=3。
        let src = [0u8; 6];
        renderer.blit(Pixel::new(0, 0), 4, 1, PixelFormat::Bgr24, &src, 3);
        assert_eq!(renderer.buffer(), &before[..], "src_pitch 过小却写入了像素");
    }

    /// `blit` 源尺寸不足 / 宽高为 0 → 不写、不 panic。
    #[test]
    fn blit_rejects_undersized_source_and_zero_dimensions() {
        let (info, mut fb) = scratch(4, 2, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        let blank = renderer.buffer().to_vec();

        // src.len() < src_pitch * height
        let src = [0xFFu8; 6]; // 只有 6 字节，却声称 2 行 × 12 字节
        renderer.blit(Pixel::new(0, 0), 4, 2, PixelFormat::Bgr24, &src, 12);
        assert_eq!(renderer.buffer(), &blank[..], "源尺寸不足却写入了像素");

        // 宽高为 0
        let src2 = [0xFFu8; 12];
        renderer.blit(Pixel::new(0, 0), 0, 1, PixelFormat::Bgr24, &src2, 12);
        renderer.blit(Pixel::new(0, 0), 4, 0, PixelFormat::Bgr24, &src2, 12);
        assert_eq!(renderer.buffer(), &blank[..], "宽高为 0 却写入了像素");
    }

    /// 565 同格式 `blit` 为字节拷贝，结果与参考一致（含字节序）。
    #[test]
    fn blit_565_same_format_is_byte_copy() {
        for format in FORMATS_565 {
            let (w, h) = (6usize, 4usize);
            let bpp = 2usize;
            let (info, mut fb) = scratch(w, h, format, Layout::Tight);
            let pitch = info.pitch;

            let (sw, sh) = (3usize, 2usize);
            let src_pitch = sw * bpp;
            let mut src = vec![0u8; src_pitch * sh];
            for y in 0..sh {
                for x in 0..sw {
                    let off = y * src_pitch + x * bpp;
                    // 直接构造任意 2 字节内容：blit 对 565 不解码，原样搬运。
                    src[off] = (30 + x + y) as u8;
                    src[off + 1] = (200 - x) as u8;
                }
            }

            {
                let mut renderer = render_on(info, &mut fb);
                renderer.blit(Pixel::new(1, 1), sw, sh, format, &src, src_pitch);
            }
            let expected = reference_blit(&fb, pitch, w, h, 1, 1, bpp, &src, src_pitch, sw, sh);
            assert_eq!(fb, expected, "{format:?} 的 565 blit 与逐行字节拷贝不符");

            // 目标处的字节必须与源**逐字节相同**（含字节序，不经解码/编码）。
            for y in 0..sh {
                for x in 0..sw {
                    let s = y * src_pitch + x * bpp;
                    let d = (1 + y) * pitch + (1 + x) * bpp;
                    assert_eq!(
                        &fb[d..d + bpp],
                        &src[s..s + bpp],
                        "{format:?} 的 ({x},{y}) 不是字节拷贝"
                    );
                }
            }
        }
    }

    /// `blit_mask` 仅在前景处写、掩码为 0 处保留背景。
    #[test]
    fn blit_mask_writes_only_where_mask_is_nonzero() {
        let (w, h) = (6usize, 2usize);
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        let bg = Color::new(10, 20, 30);
        let fg = Color::new(200, 100, 50);
        renderer.clear(bg);

        // 掩码：[255, 0, 128, 0, 255, 0]
        let mask = [255u8, 0, 128, 0, 255, 0];
        renderer.blit_mask(Pixel::new(0, 0), 6, 1, &mask, 6, fg);

        // 全不透明处 = 前景色；掩码 0 处 = 背景；中间值 = 按掩码 alpha 混合。
        assert_eq!(renderer.get_pixel(0, 0), Some(fg), "掩码 255 应写入前景");
        assert_eq!(renderer.get_pixel(1, 0), Some(bg), "掩码 0 应保留背景");
        assert_eq!(renderer.get_pixel(3, 0), Some(bg), "掩码 0 应保留背景");
        assert_eq!(renderer.get_pixel(4, 0), Some(fg), "掩码 255 应写入前景");
        assert_eq!(renderer.get_pixel(5, 0), Some(bg), "掩码 0 应保留背景");
        // 掩码 128：与 bg 混合。
        let expect = blend_ref(&fg, &bg, 128);
        assert_eq!(
            renderer.get_pixel(2, 0),
            Some(expect),
            "掩码 128 应按掩码值作绘制 alpha"
        );
    }

    /// `blit_mask` 掩码 `pitch` 小于宽度 / 长度不足 → 静默拒绝。
    #[test]
    fn blit_mask_rejects_bad_mask_geometry() {
        let (info, mut fb) = scratch(4, 1, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);
        renderer.set_pixel(1, 0, &Color::new(255, 255, 255));
        let before = renderer.buffer().to_vec();

        // mask_pitch < width
        let mask = [0xFFu8; 2];
        renderer.blit_mask(Pixel::new(0, 0), 4, 1, &mask, 1, Color::new(0, 0, 0));
        assert_eq!(renderer.buffer(), &before[..], "mask_pitch 过小却写入");

        // mask.len() < mask_pitch * height
        let mask2 = [0xFFu8; 3];
        renderer.blit_mask(Pixel::new(0, 0), 2, 2, &mask2, 3, Color::new(0, 0, 0));
        assert_eq!(renderer.buffer(), &before[..], "掩码长度不足却写入");

        // 宽高为 0
        renderer.blit_mask(Pixel::new(0, 0), 0, 1, &mask, 4, Color::new(0, 0, 0));
        renderer.blit_mask(Pixel::new(0, 0), 4, 0, &mask, 4, Color::new(0, 0, 0));
        assert_eq!(renderer.buffer(), &before[..], "宽高为 0 却写入");
    }

    /// `blit_mask` 在 4bpp + 非对齐 `pitch` 下与参考一致。
    ///
    /// 4bpp 的批量内核要求行首 4 字节对齐；`pitch % 4 != 0` 时第 2 行起会错位，
    /// 必须回退到逐像素路径。这里同时校验像素值与第 4 字节的 `opaque_alpha`。
    #[test]
    fn blit_mask_4bpp_handles_unaligned_pitch() {
        let w = 5usize;
        let h = 3usize;
        let bg = Color::new(64, 128, 192);
        let fg = Color::new(200, 30, 90);

        // (格式, 该格式的 opaque_alpha 字节)：Xrgb8888 为 0，其余 4bpp 为 255。
        for (format, opaque) in [
            (PixelFormat::Xrgb8888, 0u8),
            (PixelFormat::Argb8888, 255),
            (PixelFormat::Rgba8888, 255),
            (PixelFormat::Bgra8888, 255),
        ] {
            for pitch in [w * 4, w * 4 + 1, w * 4 + 3] {
                let info = FramebufferInfo::new(w, h, pitch, format);
                let mut fb = vec![0u8; info.buffer_size()];
                let mut renderer = Renderer::new(info, &mut fb);
                for y in 0..h {
                    for x in 0..w {
                        renderer.set_pixel(x, y, &bg);
                    }
                }

                // 掩码全部 255：等价于不透明填充。
                let mask = vec![255u8; w * h];
                renderer.blit_mask(Pixel::new(0, 0), w, h, &mask, w, fg);

                for y in 0..h {
                    for x in 0..w {
                        assert_eq!(
                            renderer.get_pixel(x, y).map(|c| (c.r, c.g, c.b)),
                            Some((fg.r, fg.g, fg.b)),
                            "{format:?} / pitch={pitch} 的 ({x},{y}) 掩码混合错误"
                        );
                        // 第 4 字节必须是格式的 opaque_alpha。
                        let off = y * pitch + x * 4;
                        assert_eq!(
                            renderer.buffer()[off + 3],
                            opaque,
                            "{format:?} / pitch={pitch} 的 ({x},{y}) 第 4 字节错误"
                        );
                    }
                }
            }
        }
    }

    /// 565 `blit_mask` 与参考一致。
    ///
    /// 565 无批量路径：掩码字节直接作为该像素的绘制 alpha，
    /// 走「解码 → blend_channel → 量化回写」。
    #[test]
    fn blit_mask_565_matches_reference() {
        for format in FORMATS_565 {
            let w = 6usize;
            let h = 3usize;
            let bpp = 2usize;
            let (info, mut fb) = scratch(w, h, format, Layout::Tight);
            let pitch = info.pitch;
            let bg = Color::new(64, 128, 192);
            let fg = Color::new(200, 30, 90);
            let mut renderer = render_on(info, &mut fb);
            for y in 0..h {
                for x in 0..w {
                    renderer.set_pixel(x, y, &bg);
                }
            }

            let mask: Vec<u8> = (0..w * h).map(|i| (i * 37 % 256) as u8).collect();
            renderer.blit_mask(Pixel::new(0, 0), w, h, &mask, w, fg);

            // 参考：逐像素「解码 → 混合 → 量化」，掩码 0 不写。
            for y in 0..h {
                for x in 0..w {
                    let off = y * pitch + x * bpp;
                    let m = mask[y * w + x];
                    let got = renderer.buffer()[off..off + bpp].to_vec();
                    if m == 0 {
                        // 未写：仍是底色的量化值。
                        let mut want = vec![0u8; bpp];
                        format.write_color(&mut want, &bg);
                        assert_eq!(got, want, "{format:?} 掩码 0 处 ({x},{y}) 被改写");
                        continue;
                    }
                    let mut want = vec![0u8; bpp];
                    format.write_color(&mut want, &bg);
                    format.write_color(&mut want, &fg.mix_alpha(m));
                    assert_eq!(
                        got, want,
                        "{format:?} 的 ({x},{y})（掩码 {m}）与逐像素参考不符"
                    );
                }
            }
        }
    }

    /// `blit_mask` 目标越界时按可见区裁剪，不 panic、不越界。
    #[test]
    fn blit_mask_clips_destination_to_visible_area() {
        let (w, h) = (4usize, 4usize);
        let (info, mut fb) = scratch(w, h, PixelFormat::Bgr24, Layout::Tight);
        let mut renderer = render_on(info, &mut fb);

        // 全部 255 的掩码，目标 (2,2) 起 8×8 —— 只有 2×2 可见。
        let mask = vec![255u8; 64];
        renderer.blit_mask(Pixel::new(2, 2), 8, 8, &mask, 8, Color::new(1, 2, 3));

        assert_eq!(
            painted_pixels(renderer.buffer()),
            4,
            "越界 blit_mask 应只画 4 个像素"
        );
        // 可见区之外仍是 0。
        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(0, 0, 0)));
        assert_eq!(renderer.get_pixel(1, 3), Some(Color::new(0, 0, 0)));
    }

    /// 全 0 掩码 → 一个字节都不写。
    #[test]
    fn blit_mask_all_zero_mask_writes_nothing() {
        for format in ALL_FORMATS {
            let (w, h) = (4usize, 3usize);
            let (info, mut fb) = scratch(w, h, format, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            let before = renderer.buffer().to_vec();

            let mask = vec![0u8; w * h];
            renderer.blit_mask(Pixel::new(0, 0), w, h, &mask, w, Color::new(200, 100, 50));

            assert_eq!(
                renderer.buffer(),
                &before[..],
                "{format:?}：全 0 掩码却写入了字节"
            );
        }
    }

    /// 全 0 透明色 → 提前返回，一个字节都不写（不消耗无谓迭代）。
    #[test]
    fn blit_mask_transparent_color_writes_nothing() {
        for format in ALL_FORMATS {
            let (w, h) = (4usize, 3usize);
            let (info, mut fb) = scratch(w, h, format, Layout::Tight);
            let mut renderer = render_on(info, &mut fb);
            let before = renderer.buffer().to_vec();

            let mask = vec![255u8; w * h];
            let ghost = Color::with_alpha(200, 100, 50, 0);
            renderer.blit_mask(Pixel::new(0, 0), w, h, &mask, w, ghost);

            assert_eq!(
                renderer.buffer(),
                &before[..],
                "{format:?}：a=0 的颜色却写入了字节"
            );
        }
    }
}
