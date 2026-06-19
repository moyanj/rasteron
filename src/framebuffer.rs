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
    pub fn new(
        width: usize,
        height: usize,
        pitch: usize,
        format: PixelFormat,
    ) -> FramebufferInfo {
        let min_pitch = width.saturating_mul(format.bytes_per_pixel());
        assert!(
            pitch >= min_pitch,
            "pitch must be at least width * bytes_per_pixel"
        );
        FramebufferInfo {
            width: width,
            height: height,
            pitch: pitch,
            format: format,
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
    use super::FramebufferInfo;
    use crate::color::PixelFormat;

    #[test]
    fn derives_pixel_size_from_format() {
        let info = FramebufferInfo::new(4, 3, 16, PixelFormat::Rgba8888);

        assert_eq!(info.bytes_per_pixel(), 4);
        assert_eq!(info.bits_per_pixel(), 32);
        assert_eq!(info.buffer_size(), 48);
    }

    #[test]
    #[should_panic(expected = "pitch must be at least width * bytes_per_pixel")]
    fn rejects_pitch_smaller_than_visible_row() {
        let _ = FramebufferInfo::new(4, 1, 11, PixelFormat::Bgr24);
    }
}
