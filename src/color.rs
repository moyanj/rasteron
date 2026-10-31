use crate::error::ColorParseError;

/// 具有 8 位通道的 RGBA 颜色。
///
/// 大多数绘制方法接受 `&Color`，以便在热路径中做出零成本的混合决策，
/// 同时避免强制调用方进行克隆。
///
/// # 示例
///
/// ```
/// # use rasteron::color::Color;
/// let opaque = Color::new(0xFF, 0x00, 0x00);              // 红色
/// let semi   = Color::with_alpha(0x00, 0x00, 0xFF, 0x80); // 蓝色，50% 透明度
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Color {
    /// 红色通道。
    pub r: u8,
    /// 绿色通道。
    pub g: u8,
    /// 蓝色通道。
    pub b: u8,
    /// Alpha 通道（`0` = 完全透明，`255` = 完全不透明）。
    pub a: u8,
}

impl Color {
    /// 从 RGB 通道创建不透明颜色（alpha 默认为 `255`）。
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// 从 RGBA 通道创建颜色。
    pub const fn with_alpha(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 从十六进制字符串解析颜色。
    ///
    /// 支持的格式：
    ///
    /// | 格式 | 示例 |
    /// |------|------|
    /// | `#RRGGBB` | `"#FF0000"` → 不透明红色 |
    /// | `#RRGGBBAA` | `"#FF000080"` → 红色，50% 透明度 |
    ///
    /// 开头的 `#` 是可选的。
    ///
    /// # 错误
    ///
    /// 如果十六进制字符串（去除 `#` 后）长度不是 6 或 8 个字符，返回
    /// [`ColorParseError::InvalidLength`]；
    /// 如果包含无效的十六进制字符，返回 [`ColorParseError::InvalidDigit`]。
    pub fn from_hex(hex: &str) -> Result<Self, ColorParseError> {
        let hex = hex.trim_start_matches('#');

        if hex.len() != 6 && hex.len() != 8 {
            return Err(ColorParseError::InvalidLength);
        }

        fn parse_channel(hex: &str) -> Result<u8, ColorParseError> {
            u8::from_str_radix(hex, 16).map_err(|_| ColorParseError::InvalidDigit)
        }

        let r = parse_channel(&hex[0..2])?;
        let g = parse_channel(&hex[2..4])?;
        let b = parse_channel(&hex[4..6])?;

        if hex.len() == 8 {
            let a = parse_channel(&hex[6..8])?;
            Ok(Self::with_alpha(r, g, b, a))
        } else {
            Ok(Self::new(r, g, b))
        }
    }

    /// 将颜色编码为 `R24G16B8A0` 格式的 `u32`。
    ///
    /// 仅用于与外部系统交换；内部帧缓冲写入请使用 [`to_bytes`](Color::to_bytes)。
    pub fn to_rgba_u32(&self) -> u32 {
        let r = self.r as u32;
        let g = self.g as u32;
        let b = self.b as u32;
        let a = self.a as u32;
        (r << 24) | (g << 16) | (b << 8) | a
    }

    /// 从 `R24G16B8A0` 格式的 `u32` 解码颜色。
    pub fn from_rgba_u32(color: u32) -> Self {
        let r = ((color >> 24) & 0xFF) as u8;
        let g = ((color >> 16) & 0xFF) as u8;
        let b = ((color >> 8) & 0xFF) as u8;
        let a = (color & 0xFF) as u8;
        Self::with_alpha(r, g, b, a)
    }

    /// 将颜色按指定像素格式编码为字节数组。
    ///
    /// 返回一个 4 字节数组；对于 3 字节格式，最后一个字节未使用；对于
    /// 2 字节的 565 格式，高两字节恒为 0，有效字节按该格式的小端序排列
    /// （两个 565 变体都是小端；红蓝的位置差异在位域而非字节序）。565 的通道
    /// 会被量化到 5/6/5 位。
    #[inline(always)]
    pub fn to_bytes(&self, format: PixelFormat) -> [u8; 4] {
        // 565 的通道不足 8 位，无法用「8 位通道左移 shift」的通用写法表达。
        if format.bytes_per_pixel() == 2 {
            let bytes = encode_565_bytes(
                pack_565(self, format.layout_565().expect("2bpp 必是 565 变体")),
                format.stores_little_endian(),
            );
            return [bytes[0], bytes[1], 0, 0];
        }

        let mut value = 0u32;
        value |= (self.r as u32) << format.red_shift();
        value |= (self.g as u32) << format.green_shift();
        value |= (self.b as u32) << format.blue_shift();

        if let Some(alpha_shift) = format.alpha_shift() {
            value |= (self.a as u32) << alpha_shift;
        }

        value.to_le_bytes()
    }

    /// 从按指定像素格式编码的字节数组解码颜色。
    ///
    /// 565 格式会把 5/6/5 位字段扩展回 8 位（位复制），因此解出的通道值不是
    /// 编码前的任意 8 位值，而是「量化后再扩展」的规范值。
    ///
    /// # 恐慌
    ///
    /// 如果 `bytes` 长度小于 `format.bytes_per_pixel()`，将会恐慌。
    #[inline(always)]
    pub fn from_bytes(format: PixelFormat, bytes: &[u8]) -> Self {
        debug_assert!(
            bytes.len() >= format.bytes_per_pixel(),
            "not enough bytes for pixel format {:?}",
            format
        );

        if format.bytes_per_pixel() == 2 {
            let layout = format.layout_565().expect("2bpp 必是 565 变体");
            return unpack_565(
                decode_565_bytes(bytes, format.stores_little_endian()),
                layout,
            );
        }

        let mut value = 0u32;
        for (index, byte) in bytes.iter().take(format.bytes_per_pixel()).enumerate() {
            value |= (*byte as u32) << (index * 8);
        }

        let r = ((value >> format.red_shift()) & 0xFF) as u8;
        let g = ((value >> format.green_shift()) & 0xFF) as u8;
        let b = ((value >> format.blue_shift()) & 0xFF) as u8;
        let a = format
            .alpha_shift()
            .map(|shift| ((value >> shift) & 0xFF) as u8)
            .unwrap_or(255);

        Self::with_alpha(r, g, b, a)
    }

    /// 替换颜色的 alpha 值，保留 RGB 通道不变。
    pub fn mix_alpha(&self, alpha: u8) -> Self {
        Self::with_alpha(self.r, self.g, self.b, alpha)
    }

    /// 将当前颜色与另一个颜色做标准 Alpha 混合，透明度由 `alpha` 控制。
    ///
    /// 逐通道委托 [`PixelFormat::blend_channel`]，与固定功能路径的字节层实现
    /// （[`PixelFormat::write_color`]）**共用同一公式**：
    ///
    /// ```text
    /// (src × alpha + dst × (255 − alpha) + 255) >> 8
    /// ```
    ///
    /// 这是对精确 `/255` 的近似，误差 ≤ 1。全库只此一份 Alpha 混合语义——
    /// SWAR 内核、固定功能跨度填充与可编程管线都收敛到这里，因此同一次混合
    /// 在任意路径上得到逐位相同的像素。
    ///
    /// 返回值恒为不透明（`alpha == 255`）：混合结果的 alpha 通道从不落盘，
    /// 4bpp 格式写入的是格式自身的 opaque alpha 字节。
    ///
    /// # 示例
    ///
    /// ```
    /// use rasteron::color::Color;
    ///
    /// let src = Color::new(255, 0, 0);
    /// let dst = Color::new(0, 0, 255);
    /// // alpha = 128 时结果落在两色之间。
    /// assert_eq!(src.blend(&dst, 128), Color::new(128, 0, 127));
    /// ```
    #[inline(always)]
    pub fn blend(&self, other: &Color, alpha: u8) -> Self {
        let inv_alpha = 255 - alpha;
        let channel =
            |src: u8, dst: u8| PixelFormat::blend_channel(src, dst, alpha as u16, inv_alpha as u16);
        Color::new(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
        )
    }

    /// 加法混合：各通道饱和相加（`min(src + dst, 255)`），常用于发光/叠加。
    ///
    /// 仅供可编程管线的 `BlendMode::Add` 使用，结果恒为不透明。
    #[cfg(feature = "programmable")]
    pub(crate) fn blend_add(self, dst: &Color) -> Color {
        Color::new(
            self.r.saturating_add(dst.r),
            self.g.saturating_add(dst.g),
            self.b.saturating_add(dst.b),
        )
    }

    /// 乘法混合：各通道 `src × dst / 255`（`+127` 四舍五入），用于阴影/染色。
    ///
    /// 仅供可编程管线的 `BlendMode::Multiply` 使用，结果恒为不透明。
    #[cfg(feature = "programmable")]
    pub(crate) fn blend_multiply(self, dst: &Color) -> Color {
        let channel = |src: u8, dst: u8| ((src as u16 * dst as u16 + 127) / 255) as u8;
        Color::new(
            channel(self.r, dst.r),
            channel(self.g, dst.g),
            channel(self.b, dst.b),
        )
    }

    /// 预乘 Alpha 混合：片元颜色被视为已乘过 `src.a`，故直接加到衰减后的目标上，
    /// 即 `src + dst × (1 - src.a)`，结果饱和到 255。
    ///
    /// 与 [`Color::blend`] 的区别：后者会对源色再乘一次 `alpha`，本函数不会。
    ///
    /// 仅供可编程管线的 `BlendMode::PremultipliedAlpha` 使用，结果恒为不透明。
    #[cfg(feature = "programmable")]
    pub(crate) fn blend_premultiplied(self, dst: &Color) -> Color {
        let inv_alpha = 255 - self.a;
        let channel = |src: u8, dst: u8| {
            src.saturating_add(((dst as u16 * inv_alpha as u16 + 127) / 255) as u8)
        };
        Color::new(
            channel(self.r, dst.r),
            channel(self.g, dst.g),
            channel(self.b, dst.b),
        )
    }

    /// 返回颜色的反色（按位取反 RGB 通道）。
    pub fn invert(&self) -> Color {
        Color::new(!self.r, !self.g, !self.b)
    }
}

/// 像素格式描述，定义了内存中通道的布局和字节对齐。
///
/// | 格式 | 字节数 | 说明 |
/// |------|--------|------|
/// | `Rgb24` | 3 | 红、绿、蓝（小端序），无 alpha |
/// | `Bgr24` | 3 | 蓝、绿、红（小端序），无 alpha |
/// | `Xrgb8888` | 4 | 蓝、绿、红 + 保留字节，无 alpha |
/// | `Argb8888` | 4 | 蓝、绿、红、Alpha |
/// | `Rgba8888` | 4 | 红、绿、蓝、Alpha |
/// | `Bgra8888` | 4 | 蓝、绿、红、Alpha |
/// | `Rgb565` | 2 | 5/6/5 位打包，`R` 在高位，无 alpha |
/// | `Bgr565` | 2 | 5/6/5 位打包，`B` 在高位，无 alpha |
///
/// 前六种格式的通道都是 8 位且按字节对齐，通道位置由
/// [`red_shift`](PixelFormat::red_shift) 等以**位**为单位描述（对它们而言恰好
/// 等于字节位移 × 8）。`Rgb565` / `Bgr565` 的通道宽度分别是 5/6/5 位，故写入时
/// 需要量化、读取时需要扩展，由 [`Color::to_bytes`] /
/// [`read_color`](PixelFormat::read_color) 等入口统一处理；两者的位域差异见
/// `Layout565`（私有，仅内部使用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 3 字节小端序：R, G, B。
    Rgb24,
    /// 3 字节小端序：B, G, R。
    Bgr24,
    /// 4 字节小端序：B, G, R, 保留（通常忽略）。
    Xrgb8888,
    /// 4 字节小端序：B, G, R, A。
    Argb8888,
    /// 4 字节小端序：R, G, B, A。
    Rgba8888,
    /// 4 字节小端序：B, G, R, A。
    Bgra8888,
    /// 2 字节 5/6/5 打包：位域 `R = 15..11`、`G = 10..5`、`B = 4..0`，
    /// 小端序存储（字节 0 = `G[2:0] << 5 | B`，字节 1 = `R << 3 | G[5:3]`）。
    ///
    /// 无 alpha 通道。写入时 8 位通道量化到 5/6/5 位，读取时位复制扩展回 8 位。
    Rgb565,
    /// 2 字节 5/6/5 打包：位域 `B = 15..11`、`G = 10..5`、`R = 4..0`，
    /// 小端序存储（字节 0 = `G[5:3] | R << 5`，字节 1 = `B << 3 | G[2:0]`）。
    Bgr565,
}

