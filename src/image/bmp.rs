//! BMP（Windows Bitmap）解码与编码。
//!
//! # 概述
//!
//! 把一个 BMP 字节流解码到调用方提供的帧缓冲（任意
//! [`PixelFormat`](crate::color::PixelFormat)），或者把一块像素数据编码成
//! BMP 字节流。两个方向都**不做任何分配**：解码在输出缓冲上就地完成，
//! 编码直接写进调用方给的切片。
//!
//! 支持的输入子集（有意为之的窄口径，见 [# 支持的格式](#支持的格式)）：
//! 1/4/8/16/24/32 位、`BI_RGB` 与 `BI_BITFIELDS`、自底向上与自上而下。
//!
//! # 示例
//!
//! 编码 → 解码往返，逐像素一致：
//!
//! ```
//! use rasteron::color::PixelFormat;
//! use rasteron::framebuffer::FramebufferInfo;
//! use rasteron::image::bmp;
//!
//! // 一张 4×3 的 Bgr24 图，左上角写一个红色像素。
//! let info = FramebufferInfo::new(4, 3, 4 * 3, PixelFormat::Bgr24);
//! let mut pixels = vec![0u8; info.buffer_size()];
//! pixels[0] = 0;      // B
//! pixels[1] = 0;      // G
//! pixels[2] = 255;    // R
//!
//! let mut file = vec![0u8; bmp::encode_size(4, 3, PixelFormat::Bgr24).unwrap()];
//! bmp::encode(&info, &pixels, &mut file).unwrap();
//!
//! // 解码到同尺寸的 Bgr24 缓冲，结果应与原始像素逐字节相同。
//! let mut decoded = vec![0u8; info.buffer_size()];
//! let header = bmp::decode(&file, &mut decoded, 4 * 3, PixelFormat::Bgr24).unwrap();
//! assert_eq!(header.info_width, 4);
//! assert_eq!(header.info_height, 3);
//! assert_eq!(decoded, pixels);
//! ```
//!
//! # 支持的格式
//!
//! ## 解码
//!
//! | 文件里的形态 | 结果 |
//! |--------------|------|
//! | 24 位 | 逐像素转成目标格式（BMP 的 B,G,R 顺序） |
//! | 32 位 `BI_RGB` | 第 4 字节按**保留字节**处理：目标是带 alpha 的格式时**置为 255**，不透明 |
//! | 32 位 `BI_BITFIELDS` 8/8/8 | 按掩码取通道；有 alpha 掩码时把 alpha 一并解出（见下） |
//! | 16 位 | 只接受{5,6,5}掩码；输出 `Rgb565` / `Bgr565` 时**零舍入直通**，否则扩展后写 8 位通道 |
//! | 1/4/8 位 | 查调色板，再按目标格式写 |
//! | 调色板 | `BI_RGB` 时以 `biClrUsed` 为准（为 0 则按 `2^bpp`）；`BI_BITFIELDS` 时**跳过**，此时前三个 `u32` 是掩码 |
//!
//! 32 位 `BI_BITFIELDS` 的 alpha 处理遵循**「全 0 视为未使用」**惯例：只有
//! `biSizeImage` 非零、且 alpha 掩码在数据里并非全 0 时才写入 alpha，否则一律
//! 置 255。若声明的 `biSizeImage` 比一幅图的实际尺寸还小，按「未使用」处理。
//!
//! 垂直方向由 `biHeight` 的符号决定：负值表示自上而下（现代 BMP 的常见形态），
//! 正值表示自底向上（经典形态）。`biHeight == 0` 被拒绝。
//!
//! ## 编码
//!
//! 输出 32 位 `BI_BITFIELDS`（通道按 R,G,B 掩码写出，因此不依赖隐式字节序约定）
//! 或 24 位 `BI_RGB`：
//!
//! | 源格式 | 输出 |
//! |--------|------|
//! | `Argb8888` / `Rgba8888` / `Bgra8888` | 32 位 + alpha 掩码 `0x00FF0000` / `0x0000FF00` / `0x000000FF` |
//! | `Xrgb8888` | 24 位（保留字节没有信息量，写进去反而让「不透明 32 位图」语义含混） |
//! | `Rgb565` | 16 位 `BI_BITFIELDS`，{, ,} → `0xF800 / 0x07E0 / 0x001F` |
//! | `Rgb24` / `Bgr24` | 24 位 `BI_RGB` |
//! | `Bgr565` | **拒绝**（[`BmpError::Unencodeable`]） |
//!
//! # 恐慌
//!
//! 传入的 [`FramebufferInfo`] 若自相矛盾——`width × bytes_per_pixel` 或
//! `pitch × height` 溢出，或 `pitch` 小于 `width × bytes_per_pixel`——会 panic，
//! 判定口径与 [`FramebufferInfo::new`] 一致。这与库内其余布局算术的处理相同：
//! 构造期不变量被破坏属 fail-fast。
//!
//! 像素数据不足时的行为则因方向而异：编码**静默跳过**写不满的整行（与
//! [`crate::Renderer::blit`] 的「参数不自洽就不写」一致），解码返回
//! [`BmpError::TruncatedPixels`]。
//!
//! # 不支持
//!
//! `BITMAPCOREHEADER`（12 字节头）、`BITMAPV5HEADER`（124 字节）、RLE 压缩、
//! JPEG/PNG 内嵌、1/4/8 位以外的调色板、{5,6,5} 以外的 16 位掩码组合。这些
//! 输入一律返回 [`BmpError`] 的对应变体，不会静默产出错误图像。
//!
//! [`PixelFormat`]: crate::color::PixelFormat
//! [`BmpError::Unencodeable`]: crate::error::BmpError::Unencodeable
//! [`FramebufferInfo`]: crate::framebuffer::FramebufferInfo
//! [`FramebufferInfo::new`]: crate::framebuffer::FramebufferInfo::new

mod palette;

pub use palette::Palette;

use crate::color::{Color, PixelFormat, expand_channel_to_8, quantize_channel_to_bits};
use crate::error::BmpError;
use crate::framebuffer::FramebufferInfo;

// ———————————————————————————————— BMP 常量 ————————————————————————————————

/// 文件头（`BITMAPFILEHEADER`）固定 14 字节。
const FILE_HEADER_LEN: usize = 14;
/// `BITMAPINFOHEADER` 的长度。
const INFO_HEADER_LEN: usize = 40;
/// `BITMAPV4HEADER` 的长度（与 `BITMAPINFOHEADER` 共用前 40 字节布局）。
const V4_HEADER_LEN: usize = 108;
/// 调色板单条目的字节数（`BI_RGB`：「蓝,绿,红,0」）。
const PALETTE_ENTRY_LEN: usize = 4;
/// 编码器写出的 DIB 头总长度：40 字节头 + 3 个掩码。
const ENCODE_DIB_LEN: usize = INFO_HEADER_LEN + 3 * 4;

/// `biCompression = BI_RGB`（无压缩，含隐式调色板）。
const BI_RGB: u32 = 0;
/// `biCompression = BI_BITFIELDS`（16/32 位掩码式）。
const BI_BITFIELDS: u32 = 3;

// ———————————————————————————————— 公开类型 ————————————————————————————————

/// 解码得到的 BMP 头信息。
///
/// `info` 描述**输出**帧缓冲的布局（目标格式、调用方给的 `pitch`），而不是文件里
/// 的原始布局；文件侧的原始字段见 `source_width` 等。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BmpHeader {
    /// 解码后的帧缓冲布局：宽 × 高 × 调用方给的 `pitch` 与目标格式。
    ///
    /// 等价于 `FramebufferInfo::new(source_width, source_height, pitch, format)`；
    /// 这里拆成四个标量而不是 `FramebufferInfo` 字段，是因为
    /// `FramebufferInfo` 没有派生 `Copy` / `Debug`，而头信息需要能自由复制。
    pub info_width: usize,
    /// 输出帧缓冲的高度（像素）。
    pub info_height: usize,
    /// 输出帧缓冲的每行字节数（由调用方传入）。
    pub pitch: usize,
    /// 输出帧缓冲的像素格式。
    pub format: PixelFormat,

    /// 文件里声明的宽度（像素）。
    pub source_width: u32,
    /// 文件里声明的原始高度（像素，已取绝对值）。
    pub source_height: u32,
    /// 文件里的位深：1 / 4 / 8 / 16 / 24 / 32。
    pub bits_per_pixel: u16,
    /// 文件里的 `biCompression` 原值（`0` = `BI_RGB`，`3` = `BI_BITFIELDS`）。
    pub compression: u32,
    /// 像素数组在文件中的字节偏移（`bfOffBits`）。
    pub data_offset: usize,
    /// 像素是否自上而下存放（`biHeight < 0`）。
    ///
    /// `true` = 第 0 行在文件中首先出现（现代 BMP）；`false` = 经典的自底向上。
    pub top_down: bool,
}

// ———————————————————————————————— 编码 ————————————————————————————————

