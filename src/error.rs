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

/// BMP 解码失败的原因。
///
/// 由 [`crate::image::bmp::decode`] 返回（编码侧 [`crate::image::bmp::encode`]
/// 不产生错误——参数矛盾一律是调用方的构造期错误，按本库的惯例 panic）。
///
/// 所有变体都对应**调用方无法在此之前得知**的输入数据问题：字节流的内容在解码
/// 之前是未知的，因此这里全部是 `Result` 而非 panic。唯一的例外是
/// [`Unencodeable`](BmpError::Unencodeable)——调用方选什么格式编码是自己知道
/// 的，但「换格式或换库」同样是可恢复语义，故也留在 `Result` 里。
///
/// # 示例
///
/// ```
/// use rasteron::error::BmpError;
///
/// assert_eq!(
///     BmpError::NotBmp.to_string(),
///     "文件头前两字节不是 `BM`，不是 BMP 文件"
/// );
/// assert_eq!(
///     BmpError::Truncated { need: 64, have: 32 }.to_string(),
///     "BMP 数据被截断：需要 64 字节，实际 32 字节"
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmpError {
    /// 数据长度不足以包含固定头部（文件头 14 字节 + DIB 头至少 40 字节）。
    Truncated {
        /// 本解码步骤需要的总字节数。
        need: usize,
        /// 实际拿到的切片长度。
        have: usize,
    },
    /// 文件头前两个字节不是 `BM`。
    NotBmp,
    /// DIB 头大小不是 `BITMAPINFOHEADER`（40）或 `BITMAPV4HEADER`（108）。
    ///
    /// `BITMAPCOREHEADER`（12）与 `BITMAPV5HEADER`（124）同样被拒绝：前者的
    /// 尺寸字段是 `u16`，后者的色彩空间字段本库不解释。
    UnsupportedDibHeader {
        /// 文件里实际的 DIB 头大小。
        size: u32,
    },
    /// 像素位深不受支持（只接受 1 / 4 / 8 / 16 / 24 / 32）。
    UnsupportedBitDepth {
        /// 文件里实际的位深。
        bits: u16,
    },
    /// 压缩方式不受支持：`BI_RLE8` / `BI_RLE4` / `BI_JPEG` / `BI_PNG` /
    /// `BI_ALPHABITFIELDS` 等。
    UnsupportedCompression {
        /// 文件里实际的 `biCompression` 值。
        compression: u32,
    },
    /// 调色板颜色数超出 256（1/4/8 位格式的调色板上限）。
    ///
    /// 更大的调色板只可能出现在大于 8 位的位深上，而那些位深必须带掩码，
    /// 因此这条检查等价于「调色板尺寸与位深不匹配」。
    UnsupportedPaletteSize {
        /// 文件头声明的调色板颜色数。
        colors: u32,
    },
    /// 位域掩码不是本库支持的形状。
    ///
    /// 支持的情形：{5,6,5}（任意红蓝顺序）、32 位下的 8/8/8（可选 alpha 掩码）；
    /// 16 位下与掩码不匹配的位深（例如 32 位图的掩码只有 5 位）也走这条。
    UnsupportedBitfields,

    /// **解码专用**：目标缓冲不足以容纳 `width × height × bytes_per_pixel`。
    ///
    /// 这是调用方侧的尺寸错误。仍用 `Result` 而非 panic：目标缓冲可能来自
    /// 固定容量的静态数组，而图像尺寸要到解析完文件头才知道，两者不匹配
    /// 属于可恢复的运行期状况（换图或换缓冲），不是构造期不变量被破坏。
    BufferTooSmall {
        /// 容纳整幅图需要的字节数（按目标格式与声明的 pitch 计算）。
        need: usize,
        /// 调用方实际提供的切片长度。
        have: usize,
    },
    /// **解码专用**：调用方提供的 `pitch` 小于 `width × bytes_per_pixel`。
    InvalidPitch {
        /// 每行至少需要的字节数。
        need: usize,
        /// 调用方提供的 `pitch`。
        have: usize,
    },
    /// 图像尺寸 × 目标像素字节数在 `usize` 中溢出。
    DimensionOverflow,
    /// **解码专用**：像素数组的偏移或长度与声明的尺寸不自洽。
    ///
    /// 例：`bmp_write` 写入的行数据不足 `height` 行，或 `data_offset` 指向
    /// 像素数据之外的区域。
    TruncatedPixels,
    /// **编码专用**：试图用某种像素格式编码，而 BMP 没有对应的原生表示。
    ///
    /// 目前只有 [`crate::color::PixelFormat::Bgr565`] 触发：BMP 没有记录红蓝
    /// 位段顺序的字段（`BI_BITFIELDS` 的掩码固定按红→绿→蓝排），故只能写成
    /// `Rgb565`。显式拒绝而不是悄悄换格式，是因为 565 的红蓝互换在视觉上就是
    /// 一张反色图。
    Unencodeable {
        /// 被拒绝的像素格式。
        format: crate::color::PixelFormat,
    },
}

