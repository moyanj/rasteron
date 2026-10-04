//! 全库错误类型的唯一定义处。
//!
//! 每个对外暴露 `Result` 的 API，其错误类型都收敛在本模块；调用方只需
//! `use rasteron::error::{…}` 即可覆盖全部失败原因，不必在各功能模块间翻找。
//!
//! # 错误分层约定
//!
//! 本库把失败分成两类，对应不同的处理方式：
//!
//! | 类别 | 处理方式 | 本模块中的类型 |
//! |------|----------|----------------|
//! | 构造期不变量被破坏（调用方参数自相矛盾） | `panic`，见 [`FramebufferInfo::new`] 等 | — |
//! | 运行期可恢复的失败 | `Result` / `Option`，错误类型在此定义 | 全部 |
//!
//! 像素级越界（光栅化裁剪）不算错误：几何超出屏幕是正常语义，由调用方的绘制 API
//! 静默裁剪，不产生本模块中的任何类型。
//!
//! # `no_std` 下的 trait 集合
//!
//! 本库是 `#![no_std]`，MSRV 1.79 早于 `core::error::Error`（1.81 稳定），
//! 因此所有错误类型仅实现 [`core::fmt::Debug`] + [`core::fmt::Display`]。
//! 需要 `std::error::Error` 的调用方可在自有边界处自行补一个 impl 包装。
//!
//! [`FramebufferInfo::new`]: crate::framebuffer::FramebufferInfo::new
//!
//! # 示例
//!
//! ```
//! use rasteron::color::Color;
//! use rasteron::error::{ColorParseError, DrawError};
//!
//! // 错误类型与失败原因可整体匹配。
//! let err = Color::from_hex("nope").unwrap_err();
//! assert_eq!(err, ColorParseError::InvalidLength);
//!
//! let draw_err = DrawError::IndexOutOfBounds { index: 3, vertex_count: 2 };
//! assert_eq!(draw_err.to_string(), "索引 3 超出顶点缓冲范围（共 2 个顶点）");
//! ```

use core::fmt;

/// 解析十六进制颜色字符串时可能发生的错误。
///
/// 由 [`Color::from_hex`] 返回。
///
/// # 示例
///
/// ```
/// use rasteron::color::Color;
/// use rasteron::error::ColorParseError;
///
/// assert_eq!(Color::from_hex("#FF0000"), Ok(Color::new(255, 0, 0)));
/// assert_eq!(Color::from_hex("#FF00"), Err(ColorParseError::InvalidLength));
/// assert_eq!(Color::from_hex("#GG0000"), Err(ColorParseError::InvalidDigit));
/// ```
///
/// [`Color::from_hex`]: crate::color::Color::from_hex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorParseError {
    /// 字符串长度不是 6 或 8 个字符（不含 `#`）。
    InvalidLength,
    /// 包含无效的十六进制字符。
    InvalidDigit,
}

impl fmt::Display for ColorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength => {
                f.write_str("十六进制颜色字符串长度非法（需 6 或 8 个字符，不含 `#`）")
            }
            Self::InvalidDigit => f.write_str("十六进制颜色字符串含非法字符"),
        }
    }
}

/// 可编程管线绘制调用失败的原因。
///
/// 由 `programmable` 模块的 `Renderer::draw_arrays` / `Renderer::draw_indexed` 返回。
///
/// 与旧版「索引越界静默跳过整条图元」不同，索引越界是**调用方的程序错误**而非
/// 可恢复的运行时状况，故 fail-fast 向上返回，由调用方决定 `.unwrap()` 还是显式处理。
///
/// # 示例
///
/// ```
/// use rasteron::error::DrawError;
///
/// let err = DrawError::IndexOutOfBounds { index: 7, vertex_count: 4 };
/// assert_eq!(
///     err.to_string(),
///     "索引 7 超出顶点缓冲范围（共 4 个顶点）"
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawError {
    /// 索引缓冲中的索引超出顶点缓冲范围。
    IndexOutOfBounds {
        /// 越界的索引值（`u16` 索引缓冲展开后的 `usize` 值）。
        index: usize,
        /// 顶点缓冲的顶点数量，即合法索引的上界。
        vertex_count: usize,
    },
}

impl fmt::Display for DrawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndexOutOfBounds {
                index,
                vertex_count,
            } => write!(
                f,
                "索引 {index} 超出顶点缓冲范围（共 {vertex_count} 个顶点）"
            ),
        }
    }
}

/// 构造二维纹理失败的原因。
///
/// 纹理尺寸是纯数据，不涉及帧缓冲尺寸，因此这里的两类失败**都可以在调用方侧修正**，
/// 对应 `Texture2D::try_new` 的可恢复版本；`Texture2D::new` 则对同样的条件直接
/// 恐慌，作为构造期不变量的断言入口。
///
/// # 示例
///
/// ```
/// use rasteron::error::TextureError;
///
/// assert_eq!(TextureError::DimensionOverflow.to_string(), "width × height 溢出 usize");
/// assert_eq!(
///     TextureError::LengthMismatch.to_string(),
///     "texels 长度与 width × height 不匹配"
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureError {
    /// `width × height` 溢出 `usize`。
    DimensionOverflow,
    /// `texels.len()` 不等于 `width × height`。
    LengthMismatch,
}

impl fmt::Display for TextureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionOverflow => f.write_str("width × height 溢出 usize"),
            Self::LengthMismatch => f.write_str("texels 长度与 width × height 不匹配"),
        }
    }
}

#[cfg(test)]
mod tests {
    // `#![no_std]` 下 `alloc` 不在 extern prelude，即使只在 `#[cfg(test)]` 里也须显式引入。
    extern crate alloc;

    use super::{ColorParseError, DrawError, TextureError};
    use alloc::string::String;
    use core::fmt::Write;

    /// `#![no_std]` 下 `ToString` 由 `alloc` 实现，测试模块需显式引入；
    /// 用 `core::fmt::Write` 写入 `String` 以捕获展示文本。
    fn display_of(error: impl core::fmt::Display) -> String {
        let mut buffer = String::new();
        write!(buffer, "{error}").expect("写入 String 不会失败");
        buffer
    }

    /// 展示文本是对外契约的一部分：调用方可能把它写进日志，措辞不应被随意改动。
    #[test]
    fn display_messages_are_stable() {
        assert_eq!(
            display_of(ColorParseError::InvalidLength),
            "十六进制颜色字符串长度非法（需 6 或 8 个字符，不含 `#`）"
        );
        assert_eq!(
            display_of(ColorParseError::InvalidDigit),
            "十六进制颜色字符串含非法字符"
        );
        assert_eq!(
            display_of(DrawError::IndexOutOfBounds {
                index: 7,
                vertex_count: 4
            }),
            "索引 7 超出顶点缓冲范围（共 4 个顶点）"
        );
        assert_eq!(
            display_of(TextureError::DimensionOverflow),
            "width × height 溢出 usize"
        );
        assert_eq!(
            display_of(TextureError::LengthMismatch),
            "texels 长度与 width × height 不匹配"
        );
    }

    /// `DrawError::IndexOutOfBounds` 的诊断信息必须同时含越界值与合法上界，
    /// 否则调用方无法区分「索引错了」与「顶点传少了」。
    #[test]
    fn draw_error_reports_both_index_and_bound() {
        let message = display_of(DrawError::IndexOutOfBounds {
            index: 7,
            vertex_count: 4,
        });
        assert!(message.contains('7'), "缺少越界索引：{message}");
        assert!(message.contains('4'), "缺少顶点数量：{message}");
    }
}
