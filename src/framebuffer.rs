pub use crate::color::PixelFormat;

/// 描述帧缓冲布局的元数据。
///
/// # 字段
///
/// * `width` / `height` — 可见像素尺寸。
/// * `pitch` — 相邻行之间的字节偏移量（可能包含填充/DWORD 对齐）。
/// * `format` — 每个像素的 [`PixelFormat`]。
///
/// # 恐慌
///
/// `new` 在以下任一情形恐慌：
///
/// * `width × bytes_per_pixel` 在 `usize` 中溢出；
/// * `pitch × height` 在 `usize` 中溢出；
/// * `pitch < width × bytes_per_pixel`。
///
/// 前两条是布局尺寸的可表示性不变量：溢出后回绕出的「容量」比实际需要的
/// 小，会让 [`Renderer::new`](crate::Renderer::new) 的长度校验形同虚设。
///
/// # 字段是公开的
///
/// `width` / `height` / `pitch` 是公开字段，构造之后仍可被直接改写。因此
/// **不要**把 `new` 的校验当成后续运算的安全前提：所有按布局做算术的地方
/// （[`FramebufferInfo::buffer_size`]、
/// [`Renderer::pixel_offset`](crate::Renderer::pixel_offset) 等）都必须
/// 独立使用 `checked_*` 计算，溢出即恐慌。
pub struct FramebufferInfo {
    /// 可见宽度（像素）。
    pub width: usize,
    /// 可见高度（像素）。
    pub height: usize,
    /// 一行到下一行的字节跨度（≥ `width × bytes_per_pixel`）。
    pub pitch: usize,
    /// 像素内存布局。
    pub format: PixelFormat,
}

impl FramebufferInfo {
    /// 创建新的帧缓冲描述符。
    ///
    /// # 恐慌
    ///
    /// 当 `width × bytes_per_pixel` 或 `pitch × height` 在 `usize` 中溢出，
    /// 或 `pitch` 小于最小所需跨度（`width × bytes_per_pixel`）时恐慌。
    /// 溢出路径**不会**饱和后继续构造：饱和值会让 `pitch >= width × bpp`
    /// 的断言对一个实际装不下的布局失效。
    pub fn new(width: usize, height: usize, pitch: usize, format: PixelFormat) -> FramebufferInfo {
        // 两种尺寸都必须可用 `usize` 精确表示，否则下面的一切判断都建立在
        // 回绕后的假值上。
        let min_pitch = width
            .checked_mul(format.bytes_per_pixel())
            .expect("framebuffer layout overflow: width * bytes_per_pixel");
        let _ = pitch
            .checked_mul(height)
            .expect("framebuffer layout overflow: pitch * height");
        assert!(
            pitch >= min_pitch,
            "pitch must be at least width * bytes_per_pixel"
        );
        FramebufferInfo {
            width,
            height,
            pitch,
            format,
        }
    }

    /// 整个帧缓冲所需的总字节容量（`pitch × height`）。
    ///
    /// # 恐慌
    ///
    /// `pitch × height` 在 `usize` 中溢出时恐慌。字段是公开的，构造期校验
    /// 不能覆盖构造之后的改写，故这里**必须**重新做检查而不是回绕。
    pub fn buffer_size(&self) -> usize {
        self.pitch
            .checked_mul(self.height)
            .expect("framebuffer size overflow")
    }

    /// 每个像素的字节数（从格式派生）。
    pub const fn bytes_per_pixel(&self) -> usize {
        self.format.bytes_per_pixel()
    }

