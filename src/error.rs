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
//! 本库是 `#![no_std]`，且刻意不实现 `std::error::Error`（MSRV 早于其稳定版本），
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
/// 对应 [`crate::programmable::texture::Texture2D::try_new`] 的可恢复版本；
/// [`crate::programmable::texture::Texture2D::new`] 则对同样的条件直接恐慌，
/// 作为构造期不变量的断言入口。
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
    /// `width × height × bytes_per_pixel` 溢出 `usize`。
    DimensionOverflow,
    /// `texels.len()` 小于 `width × height × bytes_per_pixel`。
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
    use alloc::format;
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

    /// `DrawError` 是唯一的带字段变体：字段值逐一进入文案，不允许被硬编码。
    /// 覆盖 `0` 与极大值，确认文案由字段驱动而非固定串拼接。
    #[test]
    fn draw_error_display_interpolates_fields() {
        assert_eq!(
            display_of(DrawError::IndexOutOfBounds {
                index: 0,
                vertex_count: 0
            }),
            "索引 0 超出顶点缓冲范围（共 0 个顶点）"
        );
        assert_eq!(
            display_of(DrawError::IndexOutOfBounds {
                index: usize::MAX,
                vertex_count: usize::MAX
            }),
            format!("索引 {} 超出顶点缓冲范围（共 {} 个顶点）", usize::MAX, usize::MAX)
        );
    }

    /// 两个无字段变体的 `Display` 与实例无关：多次构造结果必须完全一致。
    #[test]
    fn payloadless_variants_display_is_position_independent() {
        // 单元变体可重复构造，`Display` 不应读取任何地址或内部状态。
        for _ in 0..4 {
            assert_eq!(
                display_of(ColorParseError::InvalidLength),
                display_of(ColorParseError::InvalidLength)
            );
            assert_eq!(
                display_of(TextureError::DimensionOverflow),
                display_of(TextureError::DimensionOverflow)
            );
        }
    }

    /// 三个枚举的 `Debug` 派生都存在且可 `format!`。`no_std` 下 `Debug` 输出
    /// 交给 `alloc::format` 收集——这正是派生存在的意义，措辞随 derive 而定，
    /// 故此处只断言「能渲染出含变体名的非空串」，不锁定具体格式。
    #[test]
    fn debug_impls_exist_for_all_variants() {
        assert!(format!("{:?}", ColorParseError::InvalidLength).contains("InvalidLength"));
        assert!(format!("{:?}", ColorParseError::InvalidDigit).contains("InvalidDigit"));
        let draw = format!(
            "{:?}",
            DrawError::IndexOutOfBounds {
                index: 3,
                vertex_count: 9
            }
        );
        assert!(draw.contains("IndexOutOfBounds"), "{draw}");
        assert!(draw.contains('3'), "{draw}");
        assert!(draw.contains('9'), "{draw}");
        assert!(format!("{:?}", TextureError::DimensionOverflow).contains("DimensionOverflow"));
        assert!(format!("{:?}", TextureError::LengthMismatch).contains("LengthMismatch"));
    }

    /// 派生集合：`Clone` + `Copy` + `PartialEq` + `Eq` 组合是本模块的对外类型契约，
    /// 调用方依赖 `Copy` 做无分配传递、依赖 `Eq` 做匹配。两两相等 + 复制后独立相等。
    #[test]
    fn enums_derive_copy_clone_and_eq() {
        let a = ColorParseError::InvalidLength;
        let b = a; // 触发 Copy
        #[allow(clippy::clone_on_copy)]
        let c = a.clone(); // 触发 Clone
        assert_eq!(a, b);
        assert_eq!(a, c);

        let d = DrawError::IndexOutOfBounds {
            index: 5,
            vertex_count: 5,
        };
        #[allow(clippy::clone_on_copy)]
        let e = d.clone();
        assert_eq!(d, e);

        let f = TextureError::LengthMismatch;
        #[allow(clippy::clone_on_copy)]
        let g = f.clone();
        assert_eq!(f, g);

        // 变体间不相等：`Eq` 保证可安全用于 match 去重 / HashMap 键。
        assert_ne!(ColorParseError::InvalidLength, ColorParseError::InvalidDigit);
        assert_ne!(TextureError::DimensionOverflow, TextureError::LengthMismatch);
    }

    /// 变体全覆盖：三个枚举的**每个**变体都必须有 `Display` 分支。
    /// 编译器已用穷尽匹配强制这一点，此测试在实现被改动时给出可读的失败信息，
    /// 并确保没有哪个变体意外回落到 `Debug` 式的占位文案。
    #[test]
    fn every_variant_has_a_distinct_display() {
        let messages = [
            display_of(ColorParseError::InvalidLength),
            display_of(ColorParseError::InvalidDigit),
            display_of(DrawError::IndexOutOfBounds {
                index: 1,
                vertex_count: 1,
            }),
            display_of(TextureError::DimensionOverflow),
            display_of(TextureError::LengthMismatch),
        ];
        for (i, message) in messages.iter().enumerate() {
            assert!(!message.is_empty(), "第 {i} 条文案为空");
        }
        // 任意两条不得相同：相同说明某个变体复制了别的变体的文案。
        for i in 0..messages.len() {
            for j in (i + 1)..messages.len() {
                assert_ne!(messages[i], messages[j], "变体 {i} 与 {j} 文案重复");
            }
        }
    }
}
