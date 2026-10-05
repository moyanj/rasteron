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
/// 如果 `pitch < width × bytes_per_pixel`，`new` 将会恐慌。
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
    /// 当 `pitch` 小于最小所需跨度（`width × bytes_per_pixel`）时恐慌。
    pub fn new(width: usize, height: usize, pitch: usize, format: PixelFormat) -> FramebufferInfo {
        let min_pitch = width.saturating_mul(format.bytes_per_pixel());
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
    pub fn buffer_size(&self) -> usize {
        self.pitch * self.height
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
            assert_eq!(info.bytes_per_pixel(), bytes, "{format:?} 的 bytes_per_pixel");
            assert_eq!(info.bits_per_pixel(), bits, "{format:?} 的 bits_per_pixel");
            assert_eq!(info.format, format, "format 字段应原样保留");
        }
    }

    /// `min_pitch` 用 `saturating_mul` 计算：`width = usize::MAX` 时乘积饱和到
    /// `usize::MAX` 而不是回绕成小值。
    ///
    /// 无回绕的直接证据是 `pitch = usize::MAX` 能通过构造——若 `new` 用的是裸 `*`，
    /// `min_pitch` 会回绕到 `usize::MAX - 3`，这条路径就通不过了。
    #[test]
    fn saturating_mul_avoids_wraparound_on_extreme_width() {
        // `width × 4` 饱和为 usize::MAX，故只有 pitch == usize::MAX 才不触发断言。
        let ok = FramebufferInfo::new(usize::MAX, 1, usize::MAX, PixelFormat::Rgba8888);
        assert_eq!(ok.width, usize::MAX);
        assert_eq!(ok.pitch, usize::MAX);
    }

    /// 同一饱和值的**另一半**：普通 `pitch` 在 `width = usize::MAX` 下必定触发断言，
    /// 消息仍是既有的那句——即溢出被转成了「构造期不变量不满足」而非静默回绕。
    #[test]
    #[should_panic(expected = "pitch must be at least")]
    fn saturating_mul_still_rejects_undersized_pitch() {
        // 565 格式：width × 2 同样饱和，4096 远小于饱和值。
        FramebufferInfo::new(usize::MAX, 0, 4096, PixelFormat::Rgb565);
    }

    /// `width` 与 `height` 不参与任何校验：巨值原样存进结构体，
    /// 证明 `new` 只把 `pitch` 当不变量。
    #[test]
    fn dimensions_are_stored_verbatim_without_validation() {
        let info = FramebufferInfo::new(usize::MAX, usize::MAX, usize::MAX, PixelFormat::Rgba8888);
        assert_eq!(info.width, usize::MAX);
        assert_eq!(info.height, usize::MAX);
    }
}
