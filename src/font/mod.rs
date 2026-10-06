//! 字体抽象：位图字体与 TrueType 字体的统一入口。
//!
//! # 概述
//!
//! [`Font`] trait 把「度量」与「光栅化」拆成两个阶段，这是文本栈的通用做法
//! （fontdue 自身的 `metrics()` / `rasterize()` 也是这个分离）。拆分的直接
//! 收益是**返回值不再借用 `&self`**：
//!
//! | 类型 | 存储来源 | 需要 `alloc` | 容量不足时 |
//! |------|----------|--------------|-----------|
//! | [`GlyphLayout`] / [`FontMetrics`] / [`PenX`] | 纯值，无存储 | 否 | — |
//! | [`Char`] | 调用方已有的掩码切片 | 否 | 构造时 panic |
//! | [`Font::rasterize`] | 调用方借出的 `&mut [u8]` | 否 | **panic** |
//!
//! # 分配由谁承担
//!
//! **默认契约是「库内绝不分配」**——所有缓冲由调用方提供，与
//! [`crate::Renderer::with_back_buffer`] 同一约定。开启 `alloc` feature 是
//! **显式放弃**这条契约，以换取两个免 scratch 的便利方法：
//!
//! | 方法 | 需要 `alloc` | 说明 |
//! |------|-------------|------|
//! | [`Renderer::draw_char`] / [`Renderer::draw_text`] | 否 | scratch 由调用方出（栈上定长数组即可） |
//! | [`Renderer::draw_char_owned`] / [`Renderer::draw_text_owned`] | **是** | scratch 由库内按 [`GlyphLayout::mask_len`] 精确分配 |
//!
//! 签名**不按 feature 分裂**：`draw_char` / `draw_text` 在任何构建下都收
//! scratch 参数，所以调用点不会随 feature 改变而需要改代码。
//!
//! | feature | 库内是否分配 |
//! |---------|------------|
//! | 无 `alloc` | **绝不**。`Font::rasterize` 的缓冲、`draw_glyph` 的掩码都来自调用方 |
//! | `alloc` | 仅 `*_owned` 方法内分配，且按需精确申请 |
//! | `font_truetype` | 隐含 `alloc`（fontdue 本身是 `no_std` + `alloc` 依赖） |
//!
//! 典型用法：
//!
//! ```text
//! // 无 alloc：栈上定长缓冲，零分配
//! let mut scratch = [0u8; 64 * 64];
//! renderer.draw_text(at, "Hello", 24.0, color, &font, &mut scratch);
//!
//! // 有 alloc：库内按字号精确分配
//! renderer.draw_text_owned(at, "Hello", 24.0, color, &font);
//! ```

//! # 坐标系
//!
//! 排版以**基线**（baseline）原点为准，与 `core.rs` 的 Y 轴向下约定一致：
//! `origin.y` 是基线所在的行。bearing 沿用字体惯例——
//! [`GlyphLayout::bearing_y`] 以「向上为正」表示到基线的距离，
//! [`GlyphLayout::bearing_x`] 可为负（悬挂部 / 斜体）。
//!
//! # 浮点
//!
//! 字号 `px` 是 `f32`（像素 / Em unit）。这是**接口上的取舍**，不是
//! `programmable` 独占浮点的破坏——`font_truetype` 内部的 fontdue 光栅化
//! 本来就以浮点累加器工作（`scale_factor(px)`、亚像素偏移）。内核态目标
//! 需要的是**关闭 `font_truetype`**，而非在该 feature 下假装能定点。
//! `font_bitmap` 路径不产生任何浮点运算。
//!
//! 浮点只出现在「度量」这一侧；**排版累加走 [`PenX`] 的 26.6 定点**，
//! 所以长串文字不会因 `f32` 尾数有限而漂移。

#[cfg(feature = "alloc")]
extern crate alloc;

use crate::{Pixel, Renderer, color::Color};

#[cfg(feature = "font_bitmap")]
mod font_8x16;
#[cfg(feature = "font_truetype")]
pub mod ttf;
#[cfg(feature = "font_bitmap")]
use font_8x16::DEFAULT_FONT;

// Hand-drawn 1bpp replacement glyph: a framed X, expanded at compile time.
const UNKNOWN_BITMAP: [u8; 16] = [
    0xFF, 0x81, 0xC3, 0xA5, 0x99, 0x99, 0xA5, 0xC3, 0xC3, 0xA5, 0x99, 0x99, 0xA5, 0xC3, 0x81, 0xFF,
];

const fn expand_unknown_bitmap() -> [u8; 128] {
    let mut expanded = [0u8; 128];
    let mut row = 0;
    while row < 16 {
        let mut col = 0;
        while col < 8 {
            expanded[row * 8 + col] = if UNKNOWN_BITMAP[row] & (0x80 >> col) != 0 {
                255
            } else {
                0
            };
            col += 1;
        }
        row += 1;
    }
    expanded
}

static UNKNOWN_MASK: [u8; 128] = expand_unknown_bitmap();

/// 无字体实现能提供该字符时使用的手绘替代字形。
///
/// 它是位图字形而非任何字体的度量：宽 8、基线在第 12 行
///（ascent 12 / descent 4），与内置 8×16 点阵字体一致。
pub static UNKNOWN_CHAR: Char<'static> = Char::new(&UNKNOWN_MASK, (8, 16));

/// 替代字形的步进宽度（像素）。
const UNKNOWN_ADVANCE: f32 = 8.0;

/// 单个字形的度量。零分配——全部字段都是值。
///
/// 由 [`Font::layout`] 产出，是该字形在指定字号下的**唯一尺寸真相**：
/// 绘制与步进都应从这里取值，不要在调用方硬编码字宽。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphLayout {
    /// 光栅位图的像素宽度（每行 `width` 个字节）。
    pub width: usize,
    /// 光栅位图的像素高度。
    pub height: usize,
    /// 位图左沿相对**基线原点**的水平偏移，可为负（悬挂部 / 斜体）。
    ///
    /// 为负意味着位图有一部分落在原点的**左侧**——绘制时必须同时裁掉源掩码
    /// 的左边界，否则字形会整体右移（见 [`Renderer::draw_glyph`]）。
    pub bearing_x: i32,
    /// 位图顶沿到基线的距离（向上为正）。字形越高该值越大。
    pub bearing_y: i32,
    /// 排版前进宽度，单位为像素（`f32`，可含亚像素小数）。
    ///
    /// 逐字排版时用 [`PenX::advance`] 推进画笔，不要在调用方直接累加
    /// `f32`——串越长误差越大。
    pub advance: f32,
}