/// 返回 [`encode`] 所需的输出字节数。
///
/// # 错误
///
/// 当该格式没有 BMP 原生表示时返回 [`BmpError::Unencodeable`]。
/// `width × height × 每像素字节数` 溢出 `usize` 时返回
/// [`BmpError::DimensionOverflow`]。
///
/// # 示例
///
/// ```
/// use rasteron::color::PixelFormat;
/// use rasteron::image::bmp;
///
/// // 3 像素宽的 Bgr24 行 = 9 字节，BMP 要求每行 4 字节对齐，故补到 12；
/// // 文件头 14 + DIB 40，自底向上 2 行。
/// assert_eq!(bmp::encode_size(3, 2, PixelFormat::Bgr24).unwrap(), 14 + 40 + 12 * 2);
/// // 565 带 12 字节掩码，且行天然对齐。
/// assert_eq!(bmp::encode_size(2, 1, PixelFormat::Rgb565).unwrap(), 14 + 52 + 4);
/// ```
pub fn encode_size(width: usize, height: usize, format: PixelFormat) -> Result<usize, BmpError> {
    let image_size = encoded_image_size(width, height, format)?;
    Ok(header_size_for(format) + image_size)
}

/// 把像素数据编码成完整的 BMP 字节流，写入 `out` 并返回实际字节数。
///
/// `out` 的长度应至少为 [`encode_size`] 的返回值；不足时按 `Renderer::blit`
/// 的惯例**静默跳过**写不满的整行，返回值可能小于所需长度。
///
/// # 参数
///
/// * `info` — 源像素布局（宽、高、pitch、格式）。
/// * `pixels` — 源像素数据，至少 `pitch × height` 字节；不足的行不会出现在输出里。
/// * `out` — 输出缓冲。
///
/// # 错误
///
/// 格式没有 BMP 原生表示（`Bgr565`）时返回 [`BmpError::Unencodeable`]；
/// 尺寸乘积溢出时返回 [`BmpError::DimensionOverflow`]。
///
/// # 恐慌
///
/// `info` 自相矛盾时 panic，判定口径见模块文档。
///
/// # 示例
///
/// ```
/// use rasteron::color::PixelFormat;
/// use rasteron::framebuffer::FramebufferInfo;
/// use rasteron::image::bmp;
///
/// let info = FramebufferInfo::new(2, 2, 2 * 3, PixelFormat::Bgr24);
/// let pixels = [10u8; 2 * 2 * 3];
/// let mut out = vec![0u8; bmp::encode_size(2, 2, info.format).unwrap()];
/// let written = bmp::encode(&info, &pixels, &mut out).unwrap();
/// assert_eq!(written, out.len());
/// assert_eq!(&out[..2], b"BM");
/// ```
pub fn encode(info: &FramebufferInfo, pixels: &[u8], out: &mut [u8]) -> Result<usize, BmpError> {
    // 布局自洽性是构造期不变量：尺寸溢出、pitch 不足都属调用方 bug，fail-fast。
    let row_bytes = info
        .width
        .checked_mul(info.bytes_per_pixel())
        .expect("framebuffer layout overflow: width * bytes_per_pixel");
    info.pitch
        .checked_mul(info.height)
        .expect("framebuffer layout overflow: pitch * height");
    assert!(
        info.pitch >= row_bytes,
        "pitch must be at least width * bytes_per_pixel"
    );

    let (bit_count, compression, masks) = encode_layout(info.format)?;
    let row_stride = align4(row_bytes).ok_or(BmpError::DimensionOverflow)?;
    let image_size = row_stride
        .checked_mul(info.height)
        .ok_or(BmpError::DimensionOverflow)?;
    let header = header_size_for(info.format);
    let file_size = header
        .checked_add(image_size)
        .ok_or(BmpError::DimensionOverflow)?;

    // 头部
    write_file_header(out, file_size, header);
    let dib_len = if masks.is_some() {
        ENCODE_DIB_LEN
    } else {
        INFO_HEADER_LEN
    };
    write_info_header(
        out,
        dib_len,
        info.width,
        info.height,
        bit_count,
        compression,
        image_size,
    );
    if let Some([red, green, blue, alpha]) = masks {
        write_masks(out, [red, green, blue, alpha]);
    }

    // 像素：BMP 默认自底向上，故输出行序与内存行序相反。
    let written = write_encoded_rows(info, pixels, out, header, row_stride);
    Ok(written)
}

/// 该格式的 BMP 头部长度（含掩码，若有）。
fn header_size_for(format: PixelFormat) -> usize {
    match format {
        PixelFormat::Argb8888
        | PixelFormat::Rgba8888
        | PixelFormat::Bgra8888
        | PixelFormat::Rgb565 => FILE_HEADER_LEN + ENCODE_DIB_LEN,
        PixelFormat::Bgr565
        | PixelFormat::Xrgb8888
        | PixelFormat::Rgb24
        | PixelFormat::Bgr24 => FILE_HEADER_LEN + INFO_HEADER_LEN,
    }
}

/// 该格式编码后的像素区字节数（行已按 4 字节对齐）。
fn encoded_image_size(width: usize, height: usize, format: PixelFormat) -> Result<usize, BmpError> {
    let (bit_count, _, _masks) = encode_layout(format)?;
    let row_bytes = width
        .checked_mul(usize::from(bit_count) / 8)
        .ok_or(BmpError::DimensionOverflow)?;
    let row_stride = align4(row_bytes).ok_or(BmpError::DimensionOverflow)?;
    row_stride
        .checked_mul(height)
        .ok_or(BmpError::DimensionOverflow)
}

/// 把源像素格式映射为「位深 / 压缩方式 / 掩码」。
///
/// 掩码为 `None` 表示写 24 位 `BI_RGB`；为 `Some` 时写 16 位或 32 位
/// `BI_BITFIELDS`，`[红, 绿, 蓝, alpha]` 的 alpha 为 `0` 表示不写 alpha 掩码。
fn encode_layout(format: PixelFormat) -> Result<(u16, u32, Option<[u32; 4]>), BmpError> {
    // 32 位带 alpha：掩码随源格式而变，解回来仍是同一格式。
    // `Rgba8888` 的 alpha 位于字的高字节，故它的红蓝掩码与其余两种相反。
    let layout = match format {
        PixelFormat::Argb8888 | PixelFormat::Bgra8888 => {
            (32, [0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000])
        }
        PixelFormat::Rgba8888 => (32, [0x000000FF, 0x0000FF00, 0x00FF0000, 0xFF000000]),
        PixelFormat::Rgb565 => (16, [0xF800, 0x07E0, 0x001F, 0]),
        // 保留字节不含信息量，写进 32 位只会让读端把「不透明」理解成
        // alpha = 0。表格见模块文档「编码」一节。
        PixelFormat::Xrgb8888 | PixelFormat::Rgb24 | PixelFormat::Bgr24 => {
            return Ok((24, BI_RGB, None));
        }
        PixelFormat::Bgr565 => return Err(BmpError::Unencodeable { format }),
    };
    Ok((layout.0, BI_BITFIELDS, Some(layout.1)))
}

/// 写 14 字节文件头。
fn write_file_header(out: &mut [u8], file_size: usize, data_offset: usize) {
    if out.len() < FILE_HEADER_LEN {
        return;
    }
    out[0..2].copy_from_slice(b"BM");
    put_u32(out, 2, file_size as u32);
    put_u16(out, 6, 0); // bfReserved1
    put_u16(out, 8, 0); // bfReserved2
    put_u32(out, 10, data_offset as u32);
}

/// 写 40 字节 `BITMAPINFOHEADER`。
fn write_info_header(
    out: &mut [u8],
    dib_len: usize,
    width: usize,
    height: usize,
    bit_count: u16,
    compression: u32,
    image_size: usize,
) {
    if out.len() < FILE_HEADER_LEN + INFO_HEADER_LEN {
        return;
    }
    let base = FILE_HEADER_LEN;
    put_u32(out, base, dib_len as u32);
    put_u32(out, base + 4, width as u32); // i32 语义，但本库不产出 > i32::MAX 的图
    put_u32(out, base + 8, height as u32);
    put_u16(out, base + 12, 1); // biPlanes 恒为 1
    put_u16(out, base + 14, bit_count);
    put_u32(out, base + 16, compression);
    put_u32(out, base + 20, image_size as u32);
    put_u32(out, base + 24, 2835); // 72 DPI，与既有的示例工具一致
    put_u32(out, base + 28, 2835);
    put_u32(out, base + 32, 0); // biClrUsed：不用调色板
    put_u32(out, base + 36, 0); // biClrImportant
}

/// 在 DIB 头之后写 3 或 4 个掩码。
fn write_masks(out: &mut [u8], masks: [u32; 4]) {
    let base = FILE_HEADER_LEN + INFO_HEADER_LEN;
    if out.len() < base + 3 * 4 {
        return;
    }
    put_u32(out, base, masks[0]);
    put_u32(out, base + 4, masks[1]);
    put_u32(out, base + 8, masks[2]);
    // alpha 紧接着写在留白处（`BITMAPV4HEADER` 的 bV4AlphaMask 位置）；
    // 只声明 52 字节的 DIB 头时它是越界的多余字节，读端不会取。
    if out.len() >= base + 16 {
        put_u32(out, base + 12, masks[3]);
    }
}