impl PixelFormat {
    /// Alpha 混合单通道的计算。
    ///
    /// 使用 `(src * alpha + dst * inv_alpha + 255) >> 8` 近似
    /// 标准 `(src * alpha + dst * inv_alpha) / 255`，误差 ≤ 1。
    #[inline(always)]
    pub fn blend_channel(src: u8, dst: u8, alpha: u16, inv_alpha: u16) -> u8 {
        let v = src as u16 * alpha + dst as u16 * inv_alpha;
        ((v + 255) >> 8) as u8
    }

    #[inline(always)]
    const fn opaque_alpha_byte(self) -> u8 {
        match self {
            PixelFormat::Xrgb8888 => 0,
            // 565 没有 alpha 字节，该值只是穷举占位（bpp != 4，永不被读取）。
            PixelFormat::Rgb24
            | PixelFormat::Bgr24
            | PixelFormat::Argb8888
            | PixelFormat::Rgba8888
            | PixelFormat::Bgra8888
            | PixelFormat::Rgb565
            | PixelFormat::Bgr565 => 255,
        }
    }

    /// 红色通道在 32 位小端字中的位移（**以位为单位**）。
    ///
    /// 例如 BGR 布局下 red 在高 16 位，位移为 16；`Rgb565` 的 red 在 15..11 位，
    /// 位移为 11，而 `Bgr565` 的 red 在低 5 位（4..0），位移为 0。
    /// 对 8 位通道的格式，该值同时是字节位移 × 8。
    #[inline(always)]
    pub const fn red_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 0,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 16,
            // 565：位移由位域布局给出（Rgb565 → 11，Bgr565 → 0）。
            // `u32 → u8` 是自解释的窄化：两个位移都 ≤ 11。
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                layout_565(self).expect("565 变体").red_shift() as u8
            }
        }
    }

    /// 绿色通道的位移（8 位格式均为 8，565 为 5）。
    ///
    /// 绿色恒居位域中央，故两个 565 变体的值相同。
    #[inline(always)]
    pub const fn green_shift(self) -> u8 {
        match self {
            // 绿色恒居位域中央，两个 565 变体同位移。
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => GREEN_SHIFT_565 as u8,
            PixelFormat::Rgb24
            | PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Rgba8888
            | PixelFormat::Bgra8888 => 8,
        }
    }

    /// 蓝色通道在 32 位小端字中的位移（以位为单位）。
    ///
    /// 与 [`red_shift`](PixelFormat::red_shift) 恰好互补：`Bgr565` 的蓝在
    /// 15..11 位（位移 11），`Rgb565` 的蓝在 4..0 位（位移 0）。
    #[inline(always)]
    pub const fn blue_shift(self) -> u8 {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => 16,
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => 0,
            // 565：与 `red_shift` 互补（Rgb565 → 0，Bgr565 → 11）。
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                layout_565(self).expect("565 变体").blue_shift() as u8
            }
        }
    }

    /// Alpha 通道的位移（如果格式支持）。
    ///
    /// 对于不含 alpha 的格式（`Rgb24`、`Bgr24`、`Xrgb8888`、`Rgb565`、`Bgr565`）
    /// 返回 `None`。
    #[inline(always)]
    pub const fn alpha_shift(self) -> Option<u8> {
        match self {
            PixelFormat::Argb8888 | PixelFormat::Rgba8888 | PixelFormat::Bgra8888 => Some(24),
            PixelFormat::Rgb24
            | PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Rgb565
            | PixelFormat::Bgr565 => None,
        }
    }

    /// 每个像素占用的字节数。
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgb24 | PixelFormat::Bgr24 => 3,
            PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Rgba8888
            | PixelFormat::Bgra8888 => 4,
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => 2,
        }
    }

    /// 16 位打包格式是否按「低字节在前」（小端序）存储。
    ///
    /// 只对 [`bytes_per_pixel`](PixelFormat::bytes_per_pixel) 为 2 的格式有意义：
    /// 8 位通道格式的字节顺序已由各自的 shift 完整表达，故恒为 `true`。
    ///
    /// **两个 565 变体都是 `true`**：`Bgr565` 与 `Rgb565` 的差异在位域里的红蓝
    /// 位置（见 [`Layout565`]），**不在**存储字节序。二者均为小端，与
    /// `DRM_FORMAT_RGB565` / `DRM_FORMAT_BGR565` 的定义一致。
    #[inline(always)]
    pub(crate) const fn stores_little_endian(self) -> bool {
        true
    }

    /// 该格式的 565 位域布局（非 565 格式返回 `None`）。
    #[inline(always)]
    pub(crate) const fn layout_565(self) -> Option<Layout565> {
        layout_565(self)
    }

    /// 每个像素占用的位数。
    pub const fn bits_per_pixel(self) -> usize {
        self.bytes_per_pixel() * 8
    }

    /// 将颜色写入目标缓冲区（支持 Alpha 混合）。
    ///
    /// 当 `color.a == 255` 时直接覆盖；否则使用 Alpha 混合。
    ///
    /// 对 565 格式，混合语义是「把目标解码到 8 位 → 用
    /// [`blend_channel`](PixelFormat::blend_channel) 混合 → 量化回 565」，
    /// 与其余格式共用同一套参考公式，误差只来自量化。
    ///
    /// # 调用方保证
    ///
    /// `dst` 长度至少为 `bytes_per_pixel()`。
    #[inline(always)]
    pub fn write_color(self, dst: &mut [u8], color: &Color) {
        debug_assert!(dst.len() >= self.bytes_per_pixel());
        let alpha = color.a as u16;
        if alpha == 255 {
            match self {
                PixelFormat::Rgb24 | PixelFormat::Rgba8888 => {
                    dst[0] = color.r;
                    dst[1] = color.g;
                    dst[2] = color.b;
                    if matches!(self, PixelFormat::Rgba8888) {
                        dst[3] = 255;
                    }
                }
                PixelFormat::Bgr24
                | PixelFormat::Xrgb8888
                | PixelFormat::Argb8888
                | PixelFormat::Bgra8888 => {
                    dst[0] = color.b;
                    dst[1] = color.g;
                    dst[2] = color.r;
                    if self.bytes_per_pixel() == 4 {
                        dst[3] = self.opaque_alpha_byte();
                    }
                }
                // 565 无 alpha 通道，不透明写入即「量化打包」，无需读取目标像素。
                PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                    let bytes = encode_565_bytes(
                        pack_565(color, self.layout_565().expect("565 变体")),
                        self.stores_little_endian(),
                    );
                    dst[0] = bytes[0];
                    dst[1] = bytes[1];
                }
            }
            return;
        }

        let inv_alpha = 255 - alpha;
        match self {
            PixelFormat::Rgb24 | PixelFormat::Rgba8888 => {
                dst[0] = Self::blend_channel(color.r, dst[0], alpha, inv_alpha);
                dst[1] = Self::blend_channel(color.g, dst[1], alpha, inv_alpha);
                dst[2] = Self::blend_channel(color.b, dst[2], alpha, inv_alpha);
                if matches!(self, PixelFormat::Rgba8888) {
                    dst[3] = 255;
                }
            }
            PixelFormat::Bgr24
            | PixelFormat::Xrgb8888
            | PixelFormat::Argb8888
            | PixelFormat::Bgra8888 => {
                dst[0] = Self::blend_channel(color.b, dst[0], alpha, inv_alpha);
                dst[1] = Self::blend_channel(color.g, dst[1], alpha, inv_alpha);
                dst[2] = Self::blend_channel(color.r, dst[2], alpha, inv_alpha);
                if self.bytes_per_pixel() == 4 {
                    dst[3] = self.opaque_alpha_byte();
                }
            }
            // 565 就地混合：解码 → blend_channel → 量化回写。
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => {
                let layout = self.layout_565().expect("565 变体");
                let little_endian = self.stores_little_endian();
                let word = decode_565_bytes(dst, little_endian);
                let bytes = encode_565_bytes(blend_565(word, color, layout), little_endian);
                dst[0] = bytes[0];
                dst[1] = bytes[1];
            }
        }
    }

    /// 从源缓冲区读取一个像素并解码为颜色。
    ///
    /// 对于不含 alpha 的格式，返回的 alpha 为 `255`。
    ///
    /// # 调用方保证
    ///
    /// `src` 长度至少为 `bytes_per_pixel()`。
    #[inline(always)]
    pub fn read_color(self, src: &[u8]) -> Color {
        match self {
            PixelFormat::Rgb24 => Color::new(src[0], src[1], src[2]),
            PixelFormat::Bgr24 => Color::new(src[2], src[1], src[0]),
            PixelFormat::Xrgb8888 => Color::new(src[2], src[1], src[0]),
            PixelFormat::Argb8888 => Color::with_alpha(src[2], src[1], src[0], src[3]),
            PixelFormat::Rgba8888 => Color::with_alpha(src[0], src[1], src[2], src[3]),
            PixelFormat::Bgra8888 => Color::with_alpha(src[2], src[1], src[0], src[3]),
            // 565：按存储字节序取字，再把 5/6/5 字段位复制扩展回 8 位。
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => unpack_565(
                decode_565_bytes(src, self.stores_little_endian()),
                self.layout_565().expect("565 变体"),
            ),
        }
    }
}

/// 便捷宏：从字面量创建 [`Color`]。
///
/// 支持以下调用方式：
///
/// | 形式 | 说明 |
/// |------|------|
/// | `color!(r, g, b)` | 不透明颜色（alpha = 255） |
/// | `color!(r, g, b, a)` | 带透明度的颜色 |
/// | `color!("#RRGGBB")` | 十六进制字符串（不透明） |
/// | `color!("#RRGGBBAA")` | 十六进制字符串（带透明度） |
///
/// # 示例
///
/// ```
/// # use rasteron::{color, color::Color};
/// assert_eq!(color!(255, 0, 0), Color::new(255, 0, 0));
/// assert_eq!(color!("#00FF00"), Color::new(0, 255, 0));
/// ```
#[macro_export]
macro_rules! color {
    // 十六进制颜色码（支持 #RRGGBB 与 #RRGGBBAA）。
    //
    // 必须放在数值分支之前：字符串字面量以 `"` 开头，与 `$r:expr` 不会混淆，
    // 但先匹配字符串可让该分支在歧义情况下优先命中。
    ($hex:literal) => {
        $crate::color::Color::from_hex($hex).expect("invalid hex color literal")
    };

    // RGB 格式 (自动填充 alpha=255)
    ($r:expr, $g:expr, $b:expr) => {
        $crate::color::Color::new($r as u8, $g as u8, $b as u8)
    };

    // RGBA 格式
    ($r:expr, $g:expr, $b:expr, $a:expr) => {
        $crate::color::Color::with_alpha($r as u8, $g as u8, $b as u8, $a as u8)
    };
}

// ———————————————————————————————— 基础颜色 ————————————————————————————————

/// 黑色 `(0, 0, 0)`。
pub const BLACK: Color = color!(0, 0, 0);
/// 白色 `(255, 255, 255)`。
pub const WHITE: Color = color!(255, 255, 255);
/// 红色 `(255, 0, 0)`。
pub const RED: Color = color!(255, 0, 0);
/// 绿色 `(0, 255, 0)`。
pub const GREEN: Color = color!(0, 255, 0);
/// 蓝色 `(0, 0, 255)`。
pub const BLUE: Color = color!(0, 0, 255);

// ———————————————————————————————— 实用混合色 ————————————————————————————————

/// 黄色 `(255, 255, 0)`（红 + 绿）。
pub const YELLOW: Color = color!(255, 255, 0);
/// 青色 `(0, 255, 255)`（绿 + 蓝）。
pub const CYAN: Color = color!(0, 255, 255);
/// 品红色 `(255, 0, 255)`（红 + 蓝）。
pub const MAGENTA: Color = color!(255, 0, 255);

// ———————————————————————————————— 中性色 ————————————————————————————————