impl GlyphLayout {
    /// 紧凑掩码所需的字节数（`width × height`，即 `mask_pitch == width`）。
    ///
    /// 用 `checked_mul` 而非裸乘：调用方通常拿它去切片定长栈数组，
    /// 回绕出的长度会让切片凭空成立。溢出返回 [`None`]。
    #[must_use]
    pub const fn mask_len(&self) -> Option<usize> {
        self.width.checked_mul(self.height)
    }

    /// 空字形（空白、零宽字符）。
    #[must_use]
    pub const fn empty() -> Self {
        GlyphLayout {
            width: 0,
            height: 0,
            bearing_x: 0,
            bearing_y: 0,
            advance: 0.0,
        }
    }
}

/// 字体在指定字号下的行度量。零分配。
///
/// 行高 = `ascent + descent + line_gap`。多行排版时相邻基线的间距就是这值，
/// 不要用「字形高度 × 行数」硬算——比例字体的行高与字形高度无关。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    /// 基线以上的距离（像素）。首行基线位于 `top_left.y + ascent`。
    pub ascent: f32,
    /// 基线以下的距离（像素），非负。
    pub descent: f32,
    /// 行与行之间的额外间距（像素），非负。
    pub line_gap: f32,
}

impl FontMetrics {
    /// 推荐的单行行距（像素）。
    #[must_use]
    pub const fn line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }

    /// 三个字段是否都是有限值。
    ///
    /// 排版入口用它拦截损坏的度量：NaN 会污染画笔，导致**后续每个字**都
    /// 落在错误位置，比直接放弃整段文本难查得多。
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.ascent.is_finite() && self.descent.is_finite() && self.line_gap.is_finite()
    }
}

/// 亚像素精度的画笔位置（**26.16 定点**：整数部分为像素 + 16 位小数）。
///
/// 逐字排版必须用它而不是裸 `f32` 累加：`f32` 只有 24 位尾数，在 x≈1600 处
/// 的 ULP 已达 0.00019 px，一行 1600 次累加最坏可漂移 0.1px 以上，多行累积
/// 后肉眼可见地左右抖。整数累加器的加法精确，且 1/65536 px 的网格能精确表示
/// 所有常见 `advance`，故量化误差也恒为 0。
///
/// 这与 [`crate::angle::Angle`] 用圈值而非「度 × 系数」是同一个取舍：
/// 累加密集、需要长期稳定的量，就该用定点而不是浮点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PenX(i32);

impl PenX {
    /// 1/65536 像素的定点刻度（26.16 定点）。
    ///
    /// 小数位数取 **16** 而非字体业内的 26.6（1/64），因为累加器每次步进都要
    /// 把 `advance` 量化到这个网格上，而**量化误差会随字距线性累积**：
    /// 1/64 px 的网格表示不了 `1.0078125 = 1 + 1/128`（常见于比例字体），
    /// 1600 字的一行会漂移 12.5 px——肉眼可见的整行错位。1/65536 px 下
    /// 所有常见 `advance` 都能精确表示，误差恒为 0。
    ///
    /// 代价是 `i32` 的整数部分从 2^25 降到 2^15（±32768 px 画笔范围），
    /// 对任何真实帧缓冲都绰绰有余。
    const SUBPIXEL_ONE: i32 = 1 << 16;
    /// 画笔坐标上限（像素）。对应 `i32::MAX / 2^16` 的整数像素部分。
    const MAX_PEN_PX: f32 = 32_767.0;

    /// 由像素值建立画笔位置。非有限或超出可表示范围返回 [`None`]。
    ///
    /// 不能靠 `as` 兜底：`f32::NAN as i32` 是 0，会把 NaN 静默变成屏幕原点。
    #[must_use]
    pub fn from_px(px: f32) -> Option<Self> {
        Self::quantize(px).map(PenX)
    }

    /// 把 `f32` 量化成 1/65536 像素的整数刻度。
    #[inline]
    fn quantize(value: f32) -> Option<i32> {
        if !value.is_finite() {
            return None;
        }
        // 范围检查必须按「量化后的刻度数是否装得进 i32」来判断，而不是按
        // 像素值和 `MAX_PEN_PX` 比：1/65536 的刻度空间只有 ±32768 px，
        // 若先按 `MAX_PEN_PX` 放行，`value * SUBPIXEL_ONE` 仍可能溢出
        // `as i32`（饱和后静默变成 i32::MIN）。这里显式比刻度数。
        if value.abs() > Self::MAX_PEN_PX {
            return None;
        }
        // 手写四舍五入：`no_std` 且无 libm 时 `f32::round` / `f32::trunc` 都不可用
        // ——与 `utils::floor_to_usize` / `ceil_to_usize` 记录的是同一条限制。
        // 截断改用 `as i32`（Rust 的 `f32 as i32` 向零取整，对负数即 floor，
        // 口径与 `floor_to_usize` 一致），`NaN` 已被上面的 `is_finite` 拦掉，
        // 不会被 `as` 的饱和语义蒙成 0。
        let scaled = value * Self::SUBPIXEL_ONE as f32;
        let truncated = scaled as i32;
        // 只在**小数部分**达到半格时进位。不能写成 `scaled >= truncated as f32`：
        // 对整数输入（如 0.5px → 32.0）那个条件恒成立，会让每个整数刻度都被
        // 多进一位——1000 次步进就凭空漂移 1000/64 ≈ 15.6 px。
        // `as i32` 对负数是向零取整，故 `scaled - truncated` 恒为小数部分 [0, 1)。
        let rounded = if scaled - truncated as f32 >= 0.5 {
            truncated + 1
        } else {
            truncated
        };
        // 上面的范围检查保证 `|scaled| <= i32::MAX`，故 `truncated + 1` 不会溢出。
        Some(rounded)
    }

    /// 就地推进 `dx` 像素。
    ///
    /// 返回 `false` 表示 `dx` 不可表示或累加后越界，此时**画笔保持原值**——
    /// 静默钳位会让后续字形叠在一起，比明确失败更难排查。
    #[must_use]
    pub fn advance(&mut self, dx: f32) -> bool {
        match Self::quantize(dx) {
            Some(delta) => match self.0.checked_add(delta) {
                Some(summed) => {
                    self.0 = summed;
                    true
                }
                None => false,
            },
            None => false,
        }
    }

    /// 画笔的像素位置（含亚像素小数）。
    #[must_use]
    pub fn to_px(self) -> f32 {
        self.0 as f32 / Self::SUBPIXEL_ONE as f32
    }

    /// 截断到整数像素的 x 坐标。
    ///
    /// 负值返回 [`None`]（[`Pixel::x`] 是 `usize`，表达不了负坐标）。
    #[must_use]
    pub fn pixel_x(self) -> Option<usize> {
        // `>>` 对负数是算术右移（等价 floor），与「取整数部分」的语义一致；
        // 负值在下面拦掉，不影响结果。
        let whole = self.0 >> Self::SUBPIXEL_BITS;
        if whole < 0 {
            None
        } else {
            Some(whole as usize)
        }
    }