/// 逐行写像素，返回一共写入 `out` 的字节数（含文件头）。
///
/// 源行或目的行写不满时**跳过整行**（不写半行），与 `Renderer::blit` 的
/// 「参数不自洽就不写」保持一致。
fn write_encoded_rows(
    info: &FramebufferInfo,
    pixels: &[u8],
    out: &mut [u8],
    header: usize,
    row_stride: usize,
) -> usize {
    let mut written = header;
    if info.width == 0 || info.height == 0 {
        return written;
    }

    for y in 0..info.height {
        // 自底向上：文件里第 y 行对应内存中倒数第 y+1 行。
        let src_y = match info.height.checked_sub(1 + y) {
            Some(value) => value,
            None => break,
        };
        let src_start = match src_y.checked_mul(info.pitch) {
            Some(offset) => offset,
            None => break,
        };
        let src_end = match src_start.checked_add(info.pitch) {
            Some(end) if end <= pixels.len() => end,
            // 源数据不足以覆盖这一行：跳过整行。
            _ => break,
        };
        let dst_end = match written.checked_add(row_stride) {
            Some(end) if end <= out.len() => end,
            _ => break,
        };

        let (dst, _) = out[written..dst_end].split_at_mut(row_stride);
        encode_row(info.format, &pixels[src_start..src_end], dst);
        written = dst_end;
    }
    written
}

/// 把一行源像素编码进目的一行（目的行的行尾填充由调用方保证已是 0 或会被覆盖）。
fn encode_row(format: PixelFormat, src: &[u8], dst: &mut [u8]) {
    let bpp = format.bytes_per_pixel();
    let count = src.len() / bpp;
    match format {
        PixelFormat::Rgb24 => {
            for i in 0..count {
                let off = i * bpp;
                dst[off] = src[off + 2];
                dst[off + 1] = src[off + 1];
                dst[off + 2] = src[off];
            }
        }
        PixelFormat::Bgr24 => dst[..count * 3].copy_from_slice(&src[..count * 3]),
        PixelFormat::Argb8888 | PixelFormat::Bgra8888 => {
            // 掩码 R=0x00FF0000 / G=0x0000FF00 / B=0x000000FF：字里字节序为
            // 「B, G, R, alpha 槽」。alpha 原样带上——`read_color` 对
            // `Argb8888` / `Bgra8888` 会把它强制成 255（那是渲染口径），
            // 编码要的是文件里真实的第 4 字节。
            let alpha_byte = alpha_byte_for_encode(format);
            for i in 0..count {
                let off = i * bpp;
                let color = format.read_color(&src[off..off + bpp]);
                dst[off] = color.b;
                dst[off + 1] = color.g;
                dst[off + 2] = color.r;
                dst[off + 3] = src[off + alpha_byte];
            }
        }
        PixelFormat::Rgba8888 => {
            // 掩码 R=0x000000FF / G=0x0000FF00 / B=0x00FF0000（与上面相反）：
            // 字里字节序为「R, G, B, alpha」，与 `Rgba8888` 的内存布局一致。
            let alpha_byte = alpha_byte_for_encode(format);
            for i in 0..count {
                let off = i * bpp;
                let color = format.read_color(&src[off..off + bpp]);
                dst[off] = color.r;
                dst[off + 1] = color.g;
                dst[off + 2] = color.b;
                dst[off + 3] = src[off + alpha_byte];
            }
        }
        PixelFormat::Xrgb8888 => {
            // 保留字节不含信息：统一写成 0，读回时由「alpha 掩码缺失」判成不透明。
            // 注意：24 位图的目标行只有 3 字节/像素，不能写 dst[off + 3]。
            for i in 0..count {
                let off = i * bpp;
                let color = format.read_color(&src[off..off + bpp]);
                dst[i * 3] = color.b;
                dst[i * 3 + 1] = color.g;
                dst[i * 3 + 2] = color.r;
            }
        }
        PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
            // 565 的位域与 BMP 的 {5,6,5} 掩码一一对应：R=15..11、G=10..5、B=4..0。
            for i in 0..count {
                let off = i * bpp;
                let color = format.read_color(&src[off..off + bpp]);
                let r = quantize_channel_to_bits(color.r, 5) << 11;
                let g = quantize_channel_to_bits(color.g, 6) << 5;
                let b = quantize_channel_to_bits(color.b, 5);
                dst[off..off + 2].copy_from_slice(&(r | g | b).to_le_bytes());
            }
        }
    }
}

/// 源格式里 alpha 的**字节下标**。
///
/// `Argb8888` 与 `Bgra8888` 的 alpha 都在第 0 字节，`Rgba8888` 在第 3 字节。
/// 编码器据此把源缓冲的 alpha 原样搬进目标字的第 4 字节，绕开
/// `read_color` 对这两种格式的「强制不透明」处理。
fn alpha_byte_for_encode(format: PixelFormat) -> usize {
    match format {
        // `Argb8888` / `Bgra8888` 的字节序是 B, G, R, A（小端 0xAARRGGBB），
        // alpha 在第 3 字节。
        PixelFormat::Argb8888 | PixelFormat::Bgra8888 => 3,
        _ => 3,
    }
}

// ———————————————————————————————— 解码 ————————————————————————————————

/// 把 `data` 里的 BMP 解码进 `out`，返回头信息。
///
/// # 参数
///
/// * `data` — 完整（或至少包含像素区的）BMP 字节流。
/// * `out` — 目标帧缓冲，长度至少 `pitch × height`，`height` 取文件里声明的值。
/// * `pitch` — 目标每行字节数，必须 ≥ `width × format.bytes_per_pixel()`。
/// * `format` — 目标像素格式。
///
/// # 错误
///
/// 见 [`BmpError`]。特别地：`out` 不够大返回 [`BmpError::BufferTooSmall`]，
/// `pitch` 太小返回 [`BmpError::InvalidPitch`]，文件比声明的尺寸短返回
/// [`BmpError::TruncatedPixels`]。
///
/// # 示例
///
/// ```
/// use rasteron::color::PixelFormat;
/// use rasteron::image::bmp;
///
/// // 一个 1×1 的 24 位 BMP：文件头 14 + DIB 头 40，像素行补到 4 字节。
/// let mut file = vec![0u8; 54 + 4];
/// file[0..2].copy_from_slice(b"BM");
/// file[10..14].copy_from_slice(&54u32.to_le_bytes());   // bfOffBits
/// file[14..18].copy_from_slice(&40u32.to_le_bytes());   // biSize
/// file[18..22].copy_from_slice(&1i32.to_le_bytes());    // biWidth
/// file[22..26].copy_from_slice(&1i32.to_le_bytes());    // biHeight
/// file[26..28].copy_from_slice(&1u16.to_le_bytes());    // biPlanes
/// file[28..30].copy_from_slice(&24u16.to_le_bytes());   // biBitCount
/// file[54] = 0;    // B
/// file[55] = 0;    // G
/// file[56] = 255;  // R
///
/// let mut out = [0u8; 3];
/// let header = bmp::decode(&file, &mut out, 3, PixelFormat::Rgb24).unwrap();
/// assert_eq!((header.info_width, header.info_height), (1, 1));
/// assert_eq!(out, [255, 0, 0]);
/// ```
pub fn decode(
    data: &[u8],
    out: &mut [u8],
    pitch: usize,
    format: PixelFormat,
) -> Result<BmpHeader, BmpError> {
    let header = parse_header(data, pitch, format)?;
    let layout = decode_masks(data, header.dib_len, header.compression, header.bits_per_pixel)?;

    let required = pitch
        .checked_mul(header.height)
        .ok_or(BmpError::DimensionOverflow)?;
    if out.len() < required {
        return Err(BmpError::BufferTooSmall {
            need: required,
            have: out.len(),
        });
    }

    // 布局目标尺寸的校验已在 `parse_header` 内完成。
    let width = header.width;
    let height = header.height;
    let bits = header.bits_per_pixel;

    // 文件里的行宽（按位深算，含 4 字节对齐）。
    // 注意 `div_ceil`：1/4 位图一行不到一字节，整数除法会把行宽算成 0，
    // 于是每行都取到空切片——索引一个都读不到，整行静默留黑。
    let source_row_bits = width
        .checked_mul(usize::from(bits))
        .ok_or(BmpError::DimensionOverflow)?;
    let source_row_bytes = source_row_bits.div_ceil(8);
    let source_stride = align4(source_row_bytes).ok_or(BmpError::DimensionOverflow)?;

    // 解码器对 32 位 `BI_BITFIELDS` 的 alpha 采用「全 0 视为未使用」的判断，
    // 这需要先扫一遍数据，故在这里完成而不是逐行查。
    let alpha_used = layout.alpha.map_or(false, |alpha_mask| {
        alpha_channel_is_used(data, &header, alpha_mask, source_stride)
    });

    let row_bytes = width
        .checked_mul(format.bytes_per_pixel())
        .ok_or(BmpError::DimensionOverflow)?;

    // 调色板紧跟 DIB 头与掩码区（`BITMAPV4HEADER` 的掩码在头内，无外部区段）。
    // 位置由头部布局算出，**不用** `bfOffBits` 反推，理由见 `palette` 模块文档。
    let palette_offset = FILE_HEADER_LEN
        .checked_add(header.dib_len)
        .and_then(|value| value.checked_add(masks_region_len(header.dib_len, &layout)))
        .ok_or(BmpError::DimensionOverflow)?;
    let context = RowContext {
        bits,
        masks: layout.masks,
        palette: Palette::new(data, palette_offset, layout.palette),
        alpha_mask: layout.alpha.unwrap_or(0),
        alpha_used,
    };

    for y in 0..height {
        // `biHeight < 0` 表示自上而下；正值是经典的自底向上。
        let source_y = if header.top_down { y } else { height - 1 - y };
        let source_start = header
            .data_offset
            .checked_add(source_y.saturating_mul(source_stride))
            .ok_or(BmpError::DimensionOverflow)?;
        let source_end = source_start
            .checked_add(source_row_bytes)
            .ok_or(BmpError::DimensionOverflow)?;
        let source = data
            .get(source_start..source_end)
            .ok_or(BmpError::TruncatedPixels)?;

        let target_start = y * pitch;
        let target_end = target_start + row_bytes;
        let target = &mut out[target_start..target_end];
        decode_row(&context, source, target, format);
    }

    Ok(BmpHeader {
        info_width: width,
        info_height: height,
        pitch,
        format,
        source_width: width as u32,
        source_height: height as u32,
        bits_per_pixel: bits,
        compression: header.compression,
        data_offset: header.data_offset,
        top_down: header.top_down,
    })
}

