//! TrueType / OpenType 字体支持（`font_truetype` feature）。
//!
//! # 概述
//!
//! [`TtfFont`] 是 [`crate::font::Font`] 的比例字体实现，底层用 `fontdue`
//! 解析字体表并做抗锯齿光栅化。与位图字体最大的差别是**同一个字符在不同字号
//! 下有不同的度量**，所以 [`Font::layout`] 与 [`Font::metrics`] 都要吃 `px`。
//!
//! # 内存
//!
//! `fontdue` 自身是 `no_std` + `alloc` 的依赖，它在 [`TtfFont::new`] 时就
//! 把字体表解析进自己的堆结构（本类型因此**必须自持字体字节**，不能借用）。
//! 但**每个字形的光栅化结果不进缓存**：[`Font::rasterize`] 直接把覆盖度
//! 写进调用方借出的 `&mut [u8]`，不产生额外分配。缓存策略交给调用方
//! （见 [`crate::font::Char`]）。
//!
//! # 度量映射
//!
//! fontdue 的 [`Metrics`] 是「位图左上角相对基线」的表示，转换关系：
//!
//! | 本库字段 | fontdue 来源 |
//! |----------|-------------|
//! | `width` / `height` | `metrics.width` / `metrics.height` |
//! | `bearing_x` | `metrics.xmin`（**原样保留负值**，见 [`TtfFont::layout`]） |
//! | `bearing_y` | `metrics.ymin + metrics.height`（位图顶沿相对基线） |
//! | `advance` | `metrics.advance_width` |
//!
//! # 浮点
//!
//! 字号与 fontdue 的度量都是 `f32`，光栅化内部亦以浮点累加。见
//! [`crate::font`] 模块文档关于「内核态目标应关闭 `font_truetype`」的说明。

extern crate alloc;

use alloc::vec::Vec;
use fontdue::Metrics;

use crate::font::{Font, FontMetrics, GlyphLayout};

/// 字节序假设与浮点都交给 fontdue 自身处理，本类型只做适配。
///
/// `FontSettings::default()` 把几何优化字号设为 40px——这是 fontdue 文档
/// 建议的值，对本库的常见字号（8–48）都在合理范围内，故沿用默认。
pub struct TtfFont {
    font: fontdue::Font,
}

impl TtfFont {
    /// 解析字体字节。
    ///
    /// 字体数据会被 fontdue **复制**进它自己的表结构，因此 `font` 参数可以
    /// 是临时切片；反过来说，本类型持有字体内容，不借用调用方的字节数组。
    ///
    /// # 恐慌
    ///
    /// 字体数据无法解析时 panic。本库对「构造期不变量被破坏」统一用
    /// [`panic`]（见 `src/error.rs` 模块文档的错误分层），而字体是否合法
    /// 是构造期就能确定的事实，不该用 [`Result`] 让每个调用点重复处理。
    #[must_use]
    pub fn new(font: &[u8]) -> Self {
        match fontdue::Font::from_bytes(font, fontdue::FontSettings::default()) {
            Ok(parsed) => TtfFont { font: parsed },
            Err(_) => panic!("TtfFont::new: 字体数据无法解析"),
        }
    }

    /// 该字体的度量，由 fontdue 的 `Metrics` 转换而来。
    ///
    /// `bearing_x` **原样保留** `xmin` 的符号：fontdue 允许左悬挂（负 `xmin`），
    /// 丢弃符号会让斜体与 `j` 之类的字形整体右移。`draw_glyph` 负责裁掉落在
    /// 原点左侧的位图部分。
    fn to_layout(&self, c: char, px: f32) -> Option<GlyphLayout> {
        if px <= 0.0 || !px.is_finite() {
            return None;
        }
        // 缺字必须在此拦下：fontdue 的 `metrics` 对缺字回退到 `.notdef`
        // 字形并给出正常外观的度量，若直接透传，缺字会画成豆腐块之外的
        // 怪东西而不是约定的替代字形。
        if !self.font.has_glyph(c) {
            return None;
        }
        let m = self.font.metrics(c, px);
        Some(GlyphLayout {
            width: m.width,
            height: m.height,
            bearing_x: m.xmin,
            // fontdue 的 `ymin` 是位图**底沿**相对基线的偏移：下伸部为负，
            // 普通字身为 0，重音等可越过基线的部分为正。顶沿相对基线的距离
            // 因而是 `ymin + height`。这里不能写成 `height - ymin`：那会让
            // 下伸部越深，位图反而越向上移动，并在基线上方画出错误字形。
            bearing_y: m.ymin + m.height as i32,
            advance: m.advance_width,
        })
    }
}