impl fmt::Display for BmpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { need, have } => {
                write!(f, "BMP 数据被截断：需要 {need} 字节，实际 {have} 字节")
            }
            Self::NotBmp => f.write_str("文件头前两字节不是 `BM`，不是 BMP 文件"),
            Self::UnsupportedDibHeader { size } => {
                write!(f, "不支持的 DIB 头大小 {size}（仅支持 40 / 108 字节）")
            }
            Self::UnsupportedBitDepth { bits } => {
                write!(f, "不支持的 BMP 位深 {bits}（仅支持 1/4/8/16/24/32）")
            }
            Self::UnsupportedCompression { compression } => {
                write!(f, "不支持的 BMP 压缩方式 {compression}")
            }
            Self::UnsupportedPaletteSize { colors } => {
                write!(f, "不支持的调色板颜色数 {colors}（上限 256）")
            }
            Self::UnsupportedBitfields => f.write_str("不支持的位域掩码组合"),
            Self::BufferTooSmall { need, have } => {
                write!(f, "目标缓冲过小：需要 {need} 字节，实际提供 {have} 字节")
            }
            Self::InvalidPitch { need, have } => {
                write!(f, "pitch {have} 小于每行像素所需的 {need} 字节")
            }
            Self::DimensionOverflow => f.write_str("BMP 尺寸 × 目标像素字节数溢出 usize"),
            Self::TruncatedPixels => f.write_str("BMP 像素数据不完整：偏移或长度与声明尺寸不自洽"),
            Self::Unencodeable { format } => {
                write!(f, "像素格式 {format:?} 没有 BMP 原生表示")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // `#![no_std]` 下 `alloc` 不在 extern prelude，即使只在 `#[cfg(test)]` 里也须显式引入。
    extern crate alloc;

    use super::{BmpError, ColorParseError, DrawError, TextureError};
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
        assert_eq!(
            display_of(BmpError::NotBmp),
            "文件头前两字节不是 `BM`，不是 BMP 文件"
        );
        assert_eq!(
            display_of(BmpError::UnsupportedDibHeader { size: 12 }),
            "不支持的 DIB 头大小 12（仅支持 40 / 108 字节）"
        );
        assert_eq!(
            display_of(BmpError::UnsupportedBitDepth { bits: 2 }),
            "不支持的 BMP 位深 2（仅支持 1/4/8/16/24/32）"
        );
        assert_eq!(
            display_of(BmpError::UnsupportedCompression { compression: 1 }),
            "不支持的 BMP 压缩方式 1"
        );
        assert_eq!(
            display_of(BmpError::UnsupportedPaletteSize { colors: 300 }),
            "不支持的调色板颜色数 300（上限 256）"
        );
        assert_eq!(
            display_of(BmpError::UnsupportedBitfields),
            "不支持的位域掩码组合"
        );
        assert_eq!(
            display_of(BmpError::BufferTooSmall {
                need: 12,
                have: 8
            }),
            "目标缓冲过小：需要 12 字节，实际提供 8 字节"
        );
        assert_eq!(
            display_of(BmpError::InvalidPitch { need: 12, have: 9 }),
            "pitch 9 小于每行像素所需的 12 字节"
        );
        assert_eq!(
            display_of(BmpError::DimensionOverflow),
            "BMP 尺寸 × 目标像素字节数溢出 usize"
        );
        assert_eq!(
            display_of(BmpError::TruncatedPixels),
            "BMP 像素数据不完整：偏移或长度与声明尺寸不自洽"
        );
        assert_eq!(
            display_of(BmpError::Unencodeable {
                format: crate::color::PixelFormat::Bgr565
            }),
            "像素格式 Bgr565 没有 BMP 原生表示"
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
            display_of(BmpError::NotBmp),
            display_of(BmpError::Truncated { need: 54, have: 10 }),
            display_of(BmpError::UnsupportedDibHeader { size: 12 }),
            display_of(BmpError::UnsupportedBitDepth { bits: 2 }),
            display_of(BmpError::UnsupportedCompression { compression: 1 }),
            display_of(BmpError::UnsupportedPaletteSize { colors: 300 }),
            display_of(BmpError::UnsupportedBitfields),
            display_of(BmpError::BufferTooSmall { need: 12, have: 8 }),
            display_of(BmpError::InvalidPitch { need: 12, have: 9 }),
            display_of(BmpError::DimensionOverflow),
            display_of(BmpError::TruncatedPixels),
            display_of(BmpError::Unencodeable {
                format: crate::color::PixelFormat::Bgr565,
            }),
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