    /// 1/65536 像素刻度下的位偏移。
    const SUBPIXEL_BITS: i32 = 16;
}

/// 整段文本排版所需的字体级度量。
///
/// [`GlyphLayout`] 描述单个字形怎么画，这个描述**整段文字**怎么流：
/// 行高与单字名义步进。它是纯描述、不含可变状态——画笔位置由 [`PenX`] 持有。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontLayout {
    /// 字体在当前字号下的行度量。
    pub metrics: FontMetrics,
    /// 单字名义步进宽度（像素）。缩排 / 全角对齐等场景会用到。
    pub advance: f32,
}

impl FontLayout {
    /// 新建一份文本布局描述。
    #[must_use]
    pub const fn new(metrics: FontMetrics, advance: f32) -> Self {
        FontLayout {
            metrics,
            advance,
        }
    }

    /// 行高（像素）：相邻两行基线的间距。
    #[must_use]
    pub const fn line_height(&self) -> f32 {
        self.metrics.line_height()
    }
}

/// 字体实现：把字符变成 8bpp Alpha 掩码。
///
/// # 契约
///
/// * [`layout`](Font::layout) 与 [`metrics`](Font::metrics) **不得分配**——
///   返回纯值；字体的内部存储（位图表 / fontdue 的 `Font`）由实现方自持。
/// * [`rasterize`](Font::rasterize) 把掩码写进调用方借出的 `&mut [u8]`，
///   长度必须 ≥ [`GlyphLayout::mask_len`]（`width × height`，逐行紧凑排列），
///   否则 **panic**。这与 `blit_mask` 的策略**相反**且是刻意的：缓冲尺寸是
///   调用方拿上一步 [`layout`](Font::layout) 的返回值算出来的，算错属于调用方
///   bug，不该被降级成「静默画不出字」。
/// * [`has_glyph`](Font::has_glyph) 必须与 [`layout`](Font::layout) 的
///   `Some`/`None` 判定一致。部分后端（如 fontdue 的 `rasterize`）会对缺字
///   回退到 `.notdef` 字形并给出结果，实现必须先行 `has_glyph` 判定，
///   否则缺字会被当成正常字形画出豆腐块之外的怪东西。
pub trait Font {
    /// 字体是否含有该字符的字形。
    fn has_glyph(&self, c: char) -> bool;

    /// 取该字符在 `px` 字号下的度量。零分配。
    ///
    /// 缺字、`px` 非有限或 `px <= 0` 都返回 [`None`]。
    fn layout(&self, c: char, px: f32) -> Option<GlyphLayout>;

    /// 取字体在 `px` 字号下的行度量。零分配。
    fn metrics(&self, px: f32) -> FontMetrics;

    /// 把字形光栅化为紧凑 8bpp Alpha 掩码（逐行 `width` 字节，0=透明）。
    ///
    /// `mask` 的长度必须 ≥ `layout.mask_len()`，否则 **panic**。
    /// 返回实际写入的字节数；缺字返回 [`None`]。
    ///
    /// `layout` 必须来自同一次 [`layout`](Font::layout) 查询——它是像素尺寸的
    /// 真相来源，实现不应重算一遍（否则两次调用之间的状态变化会让写入长度
    /// 与掩码错位）。
    ///
    /// # 恐慌
    ///
    /// `mask.len() < layout.mask_len()` 时 panic。
    fn rasterize(
        &self,
        c: char,
        px: f32,
        layout: &GlyphLayout,
        mask: &mut [u8],
    ) -> Option<usize>;

}

/// 预先光栅化好的字形：尺寸 + 掩码切片。
///
/// 与 [`GlyphLayout`] 的区别是它**带着像素**，供不想每帧重复光栅化的调用方
/// 使用（例如把 ASCII 表缓存成 `[(Char, [u8; N]); 128]`）。
#[derive(Clone, Copy)]
pub struct Char<'a> {
    size: (usize, usize),
    /// 每个字节代表一个像素的不透明度（0=透明，255=完全不透明）。
    mask: &'a [u8],
}

impl<'a> Char<'a> {
    /// 由掩码与尺寸构造。
    ///
    /// `mask.len()` 必须 ≥ `size.0 × size.1`（逐行紧凑排列），否则 **panic**——
    /// 与 [`Font::rasterize`] 的容量策略一致。
    ///
    /// # 恐慌
    ///
    /// `mask` 长度不足 `size.0 × size.1` 时 panic。
    #[must_use]
    pub const fn new(mask: &'a [u8], size: (usize, usize)) -> Self {
        assert!(
            mask.len() >= size.0.saturating_mul(size.1),
            "Char::new: mask 长度小于 size 面积"
        );
        Char { size, mask }
    }

    /// 位图尺寸（宽, 高）。
    #[must_use]
    pub const fn size(&self) -> (usize, usize) {
        self.size
    }

    /// 8bpp Alpha 掩码切片，长度 ≥ `size.0 × size.1`。
    #[must_use]
    pub const fn mask(&self) -> &'a [u8] {
        self.mask
    }
}