    /// 每个像素的位数（从格式派生）。
    pub const fn bits_per_pixel(&self) -> usize {
        self.format.bits_per_pixel()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_framebuffer_info() {
        let info = FramebufferInfo::new(640, 480, 640 * 4, PixelFormat::Xrgb8888);
        assert_eq!(info.buffer_size(), 640 * 480 * 4);
        assert_eq!(info.bytes_per_pixel(), 4);
        assert_eq!(info.bits_per_pixel(), 32);
    }

    /// 构造期不变量的 panic 契约：消息子串必须逐字保持，调用方按它做诊断匹配。
    #[test]
    #[should_panic(expected = "pitch must be at least")]
    fn pitch_below_minimum_panics() {
        // 640 × 4bpp = 2560，给 2559 少一字节即违规。
        FramebufferInfo::new(640, 480, 2559, PixelFormat::Xrgb8888);
    }

    /// 同一 panic 的另一条路径：3bpp 格式下差一字节同样触发，
    /// 说明断言按 `width × bytes_per_pixel` 换算而非按 4bpp 写死。
    #[test]
    #[should_panic(expected = "pitch must be at least")]
    fn pitch_below_minimum_panics_for_3bpp() {
        // 100 × 3bpp = 300。
        FramebufferInfo::new(100, 100, 299, PixelFormat::Bgr24);
    }

    /// `pitch` 恰好等于 `width × bytes_per_pixel` 是**合法**边界（不 panic），
    /// 即断言是 `>=` 而非 `>`。
    #[test]
    fn pitch_equal_to_minimum_is_accepted() {
        let exact = FramebufferInfo::new(64, 16, 64 * 4, PixelFormat::Rgba8888);
        assert_eq!(exact.pitch, 256);
        assert_eq!(exact.buffer_size(), 256 * 16);

        // 带行填充（DWORD 对齐到 320）同样合法。
        let padded = FramebufferInfo::new(64, 16, 320, PixelFormat::Rgba8888);
        assert_eq!(padded.pitch, 320);
        assert_eq!(padded.buffer_size(), 320 * 16);
    }

    /// `width = 0` 时 `min_pitch` 为 0，任意 `pitch`（含 0）都通过——
    /// 这是契约的一角：`new` 只校验下界，不校验上界或对齐。
    #[test]
    fn zero_width_accepts_any_pitch() {
        let info = FramebufferInfo::new(0, 10, 0, PixelFormat::Rgba8888);
        assert_eq!(info.pitch, 0);
        assert_eq!(info.buffer_size(), 0);
    }

    /// `buffer_size()` 是 `pitch × height` 的**裸乘**，行填充必须计入：
    /// 宽度只覆盖 64 像素而 pitch 是 320 时，总容量按 320 算，不是 256。
    #[test]
    fn buffer_size_counts_row_padding() {
        let info = FramebufferInfo::new(64, 10, 320, PixelFormat::Rgba8888);
        assert_eq!(info.buffer_size(), 3200);
        // 与「无填充」的同尺寸帧对照，差额恰是每行 64 字节填充 × 10 行。
        assert_eq!(info.buffer_size() - 64 * 4 * 10, 640);
    }

    /// `buffer_size()` 在 `height = 0` 时为 0，与 `pitch` 无关。
    #[test]
    fn buffer_size_is_zero_when_height_is_zero() {
        assert_eq!(
            FramebufferInfo::new(64, 0, 320, PixelFormat::Rgba8888).buffer_size(),
            0
        );
    }

    /// 表驱动：`bytes_per_pixel` / `bits_per_pixel` 必须逐格式从 `format` 派生，
    /// 且 `bits == bytes × 8` 对全部 8 种格式成立。
    #[test]
    fn pixel_depth_is_derived_from_format() {
        let table = [
            (PixelFormat::Rgb24, 3usize, 24usize),
            (PixelFormat::Bgr24, 3, 24),
            (PixelFormat::Xrgb8888, 4, 32),
            (PixelFormat::Argb8888, 4, 32),
            (PixelFormat::Rgba8888, 4, 32),
            (PixelFormat::Bgra8888, 4, 32),
            (PixelFormat::Rgb565, 2, 16),
            (PixelFormat::Bgr565, 2, 16),
        ];
        for (format, bytes, bits) in table {
            let info = FramebufferInfo::new(10, 10, bytes * 10, format);
            assert_eq!(
                info.bytes_per_pixel(),
                bytes,
                "{format:?} 的 bytes_per_pixel"
            );
            assert_eq!(info.bits_per_pixel(), bits, "{format:?} 的 bits_per_pixel");
            assert_eq!(info.format, format, "format 字段应原样保留");
        }
    }

    /// `min_pitch` 用 `checked_mul` 计算：`width = usize::MAX` 时乘积溢出，
    /// 构造**直接失败**，而不是饱和成一个看似合理的上界。
    ///
    /// 这是回归护栏：旧实现用 `saturating_mul`，`usize::MAX × 4` 饱和成
    /// `usize::MAX`，于是 `pitch = usize::MAX` 能「通过」校验——那个布局在
    /// 现实中根本不存在，却会一路流到 `Renderer::new` 的长度判断里。
    #[test]
    #[should_panic(expected = "framebuffer layout overflow: width * bytes_per_pixel")]
    fn width_times_bpp_overflow_panics() {
        FramebufferInfo::new(usize::MAX, 1, usize::MAX, PixelFormat::Rgba8888);
    }

    /// 同一路径的 565 版本：`width × 2` 同样溢出，恐慌文案按
    /// `width * bytes_per_pixel` 一致（不是靠 `pitch` 的下界断言兜住）。
    #[test]
    #[should_panic(expected = "framebuffer layout overflow: width * bytes_per_pixel")]
    fn width_times_bpp_overflow_panics_for_565() {
        FramebufferInfo::new(usize::MAX, 0, 4096, PixelFormat::Rgb565);
    }

    /// `pitch × height` 溢出时构造失败，即便 `width × bpp` 完全正常。
    ///
    /// 用 `width = 0` 让 `min_pitch = 0`，从而隔离出「只有 pitch × height
    /// 溢出」这一条路径——避免测试实际命中另一条断言而误判。
    #[test]
    #[should_panic(expected = "framebuffer layout overflow: pitch * height")]
    fn pitch_times_height_overflow_panics() {
        FramebufferInfo::new(0, 2, usize::MAX, PixelFormat::Rgba8888);
    }

    /// `pitch × height` 恰为 `usize::MAX` 的边界仍可构造（边界是溢出而非上界）。
    #[test]
    fn pitch_times_height_at_usize_max_is_accepted() {
        let info = FramebufferInfo::new(0, usize::MAX, 1, PixelFormat::Rgb565);
        assert_eq!(info.buffer_size(), usize::MAX);
    }

    /// 公开字段被构造后改写时，`buffer_size` 必须 fail-fast，不能回绕。
    ///
    /// `width` / `height` / `pitch` 都是 `pub`，`new` 的校验对构造之后的写入
    /// 没有任何约束力——这正是「不能只依赖 `new`」的具体体现。
    #[test]
    #[should_panic(expected = "framebuffer size overflow")]
    fn buffer_size_panics_when_public_fields_are_mutated_to_overflow() {
        let mut info = FramebufferInfo::new(8, 8, 32, PixelFormat::Rgba8888);
        assert_eq!(info.buffer_size(), 256);
        // 直接改写公开字段，绕过一切构造期校验。
        info.height = usize::MAX;
        let _ = info.buffer_size();
    }

    /// 溢出检查在 **release** 下同样生效：容量检查不能只在 debug 断言里存在。
    ///
    /// 断言本身是否被编译掉无法从测试内直接观察，故这里换一个可观察的口径：
    /// 回绕值必须**不可达**。`usize::MAX × 2` 回绕成 `usize::MAX - 1`，
    /// 若实现是裸乘，`buffer_size()` 会返回那个小值并让调用方按它分配。
    #[test]
    fn overflowed_capacity_is_never_returned_even_when_wrapped_value_exists() {
        let wrapped = usize::MAX.wrapping_mul(2); // = usize::MAX - 1
        assert_eq!(wrapped, usize::MAX - 1, "回绕值的前提成立");

        // 手工跑一遍 `buffer_size` 的口径：只要不是 checked 计算，就会得到
        // 那个回绕值。这条断言保证下面的 should_panic 有实际判别力。
        let mut info = FramebufferInfo::new(0, 1, usize::MAX, PixelFormat::Rgba8888);
        info.height = 2;
        let checked = info.pitch.checked_mul(info.height);
        assert!(
            checked.is_none(),
            "pitch × height 应判定为溢出，而不是回绕成 {wrapped}"
        );
    }

    /// 上一条的可观察形式：真正调用 `buffer_size()` 时 panic。
    #[test]
    #[should_panic(expected = "framebuffer size overflow")]
    fn buffer_size_panics_on_wrapped_capacity() {
        let mut info = FramebufferInfo::new(0, 1, usize::MAX, PixelFormat::Rgba8888);
        info.height = 2;
        let _ = info.buffer_size();
    }

    /// `width` 与 `height` 只要不使两种乘积溢出，就原样存进结构体——
    /// `new` 不校验几何合理性（例如宽高为 0），只校验可表示性。
    #[test]
    fn dimensions_are_stored_verbatim_without_geometric_validation() {
        // `usize::MAX` 同时出现在三个字段里必然让 pitch × height 溢出，
        // 故这里取「大但可表示」的组合：pitch × height == usize::MAX。
        let info = FramebufferInfo::new(0, 1, usize::MAX, PixelFormat::Rgba8888);
        assert_eq!(info.width, 0);
        assert_eq!(info.height, 1);
        assert_eq!(info.pitch, usize::MAX);
        assert_eq!(info.buffer_size(), usize::MAX);
    }
}