impl Font for TtfFont {
    fn has_glyph(&self, c: char) -> bool {
        // 空格没有可见字形，但排版上必须存在（否则整行塌缩），
        // 故把空白字符一律视为「有」，交给 layout 返回零尺寸。
        c.is_whitespace() || self.font.has_glyph(c)
    }

    fn layout(&self, c: char, px: f32) -> Option<GlyphLayout> {
        // 空白字符：零尺寸位图 + 一个 advance，让排版照常推进。
        if c.is_whitespace() {
            if px <= 0.0 || !px.is_finite() {
                return None;
            }
            let m: Metrics = self.font.metrics(c, px);
            return Some(GlyphLayout {
                width: 0,
                height: 0,
                bearing_x: 0,
                bearing_y: 0,
                advance: m.advance_width,
            });
        }
        self.to_layout(c, px)
    }

    fn metrics(&self, px: f32) -> FontMetrics {
        // fontdue 的 `horizontal_line_metrics` 在字体没有 hhea/OS2 表时为
        // `None`；此时回退到「按 1em 估算」，保证排版有可用的行高。
        match self.font.horizontal_line_metrics(px) {
            Some(line) => FontMetrics {
                ascent: line.ascent,
                // fontdue 的 descent 是负值（低于基线），本库约定非负。
                descent: (-line.descent).max(0.0),
                line_gap: line.line_gap.max(0.0),
            },
            None => {
                let cap = if px.is_finite() && px > 0.0 { px } else { 0.0 };
                FontMetrics {
                    ascent: cap * 0.8,
                    descent: cap * 0.2,
                    line_gap: 0.0,
                }
            }
        }
    }

    fn rasterize(
        &self,
        c: char,
        px: f32,
        layout: &GlyphLayout,
        mask: &mut [u8],
    ) -> Option<usize> {
        // 空白字符没有位图，但仍返回 0 让调用方正常推进画笔。
        if c.is_whitespace() {
            return Some(0);
        }
        let needed = layout.mask_len()?;
        // 容量策略见 trait 契约：按 layout 算出的容量不足即 panic。
        // 这里用 `assert!` 而非静默返回——静默会让文字「凭空消失」，
        // 比崩溃难查得多。
        assert!(
            mask.len() >= needed,
            "TtfFont::rasterize: mask 容量 {have} 小于所需 {needed}",
            have = mask.len()
        );
        let (metrics, bitmap) = self.rasterize_glyph(c, px)?;
        // 尺寸对不上说明 layout 与本次光栅化的字号不一致——这是调用方
        // 违反了 trait 契约（layout 必须来自同一次查询），不能默默写错。
        assert!(
            metrics.width == layout.width && metrics.height == layout.height,
            "TtfFont::rasterize: layout 与光栅化结果尺寸不一致"
        );
        mask[..needed].copy_from_slice(&bitmap[..needed]);
        Some(needed)
    }
}