/// 灰色 `(128, 128, 128)`。
pub const GRAY: Color = color!(128, 128, 128);

// ———————————————————————————————— 5/6/5 打包格式 ————————————————————————————————

/// 把 8 位通道量化到 `bits` 位，四舍五入（`(v × max + 127) / 255`）。
///
/// `bits ∈ {5, 6, 8}`；`bits == 8` 时结果恒等于输入。取值上界：
/// `(255 × 63 + 127) / 255 = 63`，不会溢出 `u16`。
///
/// 与 [`expand_channel`] 互逆：对任意 `v < 2^bits` 有
/// `quantize_channel(expand_channel(v, bits), bits) == v`。
#[inline(always)]
pub(crate) const fn quantize_channel(value: u8, bits: u8) -> u16 {
    let max = (1u32 << bits) - 1;
    ((value as u32 * max + 127) / 255) as u16
}

/// 把 `bits` 位的字段扩展回 8 位（位复制填充低位）。
///
/// 例：5 位 `v` → `(v << 3) | (v >> 2)`；6 位 `v` → `(v << 2) | (v >> 4)`。
#[inline(always)]
pub(crate) const fn expand_channel(value: u16, bits: u8) -> u8 {
    let shift = 8 - bits;
    ((value << shift) | (value >> (bits - shift))) as u8
}

/// 565 位域中绿色字段的位移：两个变体恒为 `10..5`，不受红蓝互换影响。
pub(crate) const GREEN_SHIFT_565: u32 = 5;

/// 565 格式的通道位域：R 在高位还是低位。
///
/// 两种布局的差异**只在红蓝两段的位置**，绿色恒占 `10..5`：
///
/// * [`RedHigh`](Layout565::RedHigh)（`Rgb565`）：`R = 15..11`、`B = 4..0`
/// * [`BlueHigh`](Layout565::BlueHigh)（`Bgr565`）：`B = 15..11`、`R = 4..0`
///
/// 两者**都是小端序存储**（与 `DRM_FORMAT_RGB565` / `DRM_FORMAT_BGR565` 一致），
/// 故 565 像素的字节 0 恒为位域低字节。位段差异不牵涉存储字节序，
/// [`encode_565_bytes`] 对两个格式一律走小端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout565 {
    /// `Rgb565`：红占高 5 位（`15..11`），蓝占低 5 位（`4..0`）。
    RedHigh,
    /// `Bgr565`：蓝占高 5 位（`15..11`），红占低 5 位（`4..0`）。
    BlueHigh,
}

impl Layout565 {
    /// 该布局下 5 位红通道的位移（`Rgb565` 为 11，`Bgr565` 为 0）。
    #[inline(always)]
    pub(crate) const fn red_shift(self) -> u32 {
        match self {
            Layout565::RedHigh => 11,
            Layout565::BlueHigh => 0,
        }
    }

    /// 该布局下 5 位蓝通道的位移（`Rgb565` 为 0，`Bgr565` 为 11）。
    #[inline(always)]
    pub(crate) const fn blue_shift(self) -> u32 {
        match self {
            Layout565::RedHigh => 0,
            Layout565::BlueHigh => 11,
        }
    }
}

/// [`PixelFormat`] 到 565 位域布局的映射。
///
/// 8 位通道格式与其它 565 变体不存在，故返回 `Option`，让调用方在
/// 「是否 2bpp 565」这一维度上显式表态，而不是拿到一个伪造的默认值。
#[inline(always)]
pub(crate) const fn layout_565(format: PixelFormat) -> Option<Layout565> {
    match format {
        PixelFormat::Rgb565 => Some(Layout565::RedHigh),
        PixelFormat::Bgr565 => Some(Layout565::BlueHigh),
        PixelFormat::Rgb24
        | PixelFormat::Bgr24
        | PixelFormat::Xrgb8888
        | PixelFormat::Argb8888
        | PixelFormat::Rgba8888
        | PixelFormat::Bgra8888 => None,
    }
}

/// 把颜色量化打包成 565 字（位域布局由 `layout` 指定）。
///
/// 两个 565 变体都是**小端存储**，故本函数不产出任何字节序决策——字节序统一由
/// [`encode_565_bytes`] 表达（对 565 恒为小端）。
#[inline(always)]
pub(crate) const fn pack_565(color: &Color, layout: Layout565) -> u16 {
    // 红蓝各占一段 5 位字段，位移取自 `Layout565`——位域字面量只在那一处定义。
    (quantize_channel(color.r, 5) << layout.red_shift())
        | (quantize_channel(color.g, 6) << GREEN_SHIFT_565)
        | quantize_channel(color.b, 5) << layout.blue_shift()
}

/// 把 565 字展开成 8 位通道颜色（alpha 恒为 `255`）。
///
/// 与 [`pack_565`] 互逆：`layout` 必须是同一个值，否则红蓝会互换。
#[inline(always)]
pub(crate) const fn unpack_565(word: u16, layout: Layout565) -> Color {
    // 与 `pack_565` 对称：同一个 `layout` 决定红蓝各自从哪一段取字段。
    Color::with_alpha(
        expand_channel((word >> layout.red_shift()) & 0x1F, 5),
        expand_channel((word >> GREEN_SHIFT_565) & 0x3F, 6),
        expand_channel((word >> layout.blue_shift()) & 0x1F, 5),
        255,
    )
}

/// 565 就地混合：把目标字解码到 8 位，用 [`PixelFormat::blend_channel`] 混合，
/// 再量化回 565。
///
/// 这是 565 唯一的混合语义，与其余格式共用同一套参考公式，误差只来自量化。
/// 两个短路：`color.a == 255` 直接返回 [`pack_565`]（不读目标像素）；
/// `color.a == 0` 原样返回 `word`（与 8 位路径 `blend_channel` 的不动点一致）。
#[inline(always)]
pub(crate) fn blend_565(word: u16, color: &Color, layout: Layout565) -> u16 {
    let alpha = color.a;
    if alpha == 255 {
        return pack_565(color, layout);
    }
    if alpha == 0 {
        return word;
    }

    let dst = unpack_565(word, layout);
    let alpha = alpha as u16;
    let inv_alpha = 255 - alpha;
    // 先算出量化后的 5/6/5 字段，再拼字：与 `pack_565` 的布局保持一致。
    let (r, g, b) = (
        quantize_channel(
            PixelFormat::blend_channel(color.r, dst.r, alpha, inv_alpha),
            5,
        ),
        quantize_channel(
            PixelFormat::blend_channel(color.g, dst.g, alpha, inv_alpha),
            6,
        ),
        quantize_channel(
            PixelFormat::blend_channel(color.b, dst.b, alpha, inv_alpha),
            5,
        ),
    );
    // 拼字布局与 `pack_565` 一致：红蓝的位移同样来自 `layout`。
    (r << layout.red_shift()) | (g << GREEN_SHIFT_565) | (b << layout.blue_shift())
}

/// 按存储字节序把两个字节解码成 565 字。
///
/// # 字节序
///
/// 两个 565 变体（`Rgb565` / `Bgr565`）**都是小端序存储**——它们只差位域里的
/// 红蓝位置，不差字节序（参见 [`Layout565`] 与 `DRM_FORMAT_RGB565` /
/// `DRM_FORMAT_BGR565` 的定义）。故 `little_endian` 对 565 恒为 `true`；
/// 该参数保留是为了让调用点能显式表达存储约定，并在未来引入大端变体时
/// 无需改动签名。
///
/// # 调用方保证
///
/// `src` 长度至少为 2。
#[inline(always)]
pub(crate) fn decode_565_bytes(src: &[u8], little_endian: bool) -> u16 {
    debug_assert!(src.len() >= 2, "565 像素需要 2 字节");
    let pair = [src[0], src[1]];
    if little_endian {
        u16::from_le_bytes(pair)
    } else {
        u16::from_be_bytes(pair)
    }
}

/// 按存储字节序把 565 字编码成两个字节（[`decode_565_bytes`] 的逆操作）。
///
/// `little_endian` 的语义与 [`decode_565_bytes`] 一致；对当前两个 565 变体恒为 `true`。
#[inline(always)]
pub(crate) fn encode_565_bytes(word: u16, little_endian: bool) -> [u8; 2] {
    if little_endian {
        word.to_le_bytes()
    } else {
        word.to_be_bytes()
    }
}

// ———————————————————————————— BMP 编解码用的通道助手 ————————————————————————————

/// 把 `bits` 位（`bits ∈ {5, 6, 8}`）的通道值扩展回 8 位。
///
/// 只是 [`expand_channel`] 的 crate 内别名，供 `image::bmp` 使用。BMP 的
/// `BI_BITFIELDS` 解码按文件头给出的**任意**掩码取位，{5, 6, 5} 只是最常见的一档，
/// 因此不能复用只认 565 两段布局的 [`unpack_565`]；但「位复制扩展」这一步的
/// 语义必须与它完全一致。
#[inline(always)]
pub(crate) const fn expand_channel_to_8(value: u16, bits: u8) -> u8 {
    expand_channel(value, bits)
}

/// 把 8 位通道量化到 `bits` 位（`bits ∈ {5, 6}`）。
///
/// 只是 [`quantize_channel`] 的 crate 内别名，供 BMP 编码器复用「+127 四舍五入」
/// 的公式，避免在 `image::bmp` 里再抄一份量化规则。
#[inline(always)]
pub(crate) const fn quantize_channel_to_bits(value: u8, bits: u8) -> u16 {
    quantize_channel(value, bits)
}

// ———————————————————————————————— SWAR Alpha 混合 ————————————————————————————————

/// 面向 4bpp 像素格式的 SWAR（SIMD Within A Register）Alpha 混合内核。
///
/// 只用 32 位整数运算，不依赖任何架构 intrinsics 或内联汇编，
/// 因此在 `no_std` 与任意目标上都可用。
///
/// 布局约定：一个 `u32` 字按小端序承载 4 个字节 `[c0, c1, c2, c3]`，
/// 即字节 `i` 位于位 `8i`。SWAR 运算时先把待处理的字节「摊」到 16 位槽里，
/// 空出的高 8 位用作乘法与加法的进位空间。
pub(crate) mod swar {
    use super::{Color, PixelFormat};

    /// 「偶数号字节」掩码：保留 `c0`、`c2`，抹掉 `c1`、`c3`。
    ///
    /// 被保留的字节因此各自独占一个 16 位槽（低 8 位是数据，高 8 位空出）。
    const LANE_MASK: u32 = 0x00FF_00FF;

    /// 每个 16 位槽的舍入项，等价于 [`PixelFormat::blend_channel`] 里的 `+ 255`。
    ///
    /// 数值与 [`LANE_MASK`] 相同，但语义不同（把 255 加到每个槽上），
    /// 故单独命名。
    const ROUND_LANES: u32 = 0x00FF_00FF;

    /// 构造与目标字同布局的 SWAR 源字。
    ///
    /// 返回字的 4 个字节依次为 `[c0, c1, c2, opaque_alpha]`：`is_rgb_order`
    /// 为真时 `c0..c2` 是 R/G/B，否则是 B/G/R。第 4 字节在混合后会被强制
    /// 覆写成 `opaque_alpha`，这里的取值只影响一个即将被丢弃的槽。
    #[inline(always)]
    pub(crate) fn source_word(r: u8, g: u8, b: u8, opaque_alpha: u8, is_rgb_order: bool) -> u32 {
        let (c0, c1, c2) = if is_rgb_order { (r, g, b) } else { (b, g, r) };
        (c0 as u32) | ((c1 as u32) << 8) | ((c2 as u32) << 16) | ((opaque_alpha as u32) << 24)
    }