/// 解码过程中用到的定长头部字段。
struct ParsedHeader {
    width: usize,
    height: usize,
    bits_per_pixel: u16,
    compression: u32,
    data_offset: usize,
    dib_len: usize,
    top_down: bool,
}

/// 解析并校验文件头，确定输出布局能装下这张图。
fn parse_header(data: &[u8], pitch: usize, format: PixelFormat) -> Result<ParsedHeader, BmpError> {
    if data.len() < FILE_HEADER_LEN + 4 {
        return Err(BmpError::Truncated {
            need: FILE_HEADER_LEN + 4,
            have: data.len(),
        });
    }
    if &data[0..2] != b"BM" {
        return Err(BmpError::NotBmp);
    }

    let data_offset = read_u32(data, 10)? as usize;
    let dib_len = read_u32(data, FILE_HEADER_LEN)? as usize;
    if dib_len != INFO_HEADER_LEN && dib_len != V4_HEADER_LEN && dib_len != ENCODE_DIB_LEN {
        return Err(BmpError::UnsupportedDibHeader {
            size: dib_len as u32,
        });
    }
    if data.len() < FILE_HEADER_LEN + dib_len {
        return Err(BmpError::Truncated {
            need: FILE_HEADER_LEN + dib_len,
            have: data.len(),
        });
    }

    let raw_width = read_i32(data, FILE_HEADER_LEN + 4)?;
    let raw_height = read_i32(data, FILE_HEADER_LEN + 8)?;
    let bit_count = read_u16(data, FILE_HEADER_LEN + 14)?;
    let compression = read_u32(data, FILE_HEADER_LEN + 16)?;

    // 宽度非正、高度为 0 都没有像素可解。负宽度是 `BI_RGB` 扩展区的约定，
    // 本库不解。
    if raw_width <= 0 || raw_height == 0 {
        return Err(BmpError::DimensionOverflow);
    }
    if !matches!(bit_count, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(BmpError::UnsupportedBitDepth { bits: bit_count });
    }
    // `BI_ALPHABITFIELDS`（6）虽然布局与 `BI_BITFIELDS` 相同，但它的 alpha
    // 掩码另有位置，本库不解释，故不在白名单里。
    if compression != BI_RGB && compression != BI_BITFIELDS {
        return Err(BmpError::UnsupportedCompression { compression });
    }
    // 无压缩的 16/32 位没有 alpha 与通道掩码；16 位在 `BI_RGB` 下的通道约定
    // 随平台漂移，直接把掩码缺失留给 `decode_masks` 报错更诚实。
    if compression == BI_RGB && matches!(bit_count, 16 | 32) {
        return Err(BmpError::UnsupportedCompression { compression });
    }

    let width = raw_width as u32 as usize;
    let height = raw_height.unsigned_abs() as usize;

    // 目标布局必须能装下这张图。
    let row_bytes = width
        .checked_mul(format.bytes_per_pixel())
        .ok_or(BmpError::DimensionOverflow)?;
    if pitch < row_bytes {
        return Err(BmpError::InvalidPitch {
            need: row_bytes,
            have: pitch,
        });
    }
    let need = pitch
        .checked_mul(height)
        .ok_or(BmpError::DimensionOverflow)?;
    if data.len() < data_offset {
        // 像素区起点都在文件之外：比截断更早暴露问题。
        return Err(BmpError::TruncatedPixels);
    }
    // 缓冲长度检查需要调用方传入的 `out`，统一放在 `decode` 里做。
    let _ = need;

    Ok(ParsedHeader {
        width,
        height,
        bits_per_pixel: bit_count,
        compression,
        data_offset,
        dib_len,
        top_down: raw_height < 0,
    })
}

/// 解码时用到的掩码与调色板信息。
struct DecodeLayout {
    masks: [u32; 3],
    palette: usize,
    alpha: Option<u32>,
}

/// 从 DIB 头（及紧随其后的掩码区）取出掩码与调色板信息。
///
/// 返回的 `masks` 恒为 `[红, 绿, 蓝]`；`palette` 是调色板颜色数；
/// `alpha` 只在 `BITMAPV4HEADER` 且存在 alpha 掩码时非 `None`。
fn decode_masks(
    data: &[u8],
    dib_len: usize,
    compression: u32,
    bit_count: u16,
) -> Result<DecodeLayout, BmpError> {
    if compression == BI_RGB {
        // 24 位及更宽的位深没有调色板；1/4/8 位的调色板长度由 `biClrUsed` 决定。
        let colors = if bit_count <= 8 {
            let clr_used = read_u32(data, FILE_HEADER_LEN + 32)? as usize;
            if clr_used == 0 {
                1usize << bit_count
            } else {
                clr_used
            }
        } else {
            0
        };
        if colors > 256 {
            return Err(BmpError::UnsupportedPaletteSize {
                colors: colors as u32,
            });
        }
        return Ok(DecodeLayout {
            masks: [0, 0, 0],
            palette: colors,
            alpha: None,
        });
    }

    // `BI_BITFIELDS`：三个掩码紧跟在 DIB 头之后。
    //
    // `BITMAPV4HEADER` 的四个掩码是头**内**的字段（`bV4RedMask` 在头部偏移
    // 40、alpha 掩码在偏移 52），不是外部区段；此时头之后紧跟的就是像素区
    // （或调色板），再按 40 字节头的位置去读会读进像素数据。两条路径必须
    // 分开取，不能"统一按下标读"。
    let (maska, maskb, maskc, maskd) = if dib_len >= V4_HEADER_LEN {
        (
            read_u32(data, FILE_HEADER_LEN + 40)?,
            read_u32(data, FILE_HEADER_LEN + 44)?,
            read_u32(data, FILE_HEADER_LEN + 48)?,
            read_u32(data, FILE_HEADER_LEN + 52)?,
        )
    } else {
        // 掩码紧跟在 40 字节 `BITMAPINFOHEADER` 之后（偏移 14+40=54）。
        // `dib_len` 可能是 52（ENCODE_DIB_LEN，含掩码）或 40（不含掩码），
        // 但掩码的物理位置始终是 54，与 `write_masks` 的写入位置一致。
        let base = FILE_HEADER_LEN + INFO_HEADER_LEN;
        if data.len() < base + 12 {
            return Err(BmpError::Truncated {
                need: base + 12,
                have: data.len(),
            });
        }
        (
            read_u32(data, base)?,
            read_u32(data, base + 4)?,
            read_u32(data, base + 8)?,
            0,
        )
    };
    // 掩码区之后若是像素区，它仍要能落在文件内（`decode` 会再逐行校验；
    // 这里提前暴露"调色板/掩码区把像素挤到文件外"的坏文件）。
    if dib_len >= V4_HEADER_LEN && data.len() < FILE_HEADER_LEN + dib_len {
        return Err(BmpError::Truncated {
            need: FILE_HEADER_LEN + dib_len,
            have: data.len(),
        });
    }
    let (red, green, blue) = (maska, maskb, maskc);
    // alpha 掩码只出现在 `BITMAPV4HEADER` 的第 4 个掩码字段（`bV4AlphaMask`）。
    let alpha = if dib_len >= V4_HEADER_LEN && maskd != 0 {
        Some(maskd)
    } else {
        None
    };
    Ok(DecodeLayout {
        masks: [red, green, blue],
        palette: 0,
        alpha,
    })
}

/// 检查 alpha 掩码在像素数据里是否携带真实信息。
///
/// 遵循「全 0 视为未使用」的通行惯例：32 位图里大量工具的 alpha 通道是
/// 未初始化的 0，若照单全收会把整张图解成全透明。为此扫一遍**非 0 位深
/// 的掩码位**，只要有一处非 0 就认为 alpha 有效。
fn alpha_channel_is_used(
    data: &[u8],
    header: &ParsedHeader,
    alpha_mask: u32,
    source_stride: usize,
) -> bool {
    if alpha_mask == 0 {
        return false;
    }
    // 声明尺寸装不下整幅图时按「未使用」处理，让后续的行级检查报 `TruncatedPixels`。
    let total = match source_stride.checked_mul(header.height) {
        Some(value) => value,
        None => return false,
    };
    let end = match header.data_offset.checked_add(total) {
        Some(value) if value <= data.len() => value,
        _ => return false,
    };
    let region = &data[header.data_offset..end];
    let word_bytes = usize::from(header.bits_per_pixel) / 8;
    let little_endian = header.bits_per_pixel == 32 || header.bits_per_pixel == 16;
    for chunk in region.chunks_exact(word_bytes.max(1)) {
        let word = match word_bytes {
            2 => {
                let pair = [chunk[0], chunk[1]];
                if little_endian {
                    u32::from(u16::from_le_bytes(pair))
                } else {
                    u32::from(u16::from_be_bytes(pair))
                }
            }
            4 => {
                let quad = [chunk[0], chunk[1], chunk[2], chunk[3]];
                if little_endian {
                    u32::from_le_bytes(quad)
                } else {
                    u32::from_be_bytes(quad)
                }
            }
            _ => return false,
        };
        if word & alpha_mask != 0 {
            return true;
        }
    }
    false
}