impl<'a> Renderer<'a> {
    /// 绘制一个**已光栅化**的字形，按 `layout` 的 bearing 相对基线原点定位。
    ///
    /// `origin` 是基线原点，不是位图的左上角——这是它与
    /// [`Renderer::draw_char`] 最重要的区别，也是排版正确的前提。
    ///
    /// 裁剪分两层：
    ///
    /// 1. **右侧/下侧**交回 [`Renderer::blit_mask`]（它只画帧缓冲内的部分）。
    /// 2. **左侧/上侧**必须在这里处理：`bearing_x` 为负时位图有一部分在原点
    ///    左侧，而 `blit_mask` 无从表达负的 `dst_top_left.x`（[`Pixel`] 的 x 是
    ///    `usize`），所以要**同时裁掉源掩码的左边界**并把落点右移。若只裁目标
    ///    不裁源，字形会整体右移 `|bearing_x|` 像素——这是 TTF 渲染里最常见的
    ///    一个 bug。
    ///
    /// # 恐慌
    ///
    /// 不会 panic：所有越界情形静默退化（位图完全在画面外则什么都不画）。
    pub fn draw_glyph(
        &mut self,
        origin: Pixel,
        layout: &GlyphLayout,
        mask: &[u8],
        color: Color,
    ) {
        let (w, h) = (layout.width, layout.height);
        if w == 0 || h == 0 {
            return;
        }
        // 掩码长度不足时不写：`Font::rasterize` 对容量已 panic 过，走到这里
        // 说明调用方自己拼的切片与 layout 不匹配，静默退化比二次 panic 温和。
        let Some(needed) = layout.mask_len() else {
            return;
        };
        if mask.len() < needed {
            return;
        }

        // 原点经饱和转换进 i64：库内禁止裸 `as` 做 usize→i64。
        let pen_x = crate::utils::sat_i64(origin.x);
        let base_y = crate::utils::sat_i64(origin.y);
        // bearing_y 以「向上为正」表示，故落点是基线减去 bearing。
        let Some(dst_x) = pen_x.checked_add(layout.bearing_x as i64) else {
            return;
        };
        let Some(dst_y) = base_y.checked_sub(layout.bearing_y as i64) else {
            return;
        };
        // bearing 恒为 `i32`，`as i64` 恒等；上面的 `checked_*` 兜住原点的饱和值。
        let fb_width = self.info().width as i64;
        let fb_height = self.info().height as i64;

        // 源掩码的左边界裁剪量。
        let mut src_skip: usize = 0;
        let mut draw_x = dst_x;
        if draw_x < 0 {
            // `-draw_x` 在 `draw_x < 0` 时不会溢出（`draw_x > i64::MIN`：
            // 它由 `pen_x >= 0` 加 `|bearing_x| <= i32::MAX` 得到）。
            src_skip = (-draw_x) as usize;
            if src_skip >= w {
                return; // 整幅位图在画面左侧
            }
            draw_x = 0;
        }
        // 整幅位图在画面右侧。
        if draw_x >= fb_width {
            return;
        }
        let draw_w = w - src_skip;

        // 顶部裁剪同理：dst_y 为负时丢弃上方的整行，并把落点钳到 0。
        // **必须**在这里改写 dst_y 本身——`Pixel::y` 是 `usize`，把负值
        // 直接 `as usize` 会得到一个巨大的正数，位图就被推到画面外，
        // 顶部裁剪等于没做。
        let mut src_row_skip: usize = 0;
        let mut draw_y = dst_y;
        if draw_y < 0 {
            src_row_skip = (-draw_y) as usize;
            if src_row_skip >= h {
                return;
            }
            draw_y = 0;
        }
        // 整幅位图在帧缓冲下方。
        if draw_y >= fb_height {
            return;
        }
        let draw_h = h - src_row_skip;

        // 目标坐标已钳到 `[0, fb)` 内；上面各分支保证其非负。
        let top_left = Pixel::new(draw_x as usize, draw_y as usize);
        // 源切片：先跳 `src_row_skip` 整行，再每行跳 `src_skip` 列。
        //
        // 关键：`mask_pitch` 必须从 `w`（原字形宽度）**改成 `draw_w`**。
        // `blit_mask` 会校验 `mask.len() >= mask_pitch × height`，若仍传 `w`，
        // 被裁掉左列后的短切片就会被判为「长度不足」而**静默不画**——
        // 表现就是所有带悬挂部的字形（斜体、f、j）凭空消失。
        let src_off = src_row_skip * w + src_skip;
        debug_assert!(src_off <= needed, "draw_glyph: 源偏移越过掩码");
        let src = &mask[src_off..];
        self.blit_mask(top_left, draw_w, draw_h, src, draw_w, color);
    }

    /// 按基线原点绘制单个字符：查度量 → 光栅化 → [`Renderer::draw_glyph`]。
    ///
    /// 这是给「一次性绘制、不关心排版」的调用方的便利方法。
    /// **成段排版请用 [`Renderer::draw_text`]**——本方法每字都重新光栅化，
    /// 也不处理字距调整。
    ///
    /// `font` 为 [`None`] 时退回 `font_bitmap` 的内置 8×16 点阵字体；该
    /// feature 关闭且 `font` 仍为 [`None`] 时不绘制。字符缺字形时使用手绘的
    /// 替代字形。
    ///
    /// `scratch` 是**光栅化用的临时缓冲**，长度需 ≥ 该字形的
    /// [`GlyphLayout::mask_len`]。库内在任何 feature 组合下都不分配，
    /// 它必须由调用方提供（见模块文档「分配由谁承担」）。
    ///
    /// 签名**不按 feature 分裂**：位图字体的掩码虽是编译期常量表，调用方
    /// 仍需传一个占位缓冲。开了 `alloc` 时用
    /// [`Renderer::draw_char_owned`] 可以省掉这个参数。
    ///
    /// # 恐慌
    ///
    /// `scratch` 不足时 [`Font::rasterize`] 会 panic。这是调用方未按
    /// [`Font::layout`] 返回的尺寸准备缓冲，属调用方 bug。
    pub fn draw_char(
        &mut self,
        origin: Pixel,
        c: char,
        px: f32,
        color: Color,
        font: Option<&dyn Font>,
        scratch: &mut [u8],
    ) {
        let Some(font) = Self::resolve_font(font) else {
            return;
        };
        match Self::rasterize_char(font, c, px, scratch) {
            Some((layout, mask)) => self.draw_glyph(origin, &layout, mask, color),
            None => self.draw_unknown_char(origin, color),
        }
    }

    /// [`Renderer::draw_char`] 的免 scratch 变体：缓冲由库内按需分配。
    ///
    /// 仅在 `alloc` feature 下存在 —— 它需要堆分配器。容量按
    /// [`GlyphLayout::mask_len`] **精确**申请，不多不少，因此超大字号
    /// （几百 px 的字形）也能画。
    ///
    /// 若不想引入堆，用 [`Renderer::draw_char`] 在栈上开一块够大的缓冲即可：
    /// 常见字号（≤ 64px）下 64×64 = 4096 字节绰绰有余。
    #[cfg(feature = "alloc")]
    pub fn draw_char_owned(
        &mut self,
        origin: Pixel,
        c: char,
        px: f32,
        color: Color,
        font: Option<&dyn Font>,
    ) {
        let Some(font) = Self::resolve_font(font) else {
            return;
        };
        let Some(layout) = font.layout(c, px) else {
            self.draw_unknown_char(origin, color);
            return;
        };
        let Some(needed) = layout.mask_len() else {
            return;
        };
        if needed == 0 {
            return;
        }
        let mut scratch = alloc::vec![0u8; needed];
        match font.rasterize(c, px, &layout, &mut scratch) {
            Some(written) => self.draw_glyph(origin, &layout, &scratch[..written], color),
            None => self.draw_unknown_char(origin, color),
        }
    }

