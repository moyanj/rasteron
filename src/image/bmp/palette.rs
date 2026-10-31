//! 调色板读取与索引展开。
//!
//! 1 / 4 / 8 位 BMP 的像素是调色板索引，必须先查表再换成目标格式。这里的三件事
//! 都只做只读查表，不触碰任何缓冲区：
//!
//! 1. [`Palette::locate`] 从文件里算出调色板真正的位置；
//! 2. [`Palette::color`] 取出某个索引对应的颜色；
//! 3. [`Palette::decode_row`] 把一整行的打包索引展开成目标格式。
//!
//! 调色板位置**不从 `bfOffBits` 反推**：那个字段描述的是像素区起点，而调色板
//! 必然紧跟在 DIB 头（及可能的掩码区）之后。有的写入器会把 `bfOffBits` 写得比
//! 实际偏大（为了对齐或插了一段无用字节），按它反推会读到像素数据上，把索引
//! 翻译成垃圾颜色。
//!
//! # 例
//!
//! ```
//! use rasteron::color::PixelFormat;
//! use rasteron::image::bmp::Palette;
//!
//! // 两个条目，每项「B, G, R, 0」：红、绿。
//! let file = [0u8, 0, 255, 0, 0, 255, 0, 0];
//! let palette = Palette::new(&file, 0, 2);
//! assert_eq!(palette.color(0).r, 255);
//! assert_eq!(palette.color(1).g, 255);
//! // 越界索引不 panic，按黑色处理。
//! assert_eq!(palette.color(9).r, 0);
//!
//! // 1bpp 的一行：最高位是最左边的像素，索引 1, 0 → 绿、红。
//! let mut row = [0u8; 6];
//! palette.decode_row(1, &[0b1000_0000], &mut row, PixelFormat::Bgr24);
//! assert_eq!(&row[0..3], &[0, 255, 0]);
//! assert_eq!(&row[3..6], &[0, 0, 255]);
//! ```

use crate::color::{Color, PixelFormat};

use super::store_color;

/// 调色板在文件中的位置与条目数。
#[derive(Debug, Clone, Copy)]
pub struct Palette<'a> {
    /// 整份文件数据，条目在这里面按需切。
    data: &'a [u8],
    /// 第 0 条的首字节偏移。
    offset: usize,
    /// 条目数。
    colors: usize,
}

impl<'a> Palette<'a> {
    /// 用给定的偏移与条目数构造。
    ///
    /// 偏移落在文件之外时不 panic：后续 [`Palette::color`](#method.color) 会
    /// 把每次取值都当作越界并返回黑色，于是损坏文件解出一张黑图，而不是让
    /// 整帧解码失败。
    pub const fn new(data: &'a [u8], offset: usize, colors: usize) -> Self {
        Self {
            data,
            offset,
            colors,
        }
    }

    /// 条目数。
    pub const fn len(&self) -> usize {
        self.colors
    }

    /// 是否没有条目（24/32 位图恒为真）。
    pub const fn is_empty(&self) -> bool {
        self.colors == 0
    }

    /// 读取第 `index` 项；越界或数据不足时返回黑色。
    ///
    /// 返回黑色而不是错误：一个坏索引不该让整帧失败。真实文件里出现越界索引
    /// 只可能是文件损坏或 `biClrUsed` 写错。
    pub fn color(&self, index: u32) -> Color {
        let index = index as usize;
        if index >= self.colors {
            return Color::new(0, 0, 0);
        }
        let start = match self
            .offset
            .checked_add(index.saturating_mul(super::PALETTE_ENTRY_LEN))
        {
            Some(value) => value,
            None => return Color::new(0, 0, 0),
        };
        match self
            .data
            .get(start..start + super::PALETTE_ENTRY_LEN)
        {
            // 条目是 B, G, R, 0。
            Some(entry) => Color::new(entry[2], entry[1], entry[0]),
            None => Color::new(0, 0, 0),
        }
    }

    /// 把一行的打包索引展开成 `target` 里的目标格式像素。

    ///
    /// * `bits` — 1 / 4 / 8，每像素占用的位数。
    /// * `source` — 文件里的一整行（已去掉行尾填充）。
    /// * `target` — 输出行，长度至少是这一行的目标字节数。
    /// * `format` — 目标像素格式。
    ///
    /// 调用方保证 `bits` 已由 [`super::parse_header`] 的白名单校验过。
    pub fn decode_row(&self, bits: u32, source: &[u8], target: &mut [u8], format: PixelFormat) {
        let bpp = format.bytes_per_pixel();
        let pixel_count = target.len() / bpp;

        match bits {
            8 => {
                let count = source.len().min(pixel_count);
                for index in 0..count {
                    store_color(target, index * bpp, format, self.color(u32::from(source[index])));
                }
            }
            4 => {
                // 每字节两个像素，高半字节是左边的那个。
                let packed = pixel_count.div_ceil(2);
                for (index, byte) in source.iter().take(packed).enumerate() {
                    let high = u32::from(byte >> 4);
                    let low = u32::from(byte & 0x0F);
                    store_color(target, index * 2 * bpp, format, self.color(high));
                    if index * 2 + 1 < pixel_count {
                        store_color(target, (index * 2 + 1) * bpp, format, self.color(low));
                    }
                }
            }
            _ => {
                // 1bpp：每字节八个像素，最高位是最左边的那个。
                let packed = pixel_count.div_ceil(8);
                for (index, byte) in source.iter().take(packed).enumerate() {
                    for bit in 0..8 {
                        let pixel = index * 8 + bit;
                        if pixel >= pixel_count {
                            break;
                        }
                        let value = u32::from((byte >> (7 - bit)) & 1);
                        store_color(target, pixel * bpp, format, self.color(value));
                    }
                }
            }
        }
    }
}