impl TtfFont {
    /// 调用 fontdue 的光栅化，返回其度量与覆盖度位图。
    ///
    /// fontdue 内部会为位图分配一个 [`Vec`]——这是**每个字形一次**的临时分配，
    /// 无法在借用式 API 下消除。返回它让调用方立刻拷进自己的缓冲。
    fn rasterize_glyph(&self, c: char, px: f32) -> Option<(Metrics, Vec<u8>)> {
        if px <= 0.0 || !px.is_finite() {
            return None;
        }
        let (metrics, bitmap) = self.font.rasterize(c, px);
        // fontdue 对 `px <= 0` 返回空位图；此时 `metrics.width == 0`，
        // 与 layout 的口径一致，直接交给调用方的零尺寸分支。
        Some((metrics, bitmap))
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::TtfFont;
    use crate::font::Font;
    use alloc::vec;

    /// 测试字体：`tests/fixtures/minimal_ascii.ttf`，由 **DejaVu Sans** 经
    /// fontTools 子集化而来（7 字形 / 10768 B，2048 unitsPerEm）。
    ///
    /// 选它是因为 6 个字形恰好覆盖了三类排版难点：
    ///
    /// | 字形 | advance /2048 em | 覆盖的难点 |
    /// |------|------------------|-----------|
    /// | `W` | 2025 | 最宽字形，位图宽 |
    /// | `A` | 1401 | 常规大写 |
    /// | `g` | 1300 | 下伸部（低于基线） |
    /// | `\|` | 690 | 窄字形 |
    /// | `i` / `j` | 569 | 最窄 + `j` **有悬挂部**（`bearing_x` 为负） |
    ///
    /// advance 跨度 3.6 倍，等宽 fixture 测不出步进逻辑的错误；`j` 的悬挂部
    /// 则是 `Renderer::draw_glyph` 左边界裁剪的真实触发条件。
    ///
    /// 生成命令与去 hinting 理由见 `src/AGENTS.md` 的 font 契约段。
    const MINIMAL_FONT: &[u8] = include_bytes!("../../tests/fixtures/minimal_ascii.ttf");

    /// fixture 覆盖的全部字形。
    const GLYPHS: &str = "AgjiW|";

    fn font() -> TtfFont {
        TtfFont::new(MINIMAL_FONT)
    }

    #[test]
    #[should_panic(expected = "字体数据无法解析")]
    fn new_panics_on_invalid_font_data() {
        let _ = TtfFont::new(b"not a font at all");
    }

    #[test]
    #[should_panic(expected = "字体数据无法解析")]
    fn new_panics_on_empty_slice() {
        let _ = TtfFont::new(&[]);
    }

    #[test]
    fn has_glyph_distinguishes_present_from_absent() {
        let font = font();
        assert!(font.has_glyph('A'));
        assert!(font.has_glyph('g'));
        assert!(font.has_glyph(' '), "空白也算有，否则整行塌缩");
        // 中不在子集里。
        assert!(!font.has_glyph('中'));
        assert_eq!(font.layout('中', 24.0), None, "缺字必须返回 None");
    }

    #[test]
    fn layout_scales_with_font_size() {
        let font = font();
        let small = font.layout('A', 16.0).expect("'A' 有字形");
        let large = font.layout('A', 64.0).expect("'A' 有字形");
        assert!(
            large.width >= small.width && large.height >= small.height,
            "字号变大后位图不应缩小：{small:?} vs {large:?}"
        );
        assert!(
            large.advance > small.advance * 3.0,
            "advance 应随字号线性放大：{} vs {}",
            small.advance,
            large.advance
        );
    }

    #[test]
    fn layout_rejects_non_positive_and_non_finite_size() {
        let font = font();
        assert_eq!(font.layout('A', 0.0), None);
        assert_eq!(font.layout('A', -1.0), None);
        assert_eq!(font.layout('A', f32::NAN), None);
        assert_eq!(font.layout('A', f32::INFINITY), None);
    }

    #[test]
    fn layout_bearing_is_consistent_with_metrics() {
        let font = font();
        // 负 bearing_x（悬挂部）在本字体中未必出现，但垂直方向必须满足：
        // `bearing_y = ymin + height`，即位图顶沿相对基线的距离。
        for c in GLYPHS.chars() {
            let Some(layout) = font.layout(c, 32.0) else {
                continue;
            };
            assert_eq!(layout.mask_len(), Some(layout.width * layout.height));
            // 字形顶沿必在基线之上，故 bearing_y 非负。
            assert!(
                layout.bearing_y >= 0,
                "{c:?} 的 bearing_y 应非负，实得 {}",
                layout.bearing_y
            );
        }
    }

    #[test]
    fn rasterize_returns_consistent_length_and_non_empty_mask() {
        let font = font();
        for px in [12.0f32, 24.0, 48.0] {
            for c in GLYPHS.chars() {
                let layout = font.layout(c, px).expect("字形存在");
                let needed = layout.mask_len().expect("正常字号不溢出");
                let mut scratch = vec![0u8; needed];
                let written = font
                    .rasterize(c, px, &layout, &mut scratch)
                    .expect("可光栅化");
                assert_eq!(written, needed, "写入长度必须等于 layout 推导的长度");
                assert!(
                    scratch.iter().any(|&b| b > 0),
                    "{c:?}@{px} 的掩码不应全透明"
                );
                // 抗锯齿的实质：字形内部应存在**部分覆盖**的像素。
                // 若全非 0 像素都是 255，说明退化成了无 AA 的硬边渲染。
                assert!(
                    scratch.iter().any(|&b| b > 0 && b < 255),
                    "{c:?}@{px} 应有抗锯齿的中间灰度"
                );
            }
        }
    }

    /// 容量策略与 trait 契约一致：缓冲不足即 panic，而不是静默返回。
    #[test]
    #[should_panic(expected = "mask 容量")]
    fn rasterize_panics_when_mask_smaller_than_layout() {
        let font = font();
        let layout = font.layout('A', 48.0).expect("'A' 有字形");
        let needed = layout.mask_len().expect("不溢出");
        assert!(needed > 1, "48px 的 'A' 位图应大于 1 字节");
        let mut tiny = vec![0u8; needed - 1];
        let _ = font.rasterize('A', 48.0, &layout, &mut tiny);
    }

    /// 拿 16px 的 layout 去要 64px 的光栅化结果——违反 trait 契约，
    /// 实现必须发现尺寸不符，而不是默默写坏掩码。
    #[test]
    #[should_panic(expected = "尺寸不一致")]
    fn rasterize_panics_when_layout_mismatches_font_size() {
        let font = font();
        let small = font.layout('A', 16.0).expect("'A' 有字形");
        let mut scratch = vec![0u8; small.mask_len().unwrap_or(1) * 16];
        let _ = font.rasterize('A', 64.0, &small, &mut scratch);
    }

    #[test]
    fn whitespace_has_advance_but_no_bitmap() {
        let font = font();
        let space = font.layout(' ', 24.0).expect("空格有度量");
        assert_eq!(space.width, 0);
        assert_eq!(space.height, 0);
        assert_eq!(space.mask_len(), Some(0));
        assert!(space.advance > 0.0, "空格必须能推进画笔");
        assert!(
            font.rasterize(' ', 24.0, &space, &mut []).is_some(),
            "空格光栅化应成功返回 0 字节"
        );
    }

    /// `j` 在 DejaVu 里带悬挂部，`bearing_x` 为负。这是
    /// `Renderer::draw_glyph` 左边界裁剪逻辑的真实触发条件，必须钉住
    /// ——若实现把负 bearing 当 0，所有带悬挂部的字会整体右移。
    #[test]
    fn j_has_negative_bearing_x_from_the_font_metrics() {
        let font = font();
        let j = font.layout('j', 64.0).expect("'j' 在子集里");
        let i = font.layout('i', 64.0).expect("'i' 在子集里");
        assert!(
            j.bearing_x < 0,
            "'j' 应有负 bearing_x（悬挂部），实得 {}",
            j.bearing_x
        );
        // 'i' 无悬挂部，位图左沿就在原点上。
        assert!(
            i.bearing_x >= 0,
            "'i' 不应有悬挂部，实得 {}",
            i.bearing_x
        );
        // 两者 advance 相同但 bearing 不同——说明 bearing 不是从 advance 推的。
        assert_eq!(i.advance, j.advance);
    }

    /// 下伸部的底沿必须落在基线下方，不能因换算错误而被抬到字身内部。
    #[test]
    fn descender_extends_below_baseline() {
        let font = font();
        let cap = font.layout('A', 64.0).expect("'A'");
        let descender = font.layout('g', 64.0).expect("'g'");
        assert!(
            descender.height > cap.height,
            "下伸字形应比大写更高：{} vs {}",
            descender.height,
            cap.height
        );
        let cap_bottom = cap.bearing_y - cap.height as i32;
        let descender_bottom = descender.bearing_y - descender.height as i32;
        assert_eq!(cap_bottom, 0, "'A' 的底沿应位于基线");
        assert!(
            descender_bottom < 0,
            "'g' 的下伸部应低于基线，实得底沿 {descender_bottom}"
        );
    }

    #[test]
    fn metrics_are_finite_positive_and_scale_with_size() {
        let font = font();
        let small = font.metrics(16.0);
        let large = font.metrics(64.0);
        assert!(small.is_finite(), "度量必须有限：{small:?}");
        assert!(large.is_finite());
        assert!(small.ascent > 0.0, "ascent 应为正");
        assert!(small.descent >= 0.0, "descent 约定非负：{}", small.descent);
        assert!(small.line_gap >= 0.0, "line_gap 约定非负");
        assert!(
            large.line_height() > small.line_height(),
            "行高应随字号放大：{} vs {}",
            small.line_height(),
            large.line_height()
        );
    }

    #[test]
    fn proportional_advances_differ_per_glyph() {
        let font = font();
        // 若后端把所有字都当等宽，排版就废了——这正是 fixture 选这组
        // 字符的原因。
        let a = font.layout('A', 32.0).expect("'A'").advance;
        let g = font.layout('g', 32.0).expect("'g'").advance;
        let bar = font.layout('|', 32.0).expect("'|'" ).advance;
        assert!(a > g, "'A' 应比 'g' 宽：{a} vs {g}");
        assert!(g > bar, "'g' 应比 '|' 宽：{g} vs {bar}");
    }

    #[test]
    fn metrics_and_layout_agree_on_vertical_extent() {
        let font = font();
        let m = font.metrics(32.0);
        // 大写字母不应高过 ascent。
        let cap = font.layout('A', 32.0).expect("'A'").bearing_y;
        assert!(
            cap <= m.ascent as i32 + 1,
            "'A' 的 bearing_y {} 不应超过 ascent {}",
            cap,
            m.ascent
        );
    }
}