    /// 绘制单行/多行文本，自动推进画笔并对齐基线。
    ///
    /// 处理四件手写循环最容易漏的事：
    ///
    /// * **基线定位**：首行基线在 `top_left.y + ascent`，后续行每
    ///   `line_height()` 一行。
    /// * **亚像素步进**：画笔 x 由 [`PenX`] 的 26.6 定点累加器持有，长串不漂移。
    /// * **换行**：`\n` 结束当前行，基线下移一个行高（行距取自
    ///   [`FontMetrics::line_height`]）。
    /// * **缺字**：落到替代字形，不中断整段。
    ///
    /// 不做字距调整（`Font` trait 当前未暴露 kerning 查询）。其他控制字符
    /// （`\r`、`\t` 等）按普通字符处理：有点阵就画，没有则落替代字形。
    ///
    /// 画笔落在帧缓冲左侧（`pen_x < 0`）的字形会被跳过——它已经整幅在画面外。
    /// 需支持横向滚动的调用方应先用 [`PenX::pixel_x`] 之类的换算自行偏移。
    ///
    /// `scratch` 是**光栅化用的临时缓冲**，长度需能容纳最大号字的位图
    /// （该字体在 `px` 下最大单字的 `width × height`）。库内在无 `alloc` 的
    /// 构建里绝不分配，它必须由调用方提供。
    ///
    /// 开了 `alloc` 时可用 [`Renderer::draw_text_owned`] 省掉这个参数。
    ///
    /// # 恐慌
    ///
    /// `scratch` 不足时 [`Font::rasterize`] 会 panic，属调用方 bug。
    pub fn draw_text(
        &mut self,
        top_left: Pixel,
        text: &str,
        px: f32,
        color: Color,
        font: &dyn Font,
        scratch: &mut [u8],
    ) {
        self.layout_text(top_left, text, px, color, font, scratch);
    }

    /// [`Renderer::draw_text`] 的免 scratch 变体，缓冲由库内精确分配。
    ///
    /// 仅在 `alloc` feature 下存在。适合不想自己算「最大字形位图多大」的
    /// 用方——代价是每个字符一次堆分配（每帧大量文本时应改用带 scratch
    /// 的 [`Renderer::draw_text`]，或把字形缓存成 [`Char`]）。
    #[cfg(feature = "alloc")]
    pub fn draw_text_owned(
        &mut self,
        top_left: Pixel,
        text: &str,
        px: f32,
        color: Color,
        font: &dyn Font,
    ) {
        // 上界取「该字号下单字位图的面积」。经验值：正文字形的高度不超过
        // 字号，宽度也不超过字号，故 `(px + 9)²` 足够；那 9 = 1 向上取整
        // + 8 余量，吸收带下伸部/悬挂部的字（它们可能略高于 em）。
        //
        // `f32::ceil` 在 `no_std` 下不可用（见 `utils::ceil_to_usize` 的说明），
        // 故用整数加法完成上取整。
        if !px.is_finite() || px <= 0.0 {
            return;
        }
        // `px as i64` 已完成转换（非有限的 `px` 在上面已 return），且 Rust 的
        // `as` 是饱和转换、不会回绕成负数。剩下的风险只有转 `usize`（负值）
        // 与平方溢出。
        let Ok(side) = usize::try_from(px as i64 + 9) else {
            return;
        };
        let Some(cap) = side.checked_mul(side) else {
            return;
        };
        let mut scratch = alloc::vec![0u8; cap];
        self.layout_text(top_left, text, px, color, font, &mut scratch);
    }

    /// 排版循环的共同实现。
    ///
    /// 拆出这个私有内核，是为了让 [`Renderer::draw_text`] 与
    /// [`Renderer::draw_text_owned`] 共用**同一份**基线定位 / 换行 / 画笔推进
    /// 逻辑——两份复制品迟早会改一处漏一处。
    fn layout_text(
        &mut self,
        top_left: Pixel,
        text: &str,
        px: f32,
        color: Color,
        font: &dyn Font,
        scratch: &mut [u8],
    ) {
        let metrics = font.metrics(px);
        // 非有限度量必须整段放弃：NaN 会污染画笔，让后续每个字都落错位置。
        if !metrics.is_finite() {
            return;
        }
        let line_height = metrics.line_height();
        if !line_height.is_finite() || line_height <= 0.0 {
            return;
        }
        // 首行基线。后续行由「首行基线 + 行号 × 行高」一次算出，
        // 不做逐行累加，避免与 PenX 同类的漂移。
        let first_baseline = metrics.ascent + top_left.y as f32;
        if !first_baseline.is_finite() {
            return;
        }
        let Some(mut pen) = PenX::from_px(top_left.x as f32) else {
            return;
        };
        // 当前行的基线 y。`\n` 时按「首行基线 + 行号 × 行高」**重算**，
        // 不做逐行累加，避免与 PenX 同类的浮点漂移。
        let mut baseline = first_baseline;

        for c in text.chars() {
            if c == '\n' {
                baseline += line_height;
                if !baseline.is_finite() || baseline > PenX::MAX_PEN_PX {
                    return;
                }
                // 换行同时重置画笔到行首。
                let Some(reset) = PenX::from_px(top_left.x as f32) else {
                    return;
                };
                pen = reset;
                continue;
            }
            let Some(y) = row_y(baseline) else {
                return;
            };
            let Some(x) = pen.pixel_x() else {
                // 画笔已落到帧缓冲左侧，整幅字形都在画面外：仍要推进画笔，
                // 否则后续字符的 x 不前进、整行会叠在一起。跳过的只是绘制。
                let advance = font.layout(c, px).map_or(UNKNOWN_ADVANCE, |g| g.advance);
                if !pen.advance(advance) {
                    return;
                }
                continue;
            };
            let origin = Pixel::new(x, y);
            match Self::rasterize_char(font, c, px, scratch) {
                Some((layout, mask)) => {
                    self.draw_glyph(origin, &layout, mask, color);
                    // bearing_x 已在 draw_glyph 里整像素右移，这里只用 advance，
                    // 避免双重偏移。
                    if !pen.advance(layout.advance) {
                        return;
                    }
                }
                None => {
                    self.draw_unknown_char(origin, color);
                    // 替代字形宽度固定，仍按其宽度推进以免整行塌缩。
                    if !pen.advance(UNKNOWN_ADVANCE) {
                        return;
                    }
                }
            }
        }
    }

    /// 解析要用的字体：`None` 时退回内置 8×16 点阵字体。
    ///
    /// `font_bitmap` 关闭且未指定字体时返回 [`None`]（无可用字体，不绘制）。
    fn resolve_font(font: Option<&dyn Font>) -> Option<&dyn Font> {
        match font {
            Some(f) => Some(f),
            #[cfg(feature = "font_bitmap")]
            None => Some(&DEFAULT_FONT),
            #[cfg(not(feature = "font_bitmap"))]
            None => None,
        }
    }