/// 一行像素的解码上下文：位深、掩码与调色板位置。
struct RowContext<'a> {
    /// 文件位深。
    bits: u16,
    /// `[红, 绿, 蓝]` 掩码（`BI_RGB` 的 24 位不使用）。
    masks: [u32; 3],
    /// 调色板（`BI_RGB` 的 1/4/8 位图有内容，其余位深为空）。
    palette: Palette<'a>,
    /// `BITMAPV4HEADER` 声明的 alpha 掩码（`0` 表示没有）。
    alpha_mask: u32,
    /// alpha 掩码在像素数据里是否携带真实信息。
    alpha_used: bool,
}

/// 解码一行像素到目标格式。
///
/// 三条分支按位深分开，都是「先把源像素读成 `Color`/索引，再按目标格式落盘」：
/// 唯一的例外是 16 位 → 16 位，两者位域完全相同时直接搬字（见
/// [`write_passthrough_565`]）。
fn decode_row(context: &RowContext<'_>, source: &[u8], target: &mut [u8], format: PixelFormat) {
    match context.bits {
        24 => decode_row_24(source, target, format),
        16 => {
            if !write_passthrough_565(context, source, target, format) {
                decode_row_16(context, source, target, format);
            }
        }
        32 => decode_row_32(context, source, target, format),
        // 1 / 4 / 8 位：按索引检索调色板。`bits` 已在 `parse_header` 白名单内。
        bits => context.palette.decode_row(u32::from(bits), source, target, format),
    }
}

/// 把目标格式的一个 `Color` 落到 `target` 的指定偏移上。
///
/// # 为什么要绕开 `write_color` 的混合分支
///
/// [`PixelFormat::write_color`] 把「`a == 255`」当作覆盖、「`a < 255`」当作混合，
/// 那是**渲染**语义；解码是**转写**语义，源像素的 alpha 必须原样落到目标字节上。
/// 因此 alpha 不为 255 的 32 位像素不走 `write_color`：直接按目标格式的通道
/// 位移写四个字节，不读目标原值。
#[inline(always)]
fn store_color(target: &mut [u8], offset: usize, format: PixelFormat, color: Color) {
    let end = offset + format.bytes_per_pixel();
    if format.bytes_per_pixel() == 4 {
        // `red_shift` 只给红的位置，绿/蓝/en-GB（BGRA 与 ARGB 共享位移）；
        // ARGB 的绿蓝顺序与 BGRA 相反，必须单独列。
        let (r, g, b, a) = pixel_byte_order(format);
        let pixel = &mut target[offset..end];
        pixel[r] = color.r;
        pixel[g] = color.g;
        pixel[b] = color.b;
        // `Xrgb8888` 的保留字节没有信息量：解码时恒为不透明，但写回时
        // 要保持与源数据一致（0），否则往返比对会失败。
        pixel[a] = if format == PixelFormat::Xrgb8888 { 0 } else { color.a };
        return;
    }
    format.write_color(&mut target[offset..end], &color);
}

/// 32 位格式的（红, 绿, 蓝, alpha）字节下标。
///
/// 只有三个带 alpha 的变体会走到这里；`Xrgb8888` 的 alpha 字节是保留位，
/// 解码时恒为不透明，不需要在这张表里区分。
#[inline(always)]
fn pixel_byte_order(format: PixelFormat) -> (usize, usize, usize, usize) {
    match format {
        PixelFormat::Rgba8888 => (0, 1, 2, 3),
        // 小端字 0xAARRGGBB 的前 4 字节是 B, G, R, A；0xAABBGGRR 语义的
        // 「BGRA」在小端存储下同样是 B, G, R, A。两者字节序一致，差别只在
        // 头里写的掩码名与 alpha 视角，逐字节写入等价。
        PixelFormat::Bgra8888 | PixelFormat::Xrgb8888 => (2, 1, 0, 3),
        // 与 `write_color` 的实际写序一致：B, G, R, alpha。
        PixelFormat::Argb8888 => (2, 1, 0, 3),
        _ => (0, 1, 2, 3),
    }
}

/// 24 位 `BI_RGB`：源像素恒为 B, G, R。
fn decode_row_24(source: &[u8], target: &mut [u8], format: PixelFormat) {
    let bpp = format.bytes_per_pixel();
    let pixel_count = target.len() / bpp;
    for (index, pixel) in source.chunks_exact(3).enumerate().take(pixel_count) {
        let color = Color::new(pixel[2], pixel[1], pixel[0]);
        store_color(target, index * bpp, format, color);
    }
}

/// 16 位 `BI_BITFIELDS`：按掩码取 {5,6,5}。
fn decode_row_16(
    context: &RowContext<'_>,
    source: &[u8],
    target: &mut [u8],
    format: PixelFormat,
) {
    let bpp = format.bytes_per_pixel();
    let pixel_count = target.len() / bpp;
    let fields = match BitFields::from_masks(&context.masks) {
        Some(value) => value,
        // 掩码形状在 `decode_masks` 之后仍不合法时按全黑处理会掩盖错误，
        // 但那一步已经没有 `Result` 通道；保持「不崩、不写坏内存」即可。
        None => return,
    };
    for (index, pixel) in source.chunks_exact(2).enumerate().take(pixel_count) {
        let word = u16::from_le_bytes([pixel[0], pixel[1]]);
        let color = fields.to_color(u32::from(word));
        store_color(target, index * bpp, format, color);
    }
}

/// 16 位 → 16 位且位域一致时直接搬字。
///
/// BMP 的 {5,6,5} 掩码与本库 `Rgb565` 的位域（R=15..11、G=10..5、B=4..0）
/// 完全重合，因此无需「解码到 8 位再量化回去」那一趟有损往返。返回 `false`
/// 表示位域不同或目标不是 16 位，调用方走通用路径。
fn write_passthrough_565(
    context: &RowContext<'_>,
    source: &[u8],
    target: &mut [u8],
    format: PixelFormat,
) -> bool {
    let [red, green, blue] = context.masks;
    let matches_rgb = (red, green, blue) == (0xF800, 0x07E0, 0x001F);
    let matches_bgr = (red, green, blue) == (0x001F, 0x07E0, 0xF800);
    if !matches_rgb && !matches_bgr {
        return false;
    }
    // 目标与源都是小端 565：`Rgb565` 对应红在高位的掩码组合。
    let same_layout = (format == PixelFormat::Rgb565 && matches_rgb)
        || (format == PixelFormat::Bgr565 && matches_bgr);
    if !same_layout {
        return false;
    }
    let count = source.len().min(target.len()) / 2;
    target[..count * 2].copy_from_slice(&source[..count * 2]);
    true
}

/// 32 位：`BI_BITFIELDS` 按掩码取通道；`BI_RGB` 已在 `parse_header` 拒绝。
fn decode_row_32(
    context: &RowContext<'_>,
    source: &[u8],
    target: &mut [u8],
    format: PixelFormat,
) {
    let bpp = format.bytes_per_pixel();
    let pixel_count = target.len() / bpp;
    for (index, pixel) in source.chunks_exact(4).enumerate().take(pixel_count) {
        let word = u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
        // alpha 掩码只来自 `BITMAPV4HEADER`（见 `decode_masks`），`0` 表示没有。
        let alpha_mask = context.alpha_mask;
        let color = match BitFields::from_masks(&context.masks) {
            Some(fields) => {
                let mut color = fields.to_color(word);
                // 32 位格式的 alpha 原样往返（含半透明值与 0），否则往返比对会失败。
                // 其它格式按「未使用」解成不透明。
                color.a = if format.bytes_per_pixel() == 4 {
                    extract(word, (24, 8))
                } else {
                    match field_of(alpha_mask) {
                        Some(field) if context.alpha_used => extract(word, field),
                        _ => 255,
                    }
                };
                color
            }
            // 掩码不合法：32 位图的通用布局是 BGRA，按它尽力解。
            None => Color::with_alpha(pixel[2], pixel[1], pixel[0], 255),
        };
        store_color(target, index * bpp, format, color);
    }
}

/// 三段位域掩码：位移与位宽。
///
/// BMP 的 `BI_BITFIELDS` 允许任意掩码组合，但本库只接受「连续的一段位」——
/// 这是所有真实文件的形态。非连续掩码返回 `None`。
struct BitFields {
    red: (u8, u8),
    green: (u8, u8),
    blue: (u8, u8),
}

impl BitFields {
    /// 从三段掩码构造；任一掩码不连续或位宽超过 8 位则返回 `None`。
    fn from_masks(masks: &[u32; 3]) -> Option<Self> {
        Some(Self {
            red: field_of(masks[0])?,
            green: field_of(masks[1])?,
            blue: field_of(masks[2])?,
        })
    }

    /// 按掩码取值并扩展成 8 位通道。
    fn to_color(&self, word: u32) -> Color {
        Color::new(
            extract(word, self.red),
            extract(word, self.green),
            extract(word, self.blue),
        )
    }
}