    /// 对两个「偶数号字节」槽并行执行一次 `(d*(255-a) + s*a + 255) >> 8`。
    ///
    /// `d` / `s` 必须已用 [`LANE_MASK`] 把待处理字节放进 16 位槽的低 8 位；
    /// 返回值的槽布局与输入一致，每个槽的低 8 位是混合结果。
    ///
    /// # 取值范围
    ///
    /// 因为 `a ≤ 255`，槽内各项都不超过 65025：
    ///
    /// | 项 | 上界 |
    /// |----|------|
    /// | `d * 255` | 65025 |
    /// | `d * a` | 65025 |
    /// | `s * a` | 65025 |
    /// | `d*255 - d*a + s*a = d*(255-a) + s*a` | 65025 |
    /// | 再加舍入项 `255` | 65280 |
    ///
    /// 中间量 `d*255 - d*a` 与最终结果都落在 `[0, 65025]`，因此槽与槽之间
    /// 既不会进位也不会借位，整个 32 位字也不会回绕（debug 下不会触发溢出检查）。
    #[inline(always)]
    fn blend_lane_pairs(d: u32, s: u32, alpha: u32) -> u32 {
        // `d*255` 展开成 `(d << 8) - d`，省掉一次乘法。`d` 已被 LANE_MASK
        // 约束为每槽只占低 8 位，左移不会丢失任何数据。
        let blended = (d << 8) - d - d * alpha + s * alpha + ROUND_LANES;
        (blended >> 8) & LANE_MASK
    }

    /// 用 SWAR 对 4 个 8 位通道并行做 Alpha 混合。
    ///
    /// `word` 与 `src` 都按小端字节序布局，返回值的字节 `i` 等于
    /// `PixelFormat::blend_channel(src_i, word_i, alpha, 255 - alpha)`，
    /// 与逐通道参考实现逐字节一致。
    ///
    /// # 为什么分两批
    ///
    /// 逐通道参考式 `s*a + d*(255-a)` 的中间值需要 16 位，4 个通道就要 64 位；
    /// 这里把 `c0`/`c2` 留在原位、`c1`/`c3` 右移 8 位，于是一个 32 位字恰好
    /// 同时容纳两个通道的 16 位中间值，两批各处理两个通道。
    ///
    /// 直接展开 `s*a + d*(255-a)` 的中间和可达 `130050 > 2^16`，会跨槽进位；
    /// 改写成 `d*255 - d*a + s*a` 后每一项都不超过 65025，槽内自洽
    /// （推导见 [`blend_lane_pairs`]）。
    #[inline(always)]
    pub(crate) fn blend_word(word: u32, src: u32, alpha: u8) -> u32 {
        let a = alpha as u32;
        let even = blend_lane_pairs(word & LANE_MASK, src & LANE_MASK, a);
        let odd = blend_lane_pairs((word >> 8) & LANE_MASK, (src >> 8) & LANE_MASK, a);
        even | (odd << 8)
    }

    /// 对 4bpp 格式的一行像素做 SWAR 批量 Alpha 混合。
    ///
    /// 每次处理 4 个像素（16 字节 = 4 个 `u32` 字），不足 4 像素的尾部回退到
    /// 逐通道实现。第 4 字节恒被写成 `opaque_alpha` 而不参与插值
    /// （`Xrgb8888` 为 0，其余 4bpp 格式为 255）。
    ///
    /// `color.a` 必须落在 `1..=254`：完全透明由调用方提前跳过，完全不透明走
    /// 不透明填充路径。
    ///
    /// # Safety
    ///
    /// * `row` 必须指向 `pixel_count * 4` 字节的可写区域；
    /// * `row` 必须按 4 字节对齐（内核按 `u32` 读写）。
    #[inline(never)]
    pub(crate) unsafe fn blend_span_alpha_4bpp(
        row: *mut u8,
        pixel_count: usize,
        color: &Color,
        is_rgb_order: bool,
        opaque_alpha: u8,
    ) {
        let alpha = color.a;
        let src_word = source_word(color.r, color.g, color.b, opaque_alpha, is_rgb_order);
        let opaque_word = (opaque_alpha as u32) << 24;

        let quads = pixel_count / 4;
        for quad in 0..quads {
            // SAFETY: 调用方保证 row 指向 pixel_count * 4 字节且 4 字节对齐，
            // 故 quad * 16 + 16 <= quads * 16 <= pixel_count * 4，且每次访问都对齐。
            let dst = unsafe { row.add(quad * 16) as *mut u32 };
            for lane in 0..4 {
                unsafe {
                    let word = *dst.add(lane);
                    let blended = blend_word(word, src_word, alpha);
                    // 第 4 字节必须恒为 opaque_alpha，而不是参与混合的结果——
                    // 与逐像素路径的 `*dst.add(3) = opaque_alpha` 保持一致。
                    *dst.add(lane) = (blended & 0x00FF_FFFF) | opaque_word;
                }
            }
        }

        // 尾部不足 4 像素的部分回退到逐通道路径
        let alpha16 = alpha as u16;
        let inv_alpha = 255 - alpha16;
        let (c0, c1, c2) = if is_rgb_order {
            (color.r, color.g, color.b)
        } else {
            (color.b, color.g, color.r)
        };
        for pixel_idx in (quads * 4)..pixel_count {
            // SAFETY: pixel_idx < pixel_count，故 pixel_idx * 4 + 4 <= pixel_count * 4。
            let dst = unsafe { row.add(pixel_idx * 4) };
            unsafe {
                *dst = PixelFormat::blend_channel(c0, *dst, alpha16, inv_alpha);
                *dst.add(1) = PixelFormat::blend_channel(c1, *dst.add(1), alpha16, inv_alpha);
                *dst.add(2) = PixelFormat::blend_channel(c2, *dst.add(2), alpha16, inv_alpha);
                *dst.add(3) = opaque_alpha;
            }
        }
    }

    /// 对 4bpp 格式的一行像素做掩码（逐像素 alpha）混合。
    ///
    /// 与 [`blend_span_alpha_4bpp`] 的区别是：每个像素的透明度来自掩码而不是
    /// 同一个源色。
    ///
    /// # Safety
    ///
    /// * `row` 必须指向 `pixel_count * 4` 字节的可写区域，且按 4 字节对齐；
    /// * `mask` 必须指向 `pixel_count` 个可读字节。
    #[inline(never)]
    pub(crate) unsafe fn blend_masked_row_4bpp(
        row: *mut u8,
        mask: *const u8,
        pixel_count: usize,
        color: &Color,
        is_rgb_order: bool,
        opaque_alpha: u8,
    ) {
        let src_word = source_word(color.r, color.g, color.b, opaque_alpha, is_rgb_order);
        let opaque_word = (opaque_alpha as u32) << 24;
        // 整组不透明直写用的整像素字：通道顺序与目标字一致
        let (c0, c1, c2) = if is_rgb_order {
            (color.r, color.g, color.b)
        } else {
            (color.b, color.g, color.r)
        };
        let opaque_pixel = (c0 as u32) | ((c1 as u32) << 8) | ((c2 as u32) << 16) | opaque_word;

        // 进行中的混合段起点；usize::MAX 表示无待收尾的段。
        // 迭代量只取决于 pixel_count（帧缓冲派生），符合迭代量上界约束。
        let mut run_start = usize::MAX;
        let mut idx = 0usize;
        while pixel_count - idx >= 4 {
            // SAFETY: idx + 4 <= pixel_count，故 mask 的 4 字节可读。
            // 掩码按小端拼成 u32 后与字节序无关（只比较全 0 / 全 0xFF）。
            let m4 = unsafe { core::ptr::read_unaligned(mask.add(idx) as *const u32) };
            if m4 == 0 {
                // 整组全透明：不读不写；先收尾进行中的混合段
                if run_start != usize::MAX {
                    // SAFETY: run_start..idx ⊆ [0, pixel_count)，前置条件由
                    // 本函数 # Safety 段保证
                    unsafe {
                        blend_masked_segment(row, mask, run_start, idx, src_word, opaque_word)
                    };
                    run_start = usize::MAX;
                }
            } else if m4 == 0xFFFF_FFFF {
                // 整组全不透明：直写 4 个整像素，省掉读-改-写；同样先收尾
                if run_start != usize::MAX {
                    // SAFETY: 同上
                    unsafe {
                        blend_masked_segment(row, mask, run_start, idx, src_word, opaque_word)
                    };
                    run_start = usize::MAX;
                }
                // SAFETY: idx + 4 <= pixel_count 且 row 按 4 字节对齐，
                // 故 4 个 u32 写入都在可写区域内且对齐。
                unsafe {
                    let base = row.add(idx * 4) as *mut u32;
                    for k in 0..4 {
                        *base.add(k) = opaque_pixel;
                    }
                }
            } else {
                // 混合组：只记录段起点，把计算留给可向量化的段内核
                if run_start == usize::MAX {
                    run_start = idx;
                }
            }
            idx += 4;
        }
        if run_start != usize::MAX {
            // SAFETY: run_start..idx ⊆ [0, pixel_count)
            unsafe { blend_masked_segment(row, mask, run_start, idx, src_word, opaque_word) };
            run_start = usize::MAX;
        }
        if idx < pixel_count {
            // 尾部不足 4 像素：逐像素位运算混合（组内 0/255 的值由
            // blend_word 的恒等性质与 keep_high 掩码保证，见函数文档）
            // SAFETY: idx..pixel_count ⊆ [0, pixel_count)
            unsafe { blend_masked_segment(row, mask, idx, pixel_count, src_word, opaque_word) };
        }
        // run_start 在此处必为 usize::MAX（两条收尾路径已覆盖）
        debug_assert_eq!(run_start, usize::MAX);
    }