    /// 光栅化单个字符进 `scratch`，返回度量与**可绘制的掩码切片**。
    ///
    /// 缺字 / 无效尺寸返回 [`None`]。零宽字形（空格等）返回 `(layout, &[])`：
    /// 没有像素可写，但**度量仍然有效**，调用方需据此步进。
    ///
    /// 掩码借用自 `scratch`，故其生命周期与 `scratch` 一致。
    fn rasterize_char<'s>(
        font: &dyn Font,
        c: char,
        px: f32,
        scratch: &'s mut [u8],
    ) -> Option<(GlyphLayout, &'s [u8])> {
        let layout = font.layout(c, px)?;
        // 容量校验交给 `Font::rasterize`（不足即 panic），此处不重复判定——
        // 少一处判定就少一处「两处口径不一致」的可能。
        let written = font.rasterize(c, px, &layout, scratch)?;
        Some((layout, &scratch[..written]))
    }

    /// 绘制手绘的替代字形（缺字时的兜底）。
    ///
    /// 它的 bearing 是常量：基线在第 12 行，x 偏移 0。
    fn draw_unknown_char(&mut self, baseline: Pixel, color: Color) {
        const UNKNOWN_LAYOUT: GlyphLayout = GlyphLayout {
            width: 8,
            height: 16,
            bearing_x: 0,
            bearing_y: 12,
            advance: 8.0,
        };
        self.draw_glyph(baseline, &UNKNOWN_LAYOUT, &UNKNOWN_MASK, color);
    }
}