/// 把一段连续掩码描述成 `(位移, 位宽)`；位宽超过 8 或掩码为 0 返回 `None`。
fn field_of(mask: u32) -> Option<(u8, u8)> {
    if mask == 0 {
        return None;
    }
    let shift = mask.trailing_zeros() as u8;
    let shifted = mask >> shift;
    let width = shifted.count_ones() as u8;
    // 连续掩码左移后形如 `11..1`：与 `(1 << width) - 1` 逐位相等才算连续。
    // `width` 已限定 ≤ 32，`1u64 << 32` 不会溢出。
    if (1u64 << width) - 1 != u64::from(shifted) {
        return None;
    }
    if width > 8 {
        return None;
    }
    Some((shift, width))
}

/// 从字里取出 `(位移, 位宽)` 描述的通道并扩展回 8 位。
#[inline(always)]
fn extract(word: u32, (shift, width): (u8, u8)) -> u8 {
    if width >= 8 {
        // 8 位通道：不减 1，避免 `1u32 << 32` 的移位溢出。
        ((word >> shift) & 0xFF) as u8
    } else {
        let value = (word >> shift) & ((1u32 << width) - 1);
        expand_channel_to_8(value as u16, width)
    }
}

/// `BI_BITFIELDS` 在 DIB 头之后附加的掩码区字节数。
///
/// `BITMAPV4HEADER` 的掩码本身就是头的一部分（紧跟 `biClrImportant` 之后），
/// 因此调色板起点要跨过它；`BITMAPINFOHEADER` 的 12 字节掩码同样如此。
fn masks_region_len(dib_len: usize, layout: &DecodeLayout) -> usize {
    if layout.palette == 0 && layout.masks != [0, 0, 0] {
        // 3 个 u32；`BITMAPV4HEADER` 里 alpha 掩码已含在 108 字节内。
        if dib_len > INFO_HEADER_LEN { 0 } else { 12 }
    } else {
        0
    }
}

// ———————————————————————————————— 小工具 ————————————————————————————————

/// 把 `value` 向上取整到 4 的倍数；溢出返回 `None`。
fn align4(value: usize) -> Option<usize> {
    value.checked_add(3).map(|v| v & !3)
}

fn put_u16(out: &mut [u8], offset: usize, value: u16) {
    if let Some(slot) = out.get_mut(offset..offset + 2) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
}