    /// 对 `[start, end)` 内的像素逐个执行位运算掩码混合（段内核）。
    ///
    /// 段内可能含任意掩码值：`m == 0` 时 `blend_word` 恒等于原字且
    /// `keep_high` 保留原第 4 字节（值不变），`m == 255` 时前三字节恒等于
    /// 源色——因此循环体无需任何数据依赖分支，唯一的边界 `start..end`
    /// 是运行时长度、与段内数据无关，可被自动向量化。
    ///
    /// # Safety
    ///
    /// * `row` 必须指向 `pixel_count * 4` 字节的可写区域，且按 4 字节对齐；
    /// * `mask` 必须指向 `pixel_count` 个可读字节；
    /// * `start..end` 必须落在 `[0, pixel_count]` 内。
    #[inline(always)]
    unsafe fn blend_masked_segment(
        row: *mut u8,
        mask: *const u8,
        start: usize,
        end: usize,
        src_word: u32,
        opaque_word: u32,
    ) {
        for idx in start..end {
            // SAFETY: start..end ⊆ [0, pixel_count)，故 mask[idx] 可读、
            // row.add(idx * 4) 落在可写区域内，且 4 字节对齐保证 u32 访问对齐。
            unsafe {
                let m = *mask.add(idx);
                let dst = row.add(idx * 4);
                let word = *(dst as *const u32);
                let blended = blend_word(word, src_word, m);
                // m == 0 时 blended == word，第 4 字节须保留原值：
                // keep_high = m == 0 ? 0xFF00_0000 : 0（无分支构造）
                let keep_high = 0u32.wrapping_sub((m as u32).wrapping_sub(1) >> 31) << 24;
                let out = (blended & 0x00FF_FFFF) | (word & keep_high) | (opaque_word & !keep_high);
                *(dst as *mut u32) = out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    // SWAR 内核是 `pub(crate)`，测试与它同处 `color` 模块内，故直接引用其私有项。
    use super::swar::{blend_word, source_word};

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

    /// 在 565 上可无损表示的通道值：字段取满值 31/63/31，
    /// 经 `expand_channel` 位复制后恰为 255（故往返精确）。
    const REPRESENTABLE_565: Color = Color::new(255, 255, 255);

    // ————————————————————————————— S02 · Color 与十六进制 —————————————————————————————

    #[test]
    fn color_constructors_and_constants() {
        assert_eq!(Color::new(1, 2, 3), Color::with_alpha(1, 2, 3, 255));
        assert_eq!(Color::new(0, 0, 0).a, 255, "new 必须默认不透明");
        assert_eq!(Color::with_alpha(9, 8, 7, 6).a, 6);

        // 色彩常量逐字校验，避免有人改错通道顺序。
        assert_eq!(BLACK, Color::new(0, 0, 0));
        assert_eq!(WHITE, Color::new(255, 255, 255));
        assert_eq!(RED, Color::new(255, 0, 0));
        assert_eq!(GREEN, Color::new(0, 255, 0));
        assert_eq!(BLUE, Color::new(0, 0, 255));
        assert_eq!(YELLOW, Color::new(255, 255, 0));
        assert_eq!(CYAN, Color::new(0, 255, 255));
        assert_eq!(MAGENTA, Color::new(255, 0, 255));
        assert_eq!(GRAY, Color::new(128, 128, 128));
    }

    #[test]
    fn color_macro_accepts_literals_and_hex() {
        assert_eq!(color!(255, 0, 0), Color::new(255, 0, 0));
        assert_eq!(color!(1, 2, 3, 4), Color::with_alpha(1, 2, 3, 4));
        assert_eq!(color!("#FF0000"), Color::new(255, 0, 0));
        assert_eq!(color!("#FF000080"), Color::with_alpha(255, 0, 0, 128));
        // 无 `#` 前缀的字符串同样可用。
        assert_eq!(color!("00FF00"), Color::new(0, 255, 0));
        // 宏对字面量做 `as u8` 窄化。**超范围的整数字面量会在编译期被拒绝**
        // （`overflowing_literals` 是 deny lint），因此这里只验证范围内的窄化。
        assert_eq!(color!(0u8, 1u8, 2u8), Color::new(0, 1, 2));
        // 非字面量表达式走运行时 `as` 语义（截断到低 8 位）。
        let wide = 0x1FFu32; // = 0b1_1111_1111
        assert_eq!(color!(wide, 0, 0), Color::new(0xFF, 0, 0));
    }

    #[test]
    fn from_hex_parses_both_lengths_and_optional_hash() {
        assert_eq!(Color::from_hex("#FF0000"), Ok(Color::new(255, 0, 0)));
        // 无 `#` 前缀。
        assert_eq!(Color::from_hex("FF0000"), Ok(Color::new(255, 0, 0)));
        // 8 位保留 alpha。
        assert_eq!(
            Color::from_hex("#00FF0080"),
            Ok(Color::with_alpha(0, 255, 0, 128))
        );
        // 6 位补不透明 alpha。
        assert_eq!(Color::from_hex("#123456").unwrap().a, 255);
        // 大小写均可。
        assert_eq!(Color::from_hex("#abcdef"), Ok(Color::new(0xab, 0xcd, 0xef)));
        assert_eq!(Color::from_hex("#ABCDEF"), Ok(Color::new(0xab, 0xcd, 0xef)));
        assert_eq!(
            Color::from_hex("#AABBCCDD"),
            Ok(Color::with_alpha(0xaa, 0xbb, 0xcc, 0xdd))
        );
        // 混合大小写 + 8 位。
        assert_eq!(
            Color::from_hex("#aAbBcCdD"),
            Ok(Color::with_alpha(0xaa, 0xbb, 0xcc, 0xdd))
        );
        // 边界值 0 与 255。
        assert_eq!(
            Color::from_hex("#00000000"),
            Ok(Color::with_alpha(0, 0, 0, 0))
        );
        assert_eq!(
            Color::from_hex("#FFFFFFFF"),
            Ok(Color::with_alpha(255, 255, 255, 255))
        );
    }

    #[test]
    fn from_hex_rejects_bad_length_as_invalid_length() {
        for input in ["", "#", "#FFF", "#FFFF", "#FFFFF", "#FFFFFFF", "#FFFFFFFFF"] {
            assert_eq!(
                Color::from_hex(input),
                Err(ColorParseError::InvalidLength),
                "长度非 6/8 的输入应报 InvalidLength: {input:?}"
            );
        }
    }

    #[test]
    fn from_hex_rejects_bad_digits_as_invalid_digit() {
        // 长度合法（去 `#` 后 6 或 8 字符）但含非十六进制字符。
        for input in ["#GG0000", "#00GG00", "#0000GG", "-10000"] {
            assert_eq!(
                Color::from_hex(input),
                Err(ColorParseError::InvalidDigit),
                "含非法字符应报 InvalidDigit: {input:?}"
            );
        }
        // 8 位形式中的 alpha 段非法也要报 InvalidDigit。
        assert_eq!(
            Color::from_hex("#000000ZZ"),
            Err(ColorParseError::InvalidDigit)
        );
        // 注意：`u8::from_str_radix` 接受可选的 `+` 前缀，故 `"+10000"` 是**合法**输入。
        // 这是标准库行为，此处固化为已知契约，避免被误当成缺陷。
        assert_eq!(Color::from_hex("+10000"), Ok(Color::new(1, 0, 0)));
    }

    #[test]
    fn rgba_u32_round_trips_all_channels() {
        for color in [
            Color::new(0, 0, 0),
            Color::new(255, 255, 255),
            Color::with_alpha(1, 2, 3, 4),
            Color::with_alpha(255, 0, 128, 255),
            // alpha = 0 必须往返保留，不能被当成「缺失」。
            Color::with_alpha(10, 20, 30, 0),
        ] {
            assert_eq!(Color::from_rgba_u32(color.to_rgba_u32()), color);
        }
        // 编码布局：R 在最高字节，G 次之，B 再次之，A 在最低字节（`R24G16B8A0`）。
        assert_eq!(Color::new(0x12, 0x34, 0x56).to_rgba_u32(), 0x1234_56FF);
        assert_eq!(Color::new(1, 2, 3).to_rgba_u32(), 0x0102_03FF);
        assert_eq!(Color::with_alpha(1, 2, 3, 4).to_rgba_u32(), 0x0102_0304);
    }

    #[test]
    fn mix_alpha_replaces_only_alpha() {
        let base = Color::new(10, 20, 30);
        assert_eq!(base.mix_alpha(0), Color::with_alpha(10, 20, 30, 0));
        assert_eq!(base.mix_alpha(128), Color::with_alpha(10, 20, 30, 128));
        assert_eq!(base.mix_alpha(255), Color::with_alpha(10, 20, 30, 255));
        // 原颜色不受影响（`mix_alpha` 按值接收 self 但返回新值）。
        assert_eq!(base.a, 255);
    }

    #[test]
    fn blend_endpoints_match_dst_and_src() {
        let src = Color::new(255, 0, 0);
        let dst = Color::new(0, 0, 255);
        // alpha = 0 → 完全保留目标。
        assert_eq!(src.blend(&dst, 0), Color::new(0, 0, 255));
        // alpha = 255 → 完全替换为源色。
        assert_eq!(src.blend(&dst, 255), Color::new(255, 0, 0));
    }

    #[test]
    fn blend_interpolates_and_always_returns_opaque() {
        let src = Color::new(255, 0, 0);
        let dst = Color::new(0, 0, 255);
        // 文档中的示例值。
        assert_eq!(src.blend(&dst, 128), Color::new(128, 0, 127));
        // 中间值与逐通道 blend_channel 公式一致。
        let src_channels = [src.r, src.g, src.b];
        let dst_channels = [dst.r, dst.g, dst.b];
        for alpha in 0..=255u8 {
            let got = src.blend(&dst, alpha);
            let got_channels = [got.r, got.g, got.b];
            let inv = 255 - alpha as u16;
            for channel in 0..3 {
                assert_eq!(
                    got_channels[channel],
                    PixelFormat::blend_channel(
                        src_channels[channel],
                        dst_channels[channel],
                        alpha as u16,
                        inv
                    ),
                    "alpha={alpha} channel={channel}"
                );
            }
            // 混合结果的 alpha 恒为 255（从不落盘源 alpha）。
            assert_eq!(got.a, 255, "blend 必须返回不透明颜色");
        }
    }

    #[test]
    fn invert_flips_rgb_and_keeps_alpha() {
        let color = Color::with_alpha(0x12, 0x34, 0x56, 0x78);
        let inverted = color.invert();
        assert_eq!(inverted, Color::with_alpha(0xED, 0xCB, 0xA9, 255));
        // alpha 被重置为不透明（`invert` 走 `Color::new`）。
        assert_eq!(inverted.a, 255);
        // 双重取反还原 RGB。
        assert_eq!(Color::new(0x12, 0x34, 0x56), inverted.invert());
    }

    #[cfg(feature = "programmable")]
    #[test]
    fn programmable_blend_modes_match_their_formulas() {
        let src = Color::with_alpha(200, 100, 50, 128);
        let dst = Color::new(10, 20, 30);

        // Add：各通道饱和相加。
        assert_eq!(src.blend_add(&dst), Color::new(210, 120, 80));
        // 饱和：200 + 200 > 255 被钳到 255。
        let hot = Color::new(200, 200, 200);
        assert_eq!(
            hot.blend_add(&Color::new(200, 10, 10)),
            Color::new(255, 210, 210)
        );

        // Multiply：src × dst / 255（+127 四舍五入）。
        let mul = |s: u8, d: u8| ((s as u16 * d as u16 + 127) / 255) as u8;
        assert_eq!(
            src.blend_multiply(&dst),
            Color::new(mul(200, 10), mul(100, 20), mul(50, 30))
        );
        // 与 0 相乘得 0，与 255 相乘得原值。
        assert_eq!(
            Color::new(7, 8, 9).blend_multiply(&Color::new(0, 0, 0)),
            Color::new(0, 0, 0)
        );
        assert_eq!(
            Color::new(7, 8, 9).blend_multiply(&Color::new(255, 255, 255)),
            Color::new(7, 8, 9)
        );

        // PremultipliedAlpha：src + dst × (1 - a)，**不再乘一次 src.a**。
        // 这是它与 `Color::blend` 的关键区别。
        let inv_alpha = 255 - src.a as u16;
        let pre = |s: u8, d: u8| s.saturating_add(((d as u16 * inv_alpha + 127) / 255) as u8);
        assert_eq!(
            src.blend_premultiplied(&dst),
            Color::new(pre(200, 10), pre(100, 20), pre(50, 30))
        );
        // 语义对照：同样的 src/dst，`blend` 会再乘一次 a，结果必然不同。
        assert_ne!(
            src.blend_premultiplied(&dst),
            src.blend(&dst, src.a),
            "预乘混合不应等同于普通 alpha 混合"
        );
        // a = 255 时预乘混合退化为纯覆盖 src。
        let opaque = Color::with_alpha(11, 22, 33, 255);
        assert_eq!(opaque.blend_premultiplied(&dst), Color::new(11, 22, 33));
    }

    // ————————————————————————————— S03 · PixelFormat 元数据 —————————————————————————————

    #[test]
    fn bytes_and_bits_per_pixel_are_table_driven() {
        let expected = [
            (PixelFormat::Rgb24, 3usize, 24usize),
            (PixelFormat::Bgr24, 3, 24),
            (PixelFormat::Xrgb8888, 4, 32),
            (PixelFormat::Argb8888, 4, 32),
            (PixelFormat::Rgba8888, 4, 32),
            (PixelFormat::Bgra8888, 4, 32),
            (PixelFormat::Rgb565, 2, 16),
            (PixelFormat::Bgr565, 2, 16),
        ];
        for (format, bpp, bits) in expected {
            assert_eq!(
                format.bytes_per_pixel(),
                bpp,
                "{format:?} 的 bytes_per_pixel"
            );
            assert_eq!(
                format.bits_per_pixel(),
                bits,
                "{format:?} 的 bits_per_pixel"
            );
            // bits 恒等于 bytes × 8。
            assert_eq!(format.bits_per_pixel(), format.bytes_per_pixel() * 8);
        }
    }

    #[test]
    fn channel_shifts_are_table_driven() {
        let expected = [
            // (format, r, g, b, alpha)
            (PixelFormat::Rgb24, 0u8, 8u8, 16u8, None),
            (PixelFormat::Bgr24, 16, 8, 0, None),
            (PixelFormat::Xrgb8888, 16, 8, 0, None),
            (PixelFormat::Argb8888, 16, 8, 0, Some(24u8)),
            (PixelFormat::Rgba8888, 0, 8, 16, Some(24)),
            (PixelFormat::Bgra8888, 16, 8, 0, Some(24)),
            (PixelFormat::Rgb565, 11, 5, 0, None),
            (PixelFormat::Bgr565, 0, 5, 11, None),
        ];
        for (format, r, g, b, alpha) in expected {
            assert_eq!(format.red_shift(), r, "{format:?} 的 red_shift");
            assert_eq!(format.green_shift(), g, "{format:?} 的 green_shift");
            assert_eq!(format.blue_shift(), b, "{format:?} 的 blue_shift");
            assert_eq!(format.alpha_shift(), alpha, "{format:?} 的 alpha_shift");
        }
    }

    #[test]
    fn xrgb8888_shifts_are_exactly_bgr_order() {
        // Xrgb8888 的保留字节占最高 8 位，故 RGB 三通道与 Bgr24 完全同位域。
        assert_eq!(
            PixelFormat::Xrgb8888.red_shift(),
            PixelFormat::Bgr24.red_shift()
        );
        assert_eq!(
            PixelFormat::Xrgb8888.green_shift(),
            PixelFormat::Bgr24.green_shift()
        );
        assert_eq!(
            PixelFormat::Xrgb8888.blue_shift(),
            PixelFormat::Bgr24.blue_shift()
        );
        // 但它没有 alpha 通道。
        assert_eq!(PixelFormat::Xrgb8888.alpha_shift(), None);
    }

    #[test]
    fn write_then_read_round_trips_rgb_for_all_formats() {
        // 预置非零背景，使「写入未生效」与「解码走错通道」都能被察觉。
        let original = Color::new(0x9C, 0x3E, 0x71);
        for format in ALL_FORMATS {
            // 565 会量化：只有可表示值才能精确往返，故用满字段值 255。
            let probe = if format.bytes_per_pixel() == 2 {
                REPRESENTABLE_565
            } else {
                original
            };

            let mut buffer = [0u8; 4];
            format.write_color(&mut buffer, &probe);
            assert_eq!(
                format.read_color(&buffer),
                probe,
                "{format:?} 的往返失败（写入 {probe:?}，解出 {:?}）",
                format.read_color(&buffer)
            );
            // 无 alpha 的格式解出 alpha 恒为 255。
            if format.alpha_shift().is_none() {
                assert_eq!(
                    format.read_color(&buffer).a,
                    255,
                    "{format:?} 应解出不透明 alpha"
                );
            }
        }
    }

    #[test]
    fn alpha_preserving_formats_round_trip_alpha() {
        // 三种带 alpha 的 4bpp 格式的字节序（BGR / RGB / BGR）。
        let cases = [
            (PixelFormat::Argb8888, [3u8, 2, 1]),
            (PixelFormat::Rgba8888, [1, 2, 3]),
            (PixelFormat::Bgra8888, [3, 2, 1]),
        ];
        for (format, layout) in cases {
            // 不透明写入（a == 255）走直写分支，第 4 字节规范为 255，
            // 颜色逐字节等于该格式的通道序。
            let mut buffer = [0u8; 4];
            format.write_color(&mut buffer, &Color::new(1, 2, 3));
            assert_eq!(
                buffer,
                [layout[0], layout[1], layout[2], 255],
                "{format:?} 写入布局"
            );

            // 契约：混合结果的 alpha **从不落盘**，`read_color` 对这三种格式
            // 恒解出 255。故 alpha 通道的值域由解码侧归一化，而非往返保留。
            for alpha in [0u8, 1, 128, 254] {
                let mut blended = [10u8, 20, 30, 99];
                format.write_color(&mut blended, &Color::with_alpha(1, 2, 3, alpha));
                assert_eq!(
                    blended[3], 255,
                    "{format:?} 半透明写入应把第 4 字节规范为 255"
                );
                let decoded = format.read_color(&blended);
                assert_eq!(decoded.a, 255, "{format:?} 解出的 alpha 应恒为 255");
            }
        }
    }

    #[test]
    fn color_to_bytes_matches_write_color() {
        // 两条编码路径（Color 的字节接口与 PixelFormat 的缓冲接口）不得分叉。
        let color = Color::new(0x12, 0x34, 0x56);
        for format in ALL_FORMATS {
            let mut via_format = [0u8; 4];
            format.write_color(&mut via_format, &color);
            let via_color = color.to_bytes(format);
            assert_eq!(
                &via_format[..format.bytes_per_pixel()],
                &via_color[..format.bytes_per_pixel()],
                "{format:?} 的两条编码路径分叉"
            );
            // 解码侧同样一致。
            let decoded_via_bytes = Color::from_bytes(format, &via_color);
            let decoded_via_read = format.read_color(&via_format);
            assert_eq!(
                decoded_via_bytes, decoded_via_read,
                "{format:?} 的两条解码路径分叉"
            );
        }
    }

    #[test]
    fn xrgb8888_discards_color_alpha_and_writes_zero_pad() {
        // 不透明写入（a == 255）走直写分支，忽略 Color.a，第 4 字节写保留位 0。
        let opaque = Color::new(0x12, 0x34, 0x56);
        let mut buffer = [0xFFu8; 4];
        PixelFormat::Xrgb8888.write_color(&mut buffer, &opaque);
        // Xrgb8888 为 BGR 序：字节 0=B、1=G、2=R，第 4 字节是保留位恒为 0。
        assert_eq!(buffer, [0x56, 0x34, 0x12, 0x00]);
        // 解码按 BGR 逆映射回 (R, G, B)，保留字节被忽略、alpha 恒为 255。
        assert_eq!(
            PixelFormat::Xrgb8888.read_color(&buffer),
            Color::new(0x12, 0x34, 0x56)
        );

        // 半透明写入走混合分支，但第 4 字节同样恒为保留位 0。
        let semi = Color::with_alpha(0x12, 0x34, 0x56, 0xAB);
        let mut blended = [0u8; 4];
        PixelFormat::Xrgb8888.write_color(&mut blended, &semi);
        assert_eq!(blended[3], 0, "半透明写入下保留字节仍恒为 0");
        // 混合结果与逐通道参考一致（BGR 序：字节 0 对应 B 通道）。
        let inv = 255 - semi.a as u16;
        assert_eq!(
            blended,
            [
                PixelFormat::blend_channel(semi.b, 0, semi.a as u16, inv),
                PixelFormat::blend_channel(semi.g, 0, semi.a as u16, inv),
                PixelFormat::blend_channel(semi.r, 0, semi.a as u16, inv),
                0,
            ],
            "Xrgb8888 的混合结果与逐通道参考不符"
        );
    }

    #[test]
    fn opaque_alpha_byte_matches_each_format_contract() {
        // 不透明写入时，各格式的完整字节布局与第 4 字节的可观测结果。
        // 用 a == 255 的颜色，否则 `write_color` 会走混合分支而不做直写。
        let opaque = Color::new(1, 2, 3);
        let cases = [
            (PixelFormat::Xrgb8888, [3u8, 2, 1, 0]), // BGR 序 + 保留字节恒 0
            (PixelFormat::Argb8888, [3, 2, 1, 255]),
            (PixelFormat::Rgba8888, [1, 2, 3, 255]),
            (PixelFormat::Bgra8888, [3, 2, 1, 255]),
        ];
        for (format, expected) in cases {
            let mut buffer = [0u8; 4];
            format.write_color(&mut buffer, &opaque);
            assert_eq!(buffer, expected, "{format:?} 的不透明写入布局不符");
        }

        // 半透明写入：RGB 三通道与目标混合，但第 4 字节仍被规范为格式的 opaque 值。
        // 目标预置为已知背景，才能断言具体的混合结果。
        for (format, expected) in cases {
            let mut buffer = [10u8, 20, 30, 99];
            let semi = Color::with_alpha(100, 200, 50, 128);
            format.write_color(&mut buffer, &semi);
            let inv = 255 - 128u16;
            let blended = PixelFormat::blend_channel(100, 10, 128, inv);
            let (c0_src, c1_src, c2_src) = match format {
                // R,G,B 序：背景字节 0 对应 R
                PixelFormat::Rgba8888 => (100u8, 200, 50),
                // B,G,R 序：背景字节 0 对应 B
                _ => (50u8, 200, 100),
            };
            assert_eq!(
                buffer,
                [
                    PixelFormat::blend_channel(c0_src, 10, 128, inv),
                    PixelFormat::blend_channel(c1_src, 20, 128, inv),
                    PixelFormat::blend_channel(c2_src, 30, 128, inv),
                    expected[3],
                ],
                "{format:?} 的半透明写入未规范第 4 字节或混合结果不符"
            );
            let _ = blended;
        }
    }

    #[test]
    fn blend_channel_matches_reference_formula() {
        // 契约公式：(src * a + dst * (255 - a) + 255) >> 8。
        for src in [0u8, 1, 64, 127, 128, 200, 254, 255] {
            for dst in [0u8, 1, 64, 127, 128, 200, 254, 255] {
                for alpha in [0u16, 1, 128, 254, 255] {
                    let inv = 255 - alpha;
                    let expected = ((src as u16 * alpha + dst as u16 * inv + 255) >> 8) as u8;
                    assert_eq!(
                        PixelFormat::blend_channel(src, dst, alpha, inv),
                        expected,
                        "blend_channel({src}, {dst}, {alpha})"
                    );
                    // 端点精确：a=0 得 dst，a=255 得 src。
                    if alpha == 0 {
                        assert_eq!(PixelFormat::blend_channel(src, dst, alpha, inv), dst);
                    }
                    if alpha == 255 {
                        assert_eq!(PixelFormat::blend_channel(src, dst, alpha, inv), src);
                    }
                }
            }
        }
    }

    // ————————————————————————————— S04 · 565 打包与量化 —————————————————————————————

    #[test]
    fn pack_565_places_fields_per_layout() {
        // Rgb565 位域：R = 15..11、G = 10..5、B = 4..0。
        let white = pack_565(&Color::new(255, 255, 255), Layout565::RedHigh);
        assert_eq!(white, 0xFFFF);
        assert_eq!(
            pack_565(&Color::new(255, 0, 0), Layout565::RedHigh),
            0xF800,
            "纯红只点亮 R 字段"
        );
        assert_eq!(
            pack_565(&Color::new(0, 255, 0), Layout565::RedHigh),
            0x07E0,
            "纯绿只点亮 G 字段"
        );
        assert_eq!(
            pack_565(&Color::new(0, 0, 255), Layout565::RedHigh),
            0x001F,
            "纯蓝只点亮 B 字段"
        );

        // 字段互不重叠：只改一个通道不影响其余字段。
        let mixed = pack_565(&Color::new(255, 0, 255), Layout565::RedHigh);
        assert_eq!(mixed & 0x8000, 0x8000);
        assert_eq!(mixed & 0x07E0, 0x0000);
        assert_eq!(mixed & 0x001F, 0x001F);

        // Bgr565 位域：蓝在高、红在低——与 Rgb565 恰好红蓝互换。
        // 绿色位域恒定，故两布局的同一颜色只差红蓝两段。
        assert_eq!(
            pack_565(&Color::new(255, 0, 255), Layout565::BlueHigh),
            0xF81F,
            "Bgr565 下纯蓝点亮高 5 位"
        );
        assert_eq!(
            pack_565(&Color::new(255, 0, 0), Layout565::BlueHigh),
            0x001F,
            "Bgr565 下纯红点亮低 5 位"
        );
        assert_eq!(
            pack_565(&Color::new(0, 0, 255), Layout565::BlueHigh),
            0xF800,
            "Bgr565 下纯蓝点亮高 5 位"
        );
        assert_eq!(
            pack_565(&Color::new(0, 255, 0), Layout565::BlueHigh),
            0x07E0,
            "绿色位域不受红蓝位置影响"
        );
    }

    /// 回归护栏：`Bgr565` 必须是**位域红蓝互换**，而**不是** `Rgb565` 的字节交换。
    ///
    /// 曾经的实现把 `Bgr565` 实现成 `RGB565_BE`（仅交换两个存储字节），与
    /// Linux DRM 的 `DRM_FORMAT_BGR565`（`[15:0] B:G:R 5:6:5 little endian`）
    /// 不符——两者都是**小端**，差异在位域。下面的断言按 DRM 定义逐条固定：
    ///
    /// * 两个变体都以字节 0 为**位域低字节**（小端）；
    /// * 纯红在 `Rgb565` 落高 5 位、在 `Bgr565` 落低 5 位；
    /// * 纯蓝相反；
    /// * 二者的 shift 元数据**互补**而非相同。
    #[test]
    fn bgr565_swaps_channel_fields_but_keeps_little_endian_byte_order() {
        let color = Color::new(0x12, 0x34, 0x56);

        // 两个变体都是小端：字节 0 恒为位域低字节（这正是原缺陷的否定）。
        let mut rgb_buf = [0u8; 2];
        let mut bgr_buf = [0u8; 2];
        PixelFormat::Rgb565.write_color(&mut rgb_buf, &color);
        PixelFormat::Bgr565.write_color(&mut bgr_buf, &color);

        let rgb_word = u16::from_le_bytes(rgb_buf);
        let bgr_word = u16::from_le_bytes(bgr_buf);
        assert_eq!(
            bgr_word,
            pack_565(&color, Layout565::BlueHigh),
            "Bgr565 必须按 BlueHigh 位域打包"
        );
        assert_eq!(
            rgb_word,
            pack_565(&color, Layout565::RedHigh),
            "Rgb565 必须按 RedHigh 位域打包"
        );
        // 位域不同 ⇒ 字节内容不同（这正是「不只是交换字节序」的可观测后果）。
        assert_ne!(rgb_buf, bgr_buf, "两个变体位域不同，落盘字节不应相同");

        // 关键否定断言：Bgr565 **不是** Rgb565 的字节交换。
        assert_ne!(
            bgr_buf,
            [rgb_buf[1], rgb_buf[0]],
            "Bgr565 若等于 Rgb565 的字节交换，说明位域仍是 RGB——这正是被修掉的缺陷"
        );
        // 红蓝互换对应位段互换，而非字节互换。
        assert_eq!(
            (bgr_word >> 11) as u8,
            (rgb_word & 0x1F) as u8,
            "Bgr565 的高 5 位应是 Rgb565 的低 5 位（蓝）"
        );
        assert_eq!(
            bgr_word & 0x1F,
            rgb_word >> 11,
            "Bgr565 的低 5 位应是 Rgb565 的高 5 位（红）"
        );
        // 绿色位域恒定。
        assert_eq!(
            (rgb_word >> 5) & 0x3F,
            (bgr_word >> 5) & 0x3F,
            "绿色位域必须一致"
        );

        // shift 元数据：绿相同，红蓝互补。
        assert_eq!(
            PixelFormat::Rgb565.green_shift(),
            PixelFormat::Bgr565.green_shift()
        );
        assert_ne!(
            PixelFormat::Rgb565.red_shift(),
            PixelFormat::Bgr565.red_shift(),
            "红位移必须不同（Rgb565=11, Bgr565=0）"
        );
        assert_ne!(
            PixelFormat::Rgb565.blue_shift(),
            PixelFormat::Bgr565.blue_shift(),
            "蓝位移必须不同（Rgb565=0, Bgr565=11）"
        );
        assert_eq!(
            PixelFormat::Rgb565.red_shift(),
            PixelFormat::Bgr565.blue_shift(),
            "Rgb565 的红位移应等于 Bgr565 的蓝位移"
        );

        // 两者各自解码回原色（量化到可表示值后比对）。
        // Rgb565：红在高 5 位（15..11）、蓝在低 5 位（4..0）。
        let expected = Color::with_alpha(
            expand_channel(((rgb_word >> 11) & 0x1F) as u16, 5),
            expand_channel(((rgb_word >> 5) & 0x3F) as u16, 6),
            expand_channel((rgb_word & 0x1F) as u16, 5),
            255,
        );
        assert_eq!(PixelFormat::Rgb565.read_color(&rgb_buf), expected);
        assert_eq!(PixelFormat::Bgr565.read_color(&bgr_buf), expected);
    }

    /// `encode_565_bytes` / `decode_565_bytes` 的字节序契约。
    ///
    /// 两个 565 变体都传 `true`（小端）；参数保留是为将来可能出现的大端变体。
    #[test]
    fn decode_encode_565_bytes_respects_byte_order_flag() {
        let word = 0xABCDu16;
        assert_eq!(
            encode_565_bytes(word, true),
            [0xCD, 0xAB],
            "小端：字节 0 是低字节"
        );
        assert_eq!(
            encode_565_bytes(word, false),
            [0xAB, 0xCD],
            "大端：字节 0 是高字节"
        );
        // decode 是 encode 的逆操作（两个方向都验证）。
        for little_endian in [false, true] {
            let bytes = encode_565_bytes(word, little_endian);
            assert_eq!(decode_565_bytes(&bytes, little_endian), word);
            // 字节序标志取反会解出交换后的字。
            assert_eq!(decode_565_bytes(&bytes, !little_endian), word.swap_bytes());
        }
    }

    #[test]
    fn quantize_and_expand_are_mutually_consistent() {
        // 对任意 v < 2^bits，quantize(expand(v)) == v（可表示值精确往返）。
        for bits in [5u8, 6, 8] {
            let max = (1u16 << bits) - 1;
            for v in 0..=max {
                let expanded = expand_channel(v, bits);
                assert_eq!(
                    quantize_channel(expanded, bits),
                    v,
                    "{bits} 位通道 {v} 往返失败（expanded={expanded}）"
                );
            }
        }
        // 8 位恒等：quantize_channel(v, 8) == v。
        for v in 0..=255u8 {
            assert_eq!(quantize_channel(v, 8), v as u16);
        }
    }

    #[test]
    fn expand_channel_uses_bit_replication() {
        // 5 位：(v << 3) | (v >> 2)；6 位：(v << 2) | (v >> 4)。
        assert_eq!(expand_channel(0, 5), 0);
        assert_eq!(expand_channel(31, 5), 255);
        assert_eq!(expand_channel(0, 6), 0);
        assert_eq!(expand_channel(63, 6), 255);
        for v in 0..32u16 {
            assert_eq!(expand_channel(v, 5), ((v << 3) | (v >> 2)) as u8);
        }
        for v in 0..64u16 {
            assert_eq!(expand_channel(v, 6), ((v << 2) | (v >> 4)) as u8);
        }
    }

    #[test]
    fn quantize_channel_rounds_to_nearest_and_never_overflows() {
        // 四舍五入：(v * max + 127) / 255。
        assert_eq!(quantize_channel(0, 5), 0);
        assert_eq!(quantize_channel(255, 5), 31);
        assert_eq!(quantize_channel(255, 6), 63);
        // 上界不溢出：255 量化后恰为该位宽的最大值。
        for bits in [5u8, 6, 8] {
            assert_eq!(quantize_channel(255, bits), (1u16 << bits) - 1);
        }
        // 中点以上进位、以下舍去。
        assert_eq!(quantize_channel(127, 5), 15);
        assert_eq!(quantize_channel(128, 5), 16);
    }

    /// `pack_565` / `unpack_565` 在**两个位域布局**上都必须精确往返。
    ///
    /// 穷举全部 `32 × 64 × 32` 个字段组合；`word` 恒按 `RedHigh` 的字面量构造，
    /// `BlueHigh` 分支则先用 `unpack` 再 `pack` 验证同一组字段能自洽往返。
    #[test]
    fn pack_unpack_565_round_trips_exactly_on_representable_values() {
        for layout in [Layout565::RedHigh, Layout565::BlueHigh] {
            for r in 0..32u16 {
                for g in 0..64u16 {
                    for b in 0..32u16 {
                        // 按 layout 拼出 word：`r`/`b` 各自落在 layout 指定的段。
                        let (r_shift, b_shift) = (layout.red_shift(), layout.blue_shift());
                        let word = (r << r_shift) | (g << GREEN_SHIFT_565) | (b << b_shift);
                        let unpacked = unpack_565(word, layout);
                        // 展开后必然是规范值；再量化必须精确还原原始字段。
                        let repacked = pack_565(&unpacked, layout);
                        assert_eq!(
                            repacked, word,
                            "{layout:?} 的 565 字段 {r}/{g}/{b} 往返失败，repacked={repacked:#06x}"
                        );
                    }
                }
            }
        }
    }

    /// 565 混合在两个位域布局上都必须与逐通道参考逐位一致。
    #[test]
    fn blend_565_matches_per_channel_reference() {
        for layout in [Layout565::RedHigh, Layout565::BlueHigh] {
            for alpha in 1..=254u8 {
                for src in [0u8, 1, 64, 128, 200, 255] {
                    for dst_word in [0x0000u16, 0xFFFF, 0xF800, 0x07E0, 0x001F, 0x1234] {
                        let dst = unpack_565(dst_word, layout);
                        let color = Color::with_alpha(src, src, src, alpha);
                        let got = blend_565(dst_word, &color, layout);
                        let expected = {
                            let inv = 255 - alpha as u16;
                            let r = quantize_channel(
                                PixelFormat::blend_channel(src, dst.r, alpha as u16, inv),
                                5,
                            );
                            let g = quantize_channel(
                                PixelFormat::blend_channel(src, dst.g, alpha as u16, inv),
                                6,
                            );
                            let b = quantize_channel(
                                PixelFormat::blend_channel(src, dst.b, alpha as u16, inv),
                                5,
                            );
                            (r << layout.red_shift())
                                | (g << GREEN_SHIFT_565)
                                | (b << layout.blue_shift())
                        };
                        assert_eq!(
                            got, expected,
                            "blend_565({layout:?}, word={dst_word:#06x}, alpha={alpha}, src={src})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn blend_565_short_circuits_at_alpha_extremes() {
        let dst_word = 0x1234u16;
        for layout in [Layout565::RedHigh, Layout565::BlueHigh] {
            // alpha = 0：原样返回目标字（不动点）。
            for src in [0u8, 77, 255] {
                assert_eq!(
                    blend_565(dst_word, &Color::with_alpha(src, src, src, 0), layout),
                    dst_word,
                    "{layout:?}：alpha=0 必须原样保留目标字"
                );
            }
            // alpha = 255：直接返回量化后的源色，不读目标。
            for src in [0u8, 77, 255] {
                let color = Color::new(src, src, src);
                assert_eq!(
                    blend_565(dst_word, &color, layout),
                    pack_565(&color, layout),
                    "{layout:?}：alpha=255 必须等于量化后的源色"
                );
            }
        }
    }

    #[test]
    fn format_565_metadata_has_no_alpha_channel() {
        for format in [PixelFormat::Rgb565, PixelFormat::Bgr565] {
            assert_eq!(format.bytes_per_pixel(), 2);
            assert_eq!(format.bits_per_pixel(), 16);
            assert_eq!(format.alpha_shift(), None, "{format:?} 不应有 alpha 通道");
            // 解码出的颜色恒不透明。
            let mut buffer = [0u8; 2];
            format.write_color(&mut buffer, &Color::with_alpha(1, 2, 3, 44));
            assert_eq!(format.read_color(&buffer).a, 255);
        }
    }

    // ————————————————————————————— S05 · swar 内核 —————————————————————————————

    #[test]
    fn source_word_lays_out_both_orders_with_opaque_alpha_last() {
        // RGB 序：字节 0..2 = R,G,B。
        let rgb = source_word(0x11, 0x22, 0x33, 0xFF, true);
        assert_eq!(rgb.to_le_bytes(), [0x11, 0x22, 0x33, 0xFF]);
        // BGR 序：字节 0..2 = B,G,R。
        let bgr = source_word(0x11, 0x22, 0x33, 0xFF, false);
        assert_eq!(bgr.to_le_bytes(), [0x33, 0x22, 0x11, 0xFF]);
        // 第 4 字节恒为 opaque_alpha（Xrgb8888 传 0，其余传 255）。
        assert_eq!(source_word(1, 2, 3, 0, true).to_le_bytes()[3], 0);
        assert_eq!(source_word(1, 2, 3, 255, true).to_le_bytes()[3], 255);
        // 通道顺序标志只影响前三个字节，不影响第 4 字节。
        assert_eq!(
            source_word(1, 2, 3, 0, true).to_le_bytes()[3],
            source_word(1, 2, 3, 0, false).to_le_bytes()[3]
        );
    }

    #[test]
    fn blend_word_matches_per_channel_reference_for_every_alpha() {
        // 全部 256 个 alpha 值下逐字节一致。
        for alpha in 0..=255u8 {
            for (word, src) in representative_words() {
                let blended = blend_word(word, src, alpha).to_le_bytes();
                let word_bytes = word.to_le_bytes();
                let src_bytes = src.to_le_bytes();
                let inv = 255 - alpha as u16;
                for lane in 0..4 {
                    let expected = PixelFormat::blend_channel(
                        src_bytes[lane],
                        word_bytes[lane],
                        alpha as u16,
                        inv,
                    );
                    assert_eq!(
                        blended[lane], expected,
                        "alpha={alpha} lane={lane}: swar={} 参考={expected}",
                        blended[lane]
                    );
                }
            }
        }
    }

    #[test]
    fn blend_word_preserves_opaque_alpha_after_merge() {
        // `blend_span_alpha_4bpp` 在 blend_word 之后强制覆写第 4 字节，
        // 这里直接验证「blend_word 自身对 4 个通道都做混合」，
        // 覆写由调用方完成——确认两者组合后第 4 字节是 opaque_alpha。
        let alpha = 128u8;
        let opaque = 255u8;
        for is_rgb_order in [true, false] {
            let src = source_word(0x40, 0x80, 0xC0, opaque, is_rgb_order);
            let word = source_word(0x10, 0x20, 0x30, opaque, is_rgb_order);
            let blended = blend_word(word, src, alpha);
            // 模拟调用方的覆写：清掉高字节再填 opaque。
            let merged = (blended & 0x00FF_FFFF) | ((opaque as u32) << 24);
            assert_eq!(
                merged.to_le_bytes()[3],
                opaque,
                "第 4 字节必须是 opaque_alpha"
            );
            // 前三字节确实是混合结果（不等于任一端）。
            let bytes = merged.to_le_bytes();
            assert_ne!(&bytes[..3], &word.to_le_bytes()[..3]);
            assert_ne!(&bytes[..3], &src.to_le_bytes()[..3]);
        }
    }

    /// 代表性的「目标字 × 源字」组合：边界值、极值与步进采样。
    fn representative_words() -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        // 边界：全 0、全 1、交替、以及单通道极值。
        let specials = [
            0x0000_0000u32,
            0xFFFF_FFFF,
            0xFF00_00FF,
            0x00FF_FF00,
            0xFF00_0000,
            0x0000_00FF,
            0x8000_0080,
            0x0102_0304,
            0x8070_6050,
        ];
        for word in specials {
            for src in specials {
                pairs.push((word, src));
            }
        }
        // 步进采样：单通道线性扫描，其余通道固定。
        for step in 0..=255u16 {
            let v = step as u8;
            pairs.push((0x0000_0000 | (v as u32), 0x0000_0000 | (v as u32)));
            pairs.push((0xFFFF_FFFF ^ (v as u32), 0xFFFF_FFFF ^ (v as u32)));
        }
        pairs
    }

    /// 逐像素参考实现：对 4bpp 格式的一行做 alpha 混合。
    fn reference_blend_row_4bpp(
        row: &mut [u8],
        pixel_count: usize,
        color: &Color,
        is_rgb_order: bool,
        opaque_alpha: u8,
    ) {
        let alpha = color.a as u16;
        let inv = 255 - alpha;
        let (c0, c1, c2) = if is_rgb_order {
            (color.r, color.g, color.b)
        } else {
            (color.b, color.g, color.r)
        };
        for i in 0..pixel_count {
            let off = i * 4;
            row[off] = PixelFormat::blend_channel(c0, row[off], alpha, inv);
            row[off + 1] = PixelFormat::blend_channel(c1, row[off + 1], alpha, inv);
            row[off + 2] = PixelFormat::blend_channel(c2, row[off + 2], alpha, inv);
            row[off + 3] = opaque_alpha;
        }
    }

    #[test]
    fn blend_span_alpha_4bpp_matches_reference_including_odd_counts() {
        // 覆盖 SWAR 批量路径（4 的倍数）与逐像素回退路径（非倍数）。
        for pixel_count in [1usize, 2, 3, 4, 5, 7, 8, 9, 16, 17] {
            for is_rgb_order in [true, false] {
                for opaque_alpha in [0u8, 255] {
                    let color = Color::with_alpha(0x9A, 0x3C, 0x77, 0x80);
                    // 参考与被测必须从**同一份未混合的原始背景**出发。
                    let original = vec![0x5Au8; pixel_count * 4];

                    let mut expected = original.clone();
                    reference_blend_row_4bpp(
                        &mut expected,
                        pixel_count,
                        &color,
                        is_rgb_order,
                        opaque_alpha,
                    );

                    let mut actual = original;
                    // SAFETY: actual 恰好是 pixel_count * 4 字节，且由 Vec 分配保证对齐。
                    unsafe {
                        swar::blend_span_alpha_4bpp(
                            actual.as_mut_ptr(),
                            pixel_count,
                            &color,
                            is_rgb_order,
                            opaque_alpha,
                        );
                    }
                    assert_eq!(
                        actual, expected,
                        "count={pixel_count} rgb={is_rgb_order} opaque={opaque_alpha}"
                    );
                }
            }
        }
    }

    #[test]
    fn unaligned_rows_are_handled_by_the_fallback_not_the_swar_kernel() {
        // `blend_span_alpha_4bpp` 按 `u32` 读写，其 `# Safety` 明确要求行首
        // 4 字节对齐——对未对齐指针解引用是 UB，不能写成测试期望。
        //
        // 真实契约在调用方：`core::fill_span_alpha` 用
        // `row.align_offset(4) == 0` 判定，未对齐时**不调用**该内核，
        // 而是走逐像素回退路径。因此这里验证回退路径本身：
        // 「逐像素路径在未对齐起点上给出与对齐情形完全相同的结果」，
        // 且行首之前的字节不被触碰。
        let pixel_count = 12usize;
        let color = Color::with_alpha(0x11, 0x22, 0x33, 0x99);

        // 对齐行首（Vec 分配保证 4 字节对齐起点）。
        let mut aligned = vec![0x77u8; pixel_count * 4];
        reference_blend_row_4bpp(&mut aligned, pixel_count, &color, true, 255);

        // 未对齐行首：整段填充同样的背景色，再从偏移 1 处切出等长窗口。
        // 两段输入内容一致，结果才能逐一对应。
        let mut storage = vec![0x77u8; pixel_count * 4 + 1];
        storage[0] = 0xAA; // 行首前的哨兵字节
        let unaligned = &mut storage[1..];
        reference_blend_row_4bpp(unaligned, pixel_count, &color, true, 255);

        assert_eq!(
            unaligned,
            &aligned[..],
            "逐像素路径在未对齐起点上应与对齐情形结果一致"
        );
        assert_eq!(storage[0], 0xAA, "行首之前的哨兵字节不应被写入");
    }

    /// 逐像素参考实现：掩码混合。
    fn reference_blend_masked_row_4bpp(
        row: &mut [u8],
        mask: &[u8],
        pixel_count: usize,
        color: &Color,
        is_rgb_order: bool,
        opaque_alpha: u8,
    ) {
        let (c0, c1, c2) = if is_rgb_order {
            (color.r, color.g, color.b)
        } else {
            (color.b, color.g, color.r)
        };
        for i in 0..pixel_count {
            let m = mask[i];
            if m == 0 {
                // 掩码为 0：内核的 `keep_high` 保留原第 4 字节（值不变），
                // 前三字节也因 `blend_word(_,_,0)` 的恒等性不变，故整体跳过。
                continue;
            }
            let off = i * 4;
            if m == 255 {
                // 不透明像素：直写三通道，第 4 字节同样规范为 opaque_alpha。
                row[off] = c0;
                row[off + 1] = c1;
                row[off + 2] = c2;
            } else {
                // 0 < m < 255：混合 RGB 三通道。
                let inv = 255 - m as u16;
                row[off] = PixelFormat::blend_channel(c0, row[off], m as u16, inv);
                row[off + 1] = PixelFormat::blend_channel(c1, row[off + 1], m as u16, inv);
                row[off + 2] = PixelFormat::blend_channel(c2, row[off + 2], m as u16, inv);
            }
            // m > 0 时第 4 字节被规范为 opaque_alpha（源 alpha 不落盘）。
            row[off + 3] = opaque_alpha;
        }
    }

    #[test]
    fn blend_masked_row_4bpp_matches_reference_across_mask_shapes() {
        let color = Color::new(0xC0, 0x30, 0x90);
        // 掩码形态：全 0、全 1、交替、逐像素递增、以及尾部非 4 倍数。
        let shapes: [(&str, Vec<u8>); 6] = [
            ("全 0", vec![0u8; 20]),
            ("全 255", vec![255u8; 20]),
            (
                "交替",
                (0..20).map(|i| if i % 2 == 0 { 255 } else { 0 }).collect(),
            ),
            ("逐像素递增", (0..20).map(|i| (i * 12) as u8).collect()),
            ("全 128", vec![128u8; 20]),
            ("单点", {
                let mut m = vec![0u8; 20];
                m[7] = 255;
                m
            }),
        ];
        for (name, mask) in shapes {
            let pixel_count = mask.len();
            for is_rgb_order in [true, false] {
                for opaque_alpha in [0u8, 255] {
                    // 参考与被测必须从**同一份未混合的原始背景**出发。
                    let original = vec![0x5Au8; pixel_count * 4];

                    let mut expected = original.clone();
                    reference_blend_masked_row_4bpp(
                        &mut expected,
                        &mask,
                        pixel_count,
                        &color,
                        is_rgb_order,
                        opaque_alpha,
                    );

                    let mut actual = original;
                    // SAFETY: actual 长度恰为 pixel_count * 4 且 4 字节对齐；
                    // mask 长度恰为 pixel_count，满足 # Safety 契约。
                    unsafe {
                        swar::blend_masked_row_4bpp(
                            actual.as_mut_ptr(),
                            mask.as_ptr(),
                            pixel_count,
                            &color,
                            is_rgb_order,
                            opaque_alpha,
                        );
                    }
                    assert_eq!(
                        actual, expected,
                        "掩码 {name}, rgb={is_rgb_order}, opaque={opaque_alpha}"
                    );
                }
            }
        }
    }

    #[test]
    fn blend_masked_row_4bpp_preserves_pixels_where_mask_is_zero() {
        // 掩码全 0 时必须一个字节都不写（背景逐字节不变）。
        let pixel_count = 16usize;
        let original: Vec<u8> = (0..(pixel_count * 4))
            .map(|i| (i * 7 % 251) as u8)
            .collect();
        let mask = vec![0u8; pixel_count];
        let mut actual = original.clone();
        // SAFETY: 长度与对齐均满足契约。
        unsafe {
            swar::blend_masked_row_4bpp(
                actual.as_mut_ptr(),
                mask.as_ptr(),
                pixel_count,
                &Color::new(1, 2, 3),
                true,
                255,
            );
        }
        assert_eq!(actual, original, "掩码全 0 时不应写入任何像素");
    }

    #[test]
    fn blend_masked_row_4bpp_writes_opaque_alpha_for_nonzero_mask() {
        // 掩码非 0 的像素：第 4 字节恒为 opaque_alpha，源 alpha 从不落盘。
        let pixel_count = 8usize;
        for opaque_alpha in [0u8, 255] {
            let mask = vec![200u8; pixel_count];
            let mut row = vec![0x11u8; pixel_count * 4];
            // SAFETY: 长度与对齐均满足契约。
            unsafe {
                swar::blend_masked_row_4bpp(
                    row.as_mut_ptr(),
                    mask.as_ptr(),
                    pixel_count,
                    &Color::with_alpha(9, 9, 9, 33),
                    true,
                    opaque_alpha,
                );
            }
            for i in 0..pixel_count {
                assert_eq!(
                    row[i * 4 + 3],
                    opaque_alpha,
                    "掩码非 0 的像素第 4 字节应为 opaque_alpha"
                );
            }
        }
    }

    #[test]
    fn swar_alpha_channel_is_never_persisted_from_source() {
        // `blend_span_alpha_4bpp` 对 4bpp 格式：源 alpha 不参与落盘，
        // 第 4 字节恒为格式自身的 opaque_alpha。
        let pixel_count = 8usize;
        for source_alpha in [1u8, 128, 254] {
            for opaque_alpha in [0u8, 255] {
                let color = Color::with_alpha(0x40, 0x50, 0x60, source_alpha);
                let mut row = vec![0u8; pixel_count * 4];
                // SAFETY: 长度与对齐均满足契约。
                unsafe {
                    swar::blend_span_alpha_4bpp(
                        row.as_mut_ptr(),
                        pixel_count,
                        &color,
                        true,
                        opaque_alpha,
                    );
                }
                for i in 0..pixel_count {
                    assert_eq!(
                        row[i * 4 + 3],
                        opaque_alpha,
                        "源 alpha={source_alpha} 泄漏到了落盘字节"
                    );
                }
            }
        }
    }
}