/// 行基线的 y 坐标 → 帧缓冲行号。非有限或负值返回 [`None`]。
///
/// 不设上界：帧缓冲高度是 `usize`，而基线本就可能远在画布之下（那由
/// `draw_glyph` 静默裁掉）。`usize::MAX as f32` 的转换是饱和的，
/// 所以 `as` 在这里是精确的。
#[inline]
fn row_y(baseline: f32) -> Option<usize> {
    if !baseline.is_finite() || baseline < 0.0 {
        return None;
    }
    Some(baseline as usize)
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::{Char, Font, FontMetrics, GlyphLayout, PenX, Renderer};
    use crate::{
        Pixel,
        color::Color,
        framebuffer::{FramebufferInfo, PixelFormat},
    };
    use alloc::vec;

    /// 8×16 帧缓冲，够放一个替代字形。
    fn scratch() -> (FramebufferInfo, alloc::vec::Vec<u8>) {
        let info = FramebufferInfo::new(
            8,
            16,
            8 * PixelFormat::Rgb24.bytes_per_pixel(),
            PixelFormat::Rgb24,
        );
        let buffer = vec![0u8; info.buffer_size()];
        (info, buffer)
    }

    /// 占位字体：'A' 画成 4×4 实心块，advance 5。
    struct FakeFont;

    impl Font for FakeFont {
        fn has_glyph(&self, c: char) -> bool {
            c == 'A'
        }

        fn layout(&self, c: char, px: f32) -> Option<GlyphLayout> {
            if c != 'A' || px <= 0.0 || !px.is_finite() {
                return None;
            }
            Some(GlyphLayout {
                width: 4,
                height: 4,
                bearing_x: 0,
                bearing_y: 4,
                advance: 5.0,
            })
        }

        fn metrics(&self, _px: f32) -> FontMetrics {
            FontMetrics {
                ascent: 8.0,
                descent: 2.0,
                line_gap: 0.0,
            }
        }

        fn rasterize(
            &self,
            c: char,
            _px: f32,
            layout: &GlyphLayout,
            mask: &mut [u8],
        ) -> Option<usize> {
            if c != 'A' {
                return None;
            }
            let needed = layout.mask_len().expect("4×4 不溢出");
            if mask.len() < needed {
                panic!("rasterize: mask 容量不足");
            }
            mask[..needed].fill(255);
            Some(needed)
        }
    }

    #[test]
    fn draw_char_uses_font_alpha_mask() {
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        // 基线 y=4、bearing_y=4 → 4×4 实心块落在 x=0..4、y=0..4。
        let mut scratch = vec![0u8; 16];
        renderer.draw_char(
            Pixel::new(0, 4),
            'A',
            16.0,
            Color::new(255, 255, 255),
            Some(&FakeFont),
            &mut scratch,
        );

        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(3, 3), Some(Color::new(255, 255, 255)));
        assert_eq!(
            renderer.get_pixel(4, 0),
            Some(Color::new(0, 0, 0)),
            "advance 之外不应有像素"
        );
        assert_eq!(
            renderer.get_pixel(0, 4),
            Some(Color::new(0, 0, 0)),
            "基线以下不应有像素"
        );
    }

    #[test]
    fn draw_glyph_positions_by_bearing_relative_to_baseline() {
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        let layout = GlyphLayout {
            width: 2,
            height: 2,
            bearing_x: 0,
            bearing_y: 2,
            advance: 3.0,
        };
        renderer.draw_glyph(Pixel::new(1, 5), &layout, &[255; 4], Color::new(0, 255, 0));

        assert_eq!(
            renderer.get_pixel(1, 3),
            Some(Color::new(0, 255, 0)),
            "基线 5 − bearing 2 = 3"
        );
        assert_eq!(renderer.get_pixel(2, 4), Some(Color::new(0, 255, 0)));
        assert_eq!(renderer.get_pixel(1, 5), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn draw_glyph_clips_source_left_edge_for_negative_bearing() {
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        // bearing_x = -2：位图左两列落在原点左侧，必须裁掉源列而不是右移整幅。
        let layout = GlyphLayout {
            width: 4,
            height: 1,
            bearing_x: -2,
            bearing_y: 1,
            advance: 4.0,
        };
        // 源掩码各列 alpha：10, 20, 30, 40。
        renderer.draw_glyph(
            Pixel::new(1, 1),
            &layout,
            &[10, 20, 30, 40],
            Color::new(255, 255, 255),
        );

        // 落点 x = 1-2 = -1 → 只裁掉 1 列，剩余 3 列画在 x=0..3，
        // 分别对应源列 20/30/40。黑底上 alpha=a 混合后约得 a。
        // 若实现漏了裁源，x=0 会拿到源列 0（alpha 10）而非 20——这就是护栏。
        assert_eq!(renderer.get_pixel(0, 0).map(|c| c.r), Some(20));
        assert_eq!(renderer.get_pixel(1, 0).map(|c| c.r), Some(30));
        assert_eq!(renderer.get_pixel(2, 0).map(|c| c.r), Some(40));
        assert_eq!(renderer.get_pixel(3, 0), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn draw_glyph_clips_rows_above_framebuffer_top() {
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        // bearing_y = 5、基线 y=2 → 落点 y = -3，顶部 3 行被裁。
        let layout = GlyphLayout {
            width: 1,
            height: 5,
            bearing_x: 0,
            bearing_y: 5,
            advance: 1.0,
        };
        renderer.draw_glyph(
            Pixel::new(0, 2),
            &layout,
            &[255; 5],
            Color::new(0, 255, 0),
        );

        assert_eq!(
            renderer.get_pixel(0, 0),
            Some(Color::new(0, 255, 0)),
            "裁掉 3 行后应剩 2 行可见"
        );
        assert_eq!(renderer.get_pixel(0, 1), Some(Color::new(0, 255, 0)));
        assert_eq!(
            renderer.get_pixel(0, 2),
            Some(Color::new(0, 0, 0)),
            "不应溢出到基线以下"
        );
    }

    #[test]
    fn draw_glyph_returns_when_glyph_entirely_left_of_framebuffer() {
        let (info, mut framebuffer) = scratch();
        let blank = framebuffer.clone();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        let layout = GlyphLayout {
            width: 2,
            height: 2,
            bearing_x: -10,
            bearing_y: 0,
            advance: 2.0,
        };
        renderer.draw_glyph(Pixel::new(0, 0), &layout, &[255; 4], Color::new(255, 0, 0));
        assert_eq!(renderer.buffer(), &blank[..], "整幅在左侧不应写任何字节");
    }

    #[test]
    fn draw_glyph_returns_when_glyph_entirely_below_framebuffer() {
        let (info, mut framebuffer) = scratch();
        let blank = framebuffer.clone();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        let layout = GlyphLayout {
            width: 2,
            height: 2,
            bearing_x: 0,
            bearing_y: 100,
            advance: 2.0,
        };
        renderer.draw_glyph(Pixel::new(0, 0), &layout, &[255; 4], Color::new(255, 0, 0));
        assert_eq!(renderer.buffer(), &blank[..], "整幅在下方不应写任何字节");
    }

    #[test]
    fn draw_glyph_ignores_mask_shorter_than_layout() {
        let (info, mut framebuffer) = scratch();
        let blank = framebuffer.clone();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        let layout = GlyphLayout {
            width: 4,
            height: 4,
            bearing_x: 0,
            bearing_y: 4,
            advance: 4.0,
        };
        // 声称 4×4 却只给 2 字节掩码：静默退化而非二次 panic。
        renderer.draw_glyph(
            Pixel::new(0, 4),
            &layout,
            &[255; 2],
            Color::new(255, 0, 0),
        );
        assert_eq!(renderer.buffer(), &blank[..]);
    }

    #[test]
    fn draw_char_falls_back_to_replacement_for_missing_glyph() {
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        let mut scratch = vec![0u8; 128];
        renderer.draw_char(
            Pixel::new(0, 12),
            '中',
            16.0,
            Color::new(255, 255, 255),
            Some(&FakeFont),
            &mut scratch,
        );
        // 替代字形是手绘的方框 X，第 0 行全为 1。
        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(7, 0), Some(Color::new(255, 255, 255)));
    }

    #[test]
    fn draw_text_advances_pen_across_glyphs() {
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        // 'A' 是 4×4 实心、advance 5；ascent 8 → 基线 y=8，落点 y=4..8。
        let mut scratch = vec![0u8; 16];
        renderer.draw_text(
            Pixel::new(0, 0),
            "AA",
            16.0,
            Color::new(255, 255, 255),
            &FakeFont,
            &mut scratch,
        );

        assert_eq!(renderer.get_pixel(0, 4), Some(Color::new(255, 255, 255)));
        assert_eq!(renderer.get_pixel(5, 4), Some(Color::new(255, 255, 255)));
        assert_eq!(
            renderer.get_pixel(4, 4),
            Some(Color::new(0, 0, 0)),
            "advance 间隙应为背景"
        );
    }

    #[test]
    fn draw_text_wraps_to_next_line_on_newline() {
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        // 行高 = 8+2+0 = 10。首行基线 y=8（落点 y=4..8），
        // 次行基线 y=18，超出 16px 画布 → 应完全不可见。
        let mut scratch = vec![0u8; 16];
        renderer.draw_text(
            Pixel::new(0, 0),
            "A\nA",
            16.0,
            Color::new(255, 255, 255),
            &FakeFont,
            &mut scratch,
        );
        assert_eq!(renderer.get_pixel(0, 4), Some(Color::new(255, 255, 255)));
        // 次行 x 回到行首，证明画笔确实被重置。
        assert_eq!(renderer.get_pixel(7, 4), Some(Color::new(0, 0, 0)));
    }

    #[test]
    fn draw_text_rejects_non_finite_metrics_without_panicking() {
        struct BadFont;
        impl Font for BadFont {
            fn has_glyph(&self, _c: char) -> bool {
                false
            }
            fn layout(&self, _c: char, _px: f32) -> Option<GlyphLayout> {
                None
            }
            fn metrics(&self, _px: f32) -> FontMetrics {
                FontMetrics {
                    ascent: f32::NAN,
                    descent: 0.0,
                    line_gap: 0.0,
                }
            }
            fn rasterize(
                &self,
                _c: char,
                _px: f32,
                _l: &GlyphLayout,
                _m: &mut [u8],
            ) -> Option<usize> {
                None
            }
        }

        let (info, mut framebuffer) = scratch();
        let blank = framebuffer.clone();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        let mut scratch = vec![0u8; 128];
        renderer.draw_text(
            Pixel::new(0, 0),
            "AAA",
            16.0,
            Color::new(255, 255, 255),
            &BadFont,
            &mut scratch,
        );
        assert_eq!(renderer.buffer(), &blank[..], "NaN 度量必须整段放弃");
    }

    #[test]
    fn pen_x_keeps_subpixel_advance_exact() {
        // 1.0078125 = 129/128，1600 步真值 1612.5。
        // 1/64 px 的网格表示不了 1/128（量化后每步偏 0.0078 px，1600 步
        // 累积 12.5 px——整行错位）；1/65536 px 可以，故这里应**精确**命中。
        let mut pen = PenX::from_px(0.0).expect("0 合法");
        for _ in 0..1600 {
            assert!(pen.advance(1.0078125), "正常步进不应失败");
        }
        let px = pen.to_px();
        assert!(
            (px - 1612.5).abs() < 0.001,
            "1600 次亚像素步进漂移过大：{px} vs 1612.5"
        );
    }

    #[test]
    fn pen_x_is_exact_for_binary_fractions() {
        let mut pen = PenX::from_px(0.0).expect("0 合法");
        for _ in 0..1000 {
            assert!(pen.advance(0.5));
        }
        assert_eq!(pen.to_px(), 500.0, "1/2 px 步进必须精确无漂移");
    }

    #[test]
    fn pen_x_rejects_non_finite_and_out_of_range() {
        assert_eq!(PenX::from_px(f32::NAN), None);
        assert_eq!(PenX::from_px(f32::INFINITY), None);
        assert_eq!(PenX::from_px(f32::MAX), None);
        let mut pen = PenX::from_px(10.0).expect("10 合法");
        assert!(!pen.advance(f32::NAN), "NaN 步进必须失败");
        assert_eq!(pen.to_px(), 10.0, "失败的步进不得改变画笔");
        assert!(!pen.advance(1.0e30));
        assert_eq!(pen.to_px(), 10.0);
    }

    #[test]
    fn pen_x_pixel_x_rejects_negative_positions() {
        let pen = PenX::from_px(-1.0).expect("-1 可表示");
        assert_eq!(pen.pixel_x(), None, "负画笔无法映射到 usize 像素");
        assert!(PenX::from_px(0.0).expect("0 合法").pixel_x() == Some(0));
    }

    #[test]
    fn glyph_layout_mask_len_rejects_overflow() {
        let overflowing = GlyphLayout {
            width: usize::MAX,
            height: 2,
            bearing_x: 0,
            bearing_y: 0,
            advance: 0.0,
        };
        assert_eq!(overflowing.mask_len(), None, "溢出必须返回 None 而非回绕");
        let normal = GlyphLayout {
            width: 3,
            height: 4,
            bearing_x: 0,
            bearing_y: 0,
            advance: 0.0,
        };
        assert_eq!(normal.mask_len(), Some(12));
        assert_eq!(GlyphLayout::empty().mask_len(), Some(0));
    }

    #[test]
    #[should_panic(expected = "mask 长度小于 size 面积")]
    fn char_new_panics_when_mask_too_short() {
        let _ = Char::new(&[0u8; 3], (2, 2));
    }

    #[test]
    fn font_metrics_line_height_sums_parts() {
        let m = FontMetrics {
            ascent: 10.0,
            descent: 3.0,
            line_gap: 2.0,
        };
        assert_eq!(m.line_height(), 15.0);
        assert!(m.is_finite());
        assert!(!FontMetrics {
            ascent: f32::INFINITY,
            descent: 0.0,
            line_gap: 0.0
        }
        .is_finite());
    }

    /// `alloc` 路径与零分配路径必须画出一模一样的像素——
    /// 这是 `draw_*_owned` 存在的全部意义。
    #[cfg(feature = "alloc")]
    #[test]
    fn owned_variants_match_scratch_variants_pixel_for_pixel() {
        use alloc::vec;

        /// 跑一遍 `owned` 与 `scratch` 两条路径，返回各自的帧缓冲字节。
        fn both_paths() -> (alloc::vec::Vec<u8>, alloc::vec::Vec<u8>) {
            let (info_a, mut owned_buf) = scratch();
            let (info_b, mut scratch_buf) = scratch();
            let mut owned = Renderer::new(info_a, &mut owned_buf);
            let mut explicit = Renderer::new(info_b, &mut scratch_buf);
            let mut buf = vec![0u8; 64];

            owned.draw_char_owned(
                Pixel::new(0, 12),
                'A',
                16.0,
                Color::new(255, 255, 255),
                Some(&FakeFont),
            );
            explicit.draw_char(
                Pixel::new(0, 12),
                'A',
                16.0,
                Color::new(255, 255, 255),
                Some(&FakeFont),
                &mut buf,
            );

            owned.draw_text_owned(
                Pixel::new(0, 0),
                "AA",
                16.0,
                Color::new(255, 255, 255),
                &FakeFont,
            );
            explicit.draw_text(
                Pixel::new(0, 0),
                "AA",
                16.0,
                Color::new(255, 255, 255),
                &FakeFont,
                &mut buf,
            );

            (owned_buf, scratch_buf)
        }

        let (owned, explicit) = both_paths();
        assert_eq!(
            owned, explicit,
            "owned 与 scratch 两条路径的结果必须逐字节相同"
        );
    }

    /// 无 `alloc` 时必须提供 scratch——这是「库内不做分配」的默认契约。
    #[cfg(not(feature = "alloc"))]
    #[test]
    fn scratch_is_mandatory_without_alloc() {
        // 编译期事实：`draw_char` 在无 alloc 下没有 `*_owned` 变体。
        // 本测试只断言调用能正常工作（即调用点确实必须自备缓冲）。
        let (info, mut framebuffer) = scratch();
        let mut renderer = Renderer::new(info, &mut framebuffer);
        let mut buf = alloc::vec![0u8; 16];
        renderer.draw_char(
            Pixel::new(0, 4),
            'A',
            16.0,
            Color::new(255, 255, 255),
            Some(&FakeFont),
            &mut buf,
        );
        assert_eq!(renderer.get_pixel(0, 0), Some(Color::new(255, 255, 255)));
    }

    #[cfg(feature = "font_bitmap")]
    mod bitmap {
        use super::super::{Font, UNKNOWN_CHAR};
        use crate::font::font_8x16::DEFAULT_FONT;
        use super::alloc::vec;

        #[test]
        fn default_font_reports_missing_non_ascii_glyph() {
            assert!(!DEFAULT_FONT.has_glyph('中'));
            assert_eq!(DEFAULT_FONT.layout('中', 16.0), None);
        }

        #[test]
        fn default_font_layout_matches_real_baseline() {
            // 基线在第 12 行：92/128 个字形的最低墨迹在第 11 行，
            // `$^~` 顶到第 0 行、`gjpqy` 沉到第 14 行，故 ascent=12/descent=4。
            let layout = DEFAULT_FONT.layout('A', 16.0).expect("'A' 必有字形");
            assert_eq!((layout.width, layout.height), (8, 16));
            assert_eq!(layout.bearing_x, 0);
            assert_eq!(layout.bearing_y, 12);
            assert_eq!(layout.advance, 8.0);
            let m = DEFAULT_FONT.metrics(16.0);
            assert_eq!(m.ascent, 12.0);
            assert_eq!(m.descent, 4.0);
            assert_eq!(m.line_height(), 16.0);
        }

        #[test]
        fn default_font_layout_is_size_independent() {
            // 点阵字体只有一种字号，layout 对任意 px 都应给出同一结果。
            for px in [1.0f32, 8.0, 16.0, 64.0] {
                let layout = DEFAULT_FONT.layout('A', px).expect("'A' 必有字形");
                assert_eq!(layout.width, 8);
                assert_eq!(layout.bearing_y, 12);
            }
        }

        #[test]
        fn default_font_rejects_non_positive_size() {
            assert_eq!(DEFAULT_FONT.layout('A', 0.0), None);
            assert_eq!(DEFAULT_FONT.layout('A', -1.0), None);
            assert_eq!(DEFAULT_FONT.layout('A', f32::NAN), None);
        }

        #[test]
        fn unknown_char_differs_from_question_mark_glyph() {
            let layout = DEFAULT_FONT.layout('?', 16.0).expect("'?' 必有字形");
            let mut scratch = vec![0u8; layout.mask_len().expect("不溢出")];
            let written = DEFAULT_FONT
                .rasterize('?', 16.0, &layout, &mut scratch)
                .expect("'?' 应可光栅化");
            assert_ne!(UNKNOWN_CHAR.mask(), &scratch[..written]);
            assert_eq!(UNKNOWN_CHAR.size(), (8, 16));
        }
    }
}