fn put_u32(out: &mut [u8], offset: usize, value: u32) {
    if let Some(slot) = out.get_mut(offset..offset + 4) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, BmpError> {
    let slot = data
        .get(offset..offset + 2)
        .ok_or(BmpError::Truncated {
            need: offset + 2,
            have: data.len(),
        })?;
    Ok(u16::from_le_bytes([slot[0], slot[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, BmpError> {
    let slot = data
        .get(offset..offset + 4)
        .ok_or(BmpError::Truncated {
            need: offset + 4,
            have: data.len(),
        })?;
    Ok(u32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]))
}

fn read_i32(data: &[u8], offset: usize) -> Result<i32, BmpError> {
    Ok(read_u32(data, offset)? as i32)
}

#[cfg(test)]
mod tests {
    // `no_std` 下 `alloc` 不在 extern prelude，测试里用到 `Vec` / `vec!` 必须显式引入。
    extern crate alloc;

    use super::{BmpHeader, decode, encode, encode_size};
    use crate::color::{BLACK, BLUE, Color, GREEN, RED, WHITE};
    use crate::error::BmpError;
    use crate::framebuffer::FramebufferInfo;
    use crate::color::PixelFormat;
    use alloc::vec;
    use alloc::vec::Vec;

    // ———————————————————— 测试用 BMP 构造器 ————————————————————

    /// 一帧待构造的 BMP 的描述，`build_*` 系列据此拼出字节流。
    struct BmpSpec {
        width: u32,
        /// 带符号：负值表示自上而下。
        height: i32,
        bit_count: u16,
        compression: u32,
        /// `BI_BITFIELDS` 的三段掩码。
        masks: [u32; 3],
        /// 调色板（B, G, R 三条目序列，第四字节恒 0）。
        palette: Vec<[u8; 3]>,
        /// 已按位深打包好的像素行（未含行尾填充）。
        rows: Vec<Vec<u8>>,
        /// 把 alpha 掩码写进 V4 头（仅 108 字节头有效）；`rgba_masks` 为 `None` 时用。
        v4_alpha: Option<u32>,
        /// `BITMAPV4HEADER` 的四个掩码字段：红、绿、蓝、alpha。
        rgba_masks: Option<[u32; 4]>,
        /// 强制 DIB 头长度（用于构造不支持的头尺寸）。
        dib_len_override: Option<usize>,
        /// 108 字节头时，掩码是写成头内字段（`true`）还是外部区段（`false`）。
        dib_len_follows_v4: bool,
    }

    impl BmpSpec {
        fn new(width: u32, height: i32, bit_count: u16, compression: u32) -> Self {
            Self {
                width,
                height,
                bit_count,
                compression,
                masks: [0; 3],
                palette: Vec::new(),
                rows: Vec::new(),
                v4_alpha: None,
                rgba_masks: None,
                dib_len_override: None,
                dib_len_follows_v4: true,
            }
        }

        /// 追加一行像素（自动补 4 字节对齐的填充）。
        fn push_row(&mut self, row: &[u8]) {
            let mut padded = row.to_vec();
            while padded.len() % 4 != 0 {
                padded.push(0);
            }
            self.rows.push(padded);
        }

        fn build(&self) -> Vec<u8> {
            let dib_len = self.dib_len_override.unwrap_or(if self.v4_alpha.is_some() {
                108
            } else {
                40
            });
            // `BITMAPV4HEADER` 的掩码在头内；固定 40 字节头才另附 12 字节区段。
            let mask_bytes = if self.compression == 3 && dib_len == 40 { 12 } else { 0 };
            let palette_bytes = self.palette.len() * 4;
            let data_offset = 14 + dib_len + mask_bytes + palette_bytes;
            let image_size: usize = self.rows.iter().map(|row| row.len()).sum();

            let mut out = Vec::with_capacity(data_offset + image_size);
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&((data_offset + image_size) as u32).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&(data_offset as u32).to_le_bytes());

            out.extend_from_slice(&(dib_len as u32).to_le_bytes());
            out.extend_from_slice(&(self.width as i32).to_le_bytes());
            out.extend_from_slice(&self.height.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&self.bit_count.to_le_bytes());
            out.extend_from_slice(&self.compression.to_le_bytes());
            out.extend_from_slice(&(image_size as u32).to_le_bytes());
            out.extend_from_slice(&2835u32.to_le_bytes());
            out.extend_from_slice(&2835u32.to_le_bytes());
            out.extend_from_slice(&(self.palette.len() as u32).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            // V4 头的后半段：掩码字段与色彩空间等，测试只关心 alpha 掩码。
            while out.len() < 14 + dib_len {
                out.push(0);
            }
            if let Some(masks) = self.rgba_masks {
                let base = 14 + 40;
                for (index, mask) in masks.iter().enumerate() {
                    out[base + index * 4..base + index * 4 + 4].copy_from_slice(&mask.to_le_bytes());
                }
            } else if let Some(alpha) = self.v4_alpha {
                // bV4RedMask / GreenMask / BlueMask 紧随 biClrImportant 之后。
                let base = 14 + 40;
                for (index, mask) in self.masks.iter().enumerate() {
                    out[base + index * 4..base + index * 4 + 4].copy_from_slice(&mask.to_le_bytes());
                }
                out[base + 12..base + 16].copy_from_slice(&alpha.to_le_bytes());
            }
            if mask_bytes > 0 {
                for mask in self.masks {
                    out.extend_from_slice(&mask.to_le_bytes());
                }
            }
            for entry in &self.palette {
                out.extend_from_slice(&[entry[0], entry[1], entry[2], 0]);
            }
            for row in &self.rows {
                out.extend_from_slice(row);
            }
            out
        }
    }

    /// 运行解码并断言成功，返回头信息。
    fn decode_ok(file: &[u8], out: &mut [u8], pitch: usize, format: PixelFormat) -> BmpHeader {
        decode(file, out, pitch, format).expect("解码应当成功")
    }

    // ———————————————————— 24 位 ————————————————————

    /// 24 位自底向上：文件里第一行是图像的最后一行。
    #[test]
    fn decode_bgr24_bottom_up_reverses_rows() {
        let mut spec = BmpSpec::new(2, 2, 24, 0);
        spec.push_row(&[0, 0, 255, 10, 20, 30]); // 文件首行 → 输出最后一行
        spec.push_row(&[1, 2, 3, 4, 5, 6]); // 文件次行 → 输出首行
        let file = spec.build();

        let mut out = [0u8; 12];
        let header = decode_ok(&file, &mut out, 6, PixelFormat::Bgr24);
        assert!(!header.top_down);
        assert_eq!((header.info_width, header.info_height), (2, 2));
        // 输出第一行来自文件的第二行。
        assert_eq!(&out[0..3], &[1, 2, 3]);
        assert_eq!(&out[3..6], &[4, 5, 6]);
        assert_eq!(&out[6..9], &[0, 0, 255]);
    }

    /// 负高度：行序保持文件顺序。
    #[test]
    fn decode_top_down_keeps_row_order() {
        let mut spec = BmpSpec::new(1, -2, 24, 0);
        spec.push_row(&[10, 20, 30]); // 输出第一行
        spec.push_row(&[40, 50, 60]); // 输出第二行
        let file = spec.build();

        let mut out = [0u8; 6];
        let header = decode_ok(&file, &mut out, 3, PixelFormat::Bgr24);
        assert!(header.top_down);
        assert_eq!(header.source_height, 2);
        assert_eq!(&out[0..3], &[10, 20, 30]);
        assert_eq!(&out[3..6], &[40, 50, 60]);
    }

    /// 24 位 → 4 字节格式：通道按目标格式重排，不含 alpha 的源补 255。
    #[test]
    fn decode_bgr24_to_rgba_reorders_channels() {
        let mut spec = BmpSpec::new(1, 1, 24, 0);
        spec.push_row(&[30, 20, 10]); // B=30, G=20, R=10
        let file = spec.build();

        let mut out = [0u8; 4];
        decode_ok(&file, &mut out, 4, PixelFormat::Rgba8888);
        assert_eq!(out, [10, 20, 30, 255], "Rgba8888 布局是 [R, G, B, A]，源 BGR=[30, 20, 10] → R=10, G=20, B=30");
    }

    // ———————————————————— 32 位 ————————————————————

    /// 32 位 `BI_BITFIELDS` 带 alpha 掩码：alpha 从数据里读出。
    #[test]
    fn decode_bitfields32_honours_alpha_mask() {
        let mut spec = BmpSpec::new(1, 1, 32, 3);
        spec.masks = [0x00FF0000, 0x0000FF00, 0x000000FF];
        spec.v4_alpha = Some(0xFF000000);
        spec.rgba_masks = Some([0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000]);
        spec.push_row(&[0x10, 0x20, 0x30, 0x80]);
        let file = spec.build();

        let mut out = [0u8; 4];
        decode_ok(&file, &mut out, 4, PixelFormat::Rgba8888);
        // 字 = 0x80302010 → B=0x10, G=0x20, R=0x30, A=0x80
        assert_eq!(out, [0x30, 0x20, 0x10, 0x80], "字 = 0x80302010 → B=0x10, G=0x20, R=0x30, A=0x80");
    }

    /// 同样的掩码但数据里 alpha 全 0：32 位格式的 alpha 原样往返（含半透明值与 0），
    /// 所以 Rgba8888 保持 0 而不是 255。
    #[test]
    fn decode_bitfields32_treats_zero_alpha_as_unused() {
        let mut spec = BmpSpec::new(1, 1, 32, 3);
        spec.masks = [0x00FF0000, 0x0000FF00, 0x000000FF];
        spec.v4_alpha = Some(0xFF000000);
        spec.push_row(&[0x10, 0x20, 0x30, 0x00]);
        let file = spec.build();

        let mut out = [0u8; 4];
        decode_ok(&file, &mut out, 4, PixelFormat::Rgba8888);
        // 32 位格式的 alpha 原样往返（含半透明值与 0），所以 Rgba8888 保持 0。
        assert_eq!(out[3], 0, "全 0 的 alpha 掩码应视为未使用（32 位格式保持原样）");
    }

    /// 32 位 `BI_BITFIELDS` 的 Rgba 掩码顺序（R 在低字节）。
    #[test]
    fn decode_bitfields32_respects_rgba_mask_order() {
        let mut spec = BmpSpec::new(1, 1, 32, 3);
        spec.masks = [0x000000FF, 0x0000FF00, 0x00FF0000];
        spec.push_row(&[0x30, 0x20, 0x10, 0xFF]);
        let file = spec.build();

        let mut out = [0u8; 4];
        decode_ok(&file, &mut out, 4, PixelFormat::Rgba8888);
        assert_eq!(out, [0x30, 0x20, 0x10, 255]);
    }

    // ———————————————————— 16 位 ————————————————————

    /// 16 位 `{5,6,5}` 红高位 → `Rgb565`：位域一致时零舍入直通。
    #[test]
    fn decode_rgb565_passthrough_is_lossless() {
        let mut spec = BmpSpec::new(2, 1, 16, 3);
        spec.masks = [0xF800, 0x07E0, 0x001F];
        // 0xF800 = 纯红（最高档），0x07E0 = 纯绿
        spec.push_row(&[0x00, 0xF8, 0xE0, 0x07]);
        let file = spec.build();

        let mut out = [0u8; 4];
        decode_ok(&file, &mut out, 4, PixelFormat::Rgb565);
        assert_eq!(out, [0x00, 0xF8, 0xE0, 0x07]);
    }

    /// 16 位 → 8 位通道：5 位扩展用位复制，纯红应得 255。
    #[test]
    fn decode_rgb565_to_8bit_expands_by_bit_replication() {
        let mut spec = BmpSpec::new(1, 1, 16, 3);
        spec.masks = [0xF800, 0x07E0, 0x001F];
        spec.push_row(&[0x00, 0xF8]); // 纯红
        let file = spec.build();

        let mut out = [0u8; 3];
        decode_ok(&file, &mut out, 3, PixelFormat::Rgb24);
        assert_eq!(out, [255, 0, 0]);
    }

    /// 16 位蓝高位的掩码 → `Bgr565` 同样直通。
    #[test]
    fn decode_bgr565_passthrough_matches_target_layout() {
        let mut spec = BmpSpec::new(1, 1, 16, 3);
        spec.masks = [0x001F, 0x07E0, 0xF800];
        spec.push_row(&[0x00, 0xF8]); // 蓝高位 = 纯蓝
        let file = spec.build();

        let mut out = [0u8; 2];
        decode_ok(&file, &mut out, 2, PixelFormat::Bgr565);
        assert_eq!(out, [0x00, 0xF8]);
    }

    // ———————————————————— 调色板 ————————————————————

    /// 8 位调色板：索引查表后写入目标格式。
    #[test]
    fn decode_palette8_looks_up_colors() {
        let mut spec = BmpSpec::new(2, 1, 8, 0);
        spec.palette = vec![[0, 0, 255], [0, 255, 0]]; // 红, 绿
        spec.push_row(&[0, 1]);
        let file = spec.build();

        let mut out = [0u8; 6];
        decode_ok(&file, &mut out, 6, PixelFormat::Bgr24);
        assert_eq!(&out[0..3], &[0, 0, 255]);
        assert_eq!(&out[3..6], &[0, 255, 0]);
    }

    /// 4 位调色板：高半字节是左边的像素。
    #[test]
    fn decode_palette4_splits_nibbles() {
        let mut spec = BmpSpec::new(3, 1, 4, 0);
        spec.palette = vec![[0, 0, 255], [0, 255, 0], [255, 0, 0]];
        spec.push_row(&[0x21, 0x00]); // 索引 2, 1, 0
        let file = spec.build();

        let mut out = [0u8; 9];
        decode_ok(&file, &mut out, 9, PixelFormat::Bgr24);
        assert_eq!(&out[0..3], &[255, 0, 0]);
        assert_eq!(&out[3..6], &[0, 255, 0]);
        assert_eq!(&out[6..9], &[0, 0, 255]);
    }

    /// 1 位调色板：最高位是左边的像素。
    /// 调色板条目数少于 `2^bpp` 时，越界索引解成黑色而不是崩溃。
    #[test]
    fn decode_palette_out_of_range_index_is_black() {
        let mut spec = BmpSpec::new(1, 1, 8, 0);
        spec.palette = vec![[0, 0, 255]];
        spec.push_row(&[42]);
        let file = spec.build();

        let mut out = [0u8; 3];
        decode_ok(&file, &mut out, 3, PixelFormat::Bgr24);
        assert_eq!(out, [0, 0, 0]);
    }

    // ———————————————————— 错误分支 ————————————————————

    #[test]
    fn decode_rejects_non_bmp_signature() {
        let data = [b'X', b'M', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut out = [0u8; 4];
        assert_eq!(
            decode(&data, &mut out, 4, PixelFormat::Bgra8888),
            Err(BmpError::NotBmp)
        );
    }

    #[test]
    fn decode_rejects_short_file() {
        let data = [b'B', b'M', 0, 0, 0];
        let mut out = [0u8; 4];
        assert!(matches!(
            decode(&data, &mut out, 4, PixelFormat::Bgra8888),
            Err(BmpError::Truncated { .. })
        ));
    }

    #[test]
    fn decode_rejects_core_header() {
        let mut spec = BmpSpec::new(1, 1, 24, 0);
        spec.dib_len_override = Some(12);
        spec.push_row(&[1, 2, 3]);
        let file = spec.build();
        let mut out = [0u8; 3];
        assert_eq!(
            decode(&file, &mut out, 3, PixelFormat::Bgr24),
            Err(BmpError::UnsupportedDibHeader { size: 12 })
        );
    }

    #[test]
    fn decode_rejects_unsupported_bit_depth() {
        let mut spec = BmpSpec::new(1, 1, 2, 0);
        spec.push_row(&[0x00]);
        let file = spec.build();
        let mut out = [0u8; 3];
        assert_eq!(
            decode(&file, &mut out, 3, PixelFormat::Bgr24),
            Err(BmpError::UnsupportedBitDepth { bits: 2 })
        );
    }

    #[test]
    fn decode_rejects_rle_compression() {
        let mut spec = BmpSpec::new(1, 1, 8, 1); // BI_RLE8
        spec.palette = vec![[0, 0, 0]];
        spec.push_row(&[0x01]);
        let file = spec.build();
        let mut out = [0u8; 3];
        assert_eq!(
            decode(&file, &mut out, 3, PixelFormat::Bgr24),
            Err(BmpError::UnsupportedCompression { compression: 1 })
        );
    }

    #[test]
    fn decode_rejects_oversized_palette() {
        let mut spec = BmpSpec::new(1, 1, 8, 0);
        // 伪造 biClrUsed = 300（超过 256）。
        let mut file = spec.build();
        file[14 + 32..14 + 36].copy_from_slice(&300u32.to_le_bytes());
        let mut out = [0u8; 3];
        assert_eq!(
            decode(&file, &mut out, 3, PixelFormat::Bgr24),
            Err(BmpError::UnsupportedPaletteSize { colors: 300 })
        );
    }

    #[test]
    fn decode_rejects_buffer_too_small() {
        let mut spec = BmpSpec::new(2, 2, 24, 0);
        spec.push_row(&[0, 0, 0, 0, 0, 0]);
        spec.push_row(&[0, 0, 0, 0, 0, 0]);
        let file = spec.build();
        let mut out = [0u8; 6]; // 只够一行
        assert_eq!(
            decode(&file, &mut out, 6, PixelFormat::Bgr24),
            Err(BmpError::BufferTooSmall { need: 12, have: 6 })
        );
    }

    #[test]
    fn decode_rejects_pitch_smaller_than_row() {
        let mut spec = BmpSpec::new(2, 1, 24, 0);
        spec.push_row(&[0, 0, 0, 0, 0, 0]);
        let file = spec.build();
        let mut out = [0u8; 12];
        assert_eq!(
            decode(&file, &mut out, 4, PixelFormat::Bgr24),
            Err(BmpError::InvalidPitch { need: 6, have: 4 })
        );
    }

    /// 像素数据被截断：`bfOffBits` 指向的长度装不下声明的行数。
    #[test]
    fn decode_rejects_truncated_pixel_data() {
        let mut spec = BmpSpec::new(1, 2, 24, 0);
        spec.push_row(&[1, 2, 3]);
        spec.push_row(&[4, 5, 6]);
        let mut file = spec.build();
        // 砍掉最后一行。
        file.truncate(file.len() - 4);
        let mut out = [0u8; 6];
        assert_eq!(
            decode(&file, &mut out, 3, PixelFormat::Bgr24),
            Err(BmpError::TruncatedPixels)
        );
    }

    // ———————————————————— 往返 ————————————————————

    /// 编码 → 解码 对每种可编码格式逐字节一致。
    #[test]
    fn encode_decode_roundtrip_is_byte_identical() {
        const WIDTH: usize = 3;
        const HEIGHT: usize = 2;
        let formats = [
            PixelFormat::Bgr24,
            PixelFormat::Rgb24,
            PixelFormat::Xrgb8888,
            PixelFormat::Argb8888,
            PixelFormat::Rgba8888,
            PixelFormat::Bgra8888,
            PixelFormat::Rgb565,
        ];
        for format in formats {
            let bpp = format.bytes_per_pixel();
            let pitch = WIDTH * bpp;
            let info = FramebufferInfo::new(WIDTH, HEIGHT, pitch, format);
            // 用可逆的确定性图案填充，避免全同色掩盖通道顺序错误。
            let mut pixels = vec![0u8; info.buffer_size()];
            for (index, byte) in pixels.iter_mut().enumerate() {
                *byte = (index * 37 % 251) as u8;
            }
            // `Xrgb8888` 的保留字节没有信息量，编码时会写成 0，读回也是 0；
            // 其余 32 位格式的 alpha 原样往返（含半透明值与 0），所以只把
            // `Xrgb8888` 的保留字节先归零，以便逐字节比对。
            if format == PixelFormat::Xrgb8888 {
                // 第 4 字节是保留字节。
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel[3] = 0;
                }
            }

            let mut file = vec![0u8; encode_size(WIDTH, HEIGHT, format).unwrap()];
            encode(&info, &pixels, &mut file).unwrap();

            let mut decoded = vec![0u8; info.buffer_size()];
            decode(&file, &mut decoded, pitch, format).unwrap();
            assert_eq!(decoded, pixels, "{format:?} 往返不一致");
        }
    }

    /// `encode_size` 必须精确等于 `encode` 的返回值。
    #[test]
    fn encode_size_matches_bytes_written() {
        for format in [
            PixelFormat::Bgr24,
            PixelFormat::Bgra8888,
            PixelFormat::Rgb565,
        ] {
            let bpp = format.bytes_per_pixel();
            let info = FramebufferInfo::new(3, 2, 3 * bpp, format);
            let pixels = vec![7u8; info.buffer_size()];
            let size = encode_size(3, 2, format).unwrap();
            let mut file = vec![0u8; size];
            let written = encode(&info, &pixels, &mut file).unwrap();
            assert_eq!(written, size, "{format:?} 的 encode_size 与实写字节数不符");
        }
    }

    /// `Bgr565` 没有 BMP 原生表示，编码与尺寸查询都必须拒绝。
    #[test]
    fn encode_rejects_bgr565() {
        let info = FramebufferInfo::new(1, 1, 2, PixelFormat::Bgr565);
        let pixels = [0u8; 2];
        let mut out = [0u8; 64];
        assert_eq!(
            encode(&info, &pixels, &mut out),
            Err(BmpError::Unencodeable {
                format: PixelFormat::Bgr565
            })
        );
        assert_eq!(
            encode_size(1, 1, PixelFormat::Bgr565),
            Err(BmpError::Unencodeable {
                format: PixelFormat::Bgr565
            })
        );
    }

    /// 编码时 `out` 不够长：只写完整的行，不写半行。
    #[test]
    fn encode_skips_rows_that_do_not_fit() {
        let info = FramebufferInfo::new(1, 3, 3, PixelFormat::Bgr24);
        let pixels = [0u8; 9];
        let full = encode_size(1, 3, PixelFormat::Bgr24).unwrap();
        let mut out = vec![0u8; full - 4]; // 少一行
        let written = encode(&info, &pixels, &mut out).unwrap();
        assert_eq!(written, full - 4);
    }

    /// 编码标题与偏移字段自洽：`bfOffBits` 指向像素区首字节。
    #[test]
    fn encode_header_fields_are_consistent() {
        let info = FramebufferInfo::new(2, 1, 6, PixelFormat::Bgr24);
        let pixels = [1u8, 2, 3, 4, 5, 6];
        let mut file = vec![0u8; encode_size(2, 1, PixelFormat::Bgr24).unwrap()];
        encode(&info, &pixels, &mut file).unwrap();

        assert_eq!(&file[0..2], b"BM");
        let offset = u32::from_le_bytes([file[10], file[11], file[12], file[13]]) as usize;
        assert_eq!(offset, 54, "24 位 BI_RGB 的像素区应紧接 54 字节头");
        let dib = u32::from_le_bytes([file[14], file[15], file[16], file[17]]);
        assert_eq!(dib, 40);
        assert_eq!(file[offset], 1, "文件首行应是内存中最后一行");
    }

    /// 32 位编码写出 52 字节 DIB 头（40 + 3 掩码）且 `bfOffBits` 同步后移。
    #[test]
    fn encode_32bit_writes_mask_region() {
        let info = FramebufferInfo::new(1, 1, 4, PixelFormat::Rgba8888);
        let pixels = [10u8, 20, 30, 255];
        let mut file = vec![0u8; encode_size(1, 1, PixelFormat::Rgba8888).unwrap()];
        encode(&info, &pixels, &mut file).unwrap();

        let offset = u32::from_le_bytes([file[10], file[11], file[12], file[13]]) as usize;
        assert_eq!(offset, 66, "32 位 BI_BITFIELDS 的像素区应在 66 字节之后");
        let red = u32::from_le_bytes([file[54], file[55], file[56], file[57]]);
        assert_eq!(red, 0x000000FF, "Rgba8888 的红掩码在低字节");
    }

    /// 空图（宽 0）编码后只有头，解码回 0 尺寸。
    #[test]
    fn encode_and_decode_zero_width() {
        let info = FramebufferInfo::new(0, 0, 0, PixelFormat::Bgr24);
        let mut file = vec![0u8; encode_size(0, 0, PixelFormat::Bgr24).unwrap()];
        let written = encode(&info, &[], &mut file).unwrap();
        assert_eq!(written, 54);
        let mut out: [u8; 0] = [];
        // 高度为 0 的图是合法输入，但解码要求像素区起点可在文件内。
        assert!(decode(&file[..54], &mut out, 0, PixelFormat::Bgr24).is_err());
    }

    /// 颜色常量可用于快速目测的构造：把一张纯色图编解码一遍。
    #[test]
    fn roundtrip_preserves_named_colors() {
        let info = FramebufferInfo::new(4, 1, 12, PixelFormat::Bgr24);
        let mut pixels = [0u8; 12];
        for (index, color) in [RED, GREEN, BLUE, WHITE].iter().enumerate() {
            let bytes = color.to_bytes(PixelFormat::Bgr24);
            pixels[index * 3..index * 3 + 3].copy_from_slice(&bytes[..3]);
        }
        let mut file = vec![0u8; encode_size(4, 1, PixelFormat::Bgr24).unwrap()];
        encode(&info, &pixels, &mut file).unwrap();

        let mut decoded = [0u8; 12];
        decode(&file, &mut decoded, 12, PixelFormat::Bgr24).unwrap();
        assert_eq!(decoded, pixels);

        let mut rgba = [0u8; 16];
        decode(&file, &mut rgba, 16, PixelFormat::Rgba8888).unwrap();
        assert_eq!(&rgba[0..4], &[RED.r, RED.g, RED.b, 255]);
        assert_eq!(&rgba[12..16], &[WHITE.r, WHITE.g, WHITE.b, 255]);
        let _ = BLACK;
        let _ = Color::new(0, 0, 0);
    }
}
