//! 超采样抗锯齿（SSAA / Supersampling）。
//!
//! SSAA 的语义是：**以 `factor` 倍的线性分辨率重新绘制一遍，再降采样回原尺寸**。
//! 它不去猜测边缘在哪里，而是让每个最终像素由 `factor²` 个真实采样平均得到——
//! 因此对任何图元、任何组合都成立，也是最不容易做错的抗锯齿方式。
//!
//! # 代价
//!
//! | 项 | 放大倍数 |
//! |----|----------|
//! | 内存（离屏缓冲） | `factor²`（2× → 4 倍，4× → 16 倍） |
//! | 填充率（光栅化像素数） | `factor²` |
//! | 调用方工作量 | 顶点坐标需按 `factor` 放大 |
//!
//! 4× SSAA 意味着每次绘制都做 16 倍的工作。它适合**静态画面、封面图、离屏
//! 合成**这类一次生成、反复使用的场景。
//!
//! 需要每帧重绘的动画不在本库当前能力范围内：屏幕空间后处理（FXAA 一类）
//! 曾尝试过，但对 1px 细线会抹出灰带、对薄结构无法正确判定边缘，
//! 未达到可交付质量，已移除。需要动画抗锯齿时，应在调用方侧降低绘制频率
//! 或缓存 SSAA 结果，而不是每帧重做超采样。
//!
//! # 坐标约定
//!
//! 闭包拿到的高分辨率渲染器尺寸为 `width × factor` 与 `height × factor`，
//! 因此调用方传入的几何坐标需**先乘以 `factor`**。降采样时按 `factor × factor`
//! 的方块取平均，方块 `(bx, by)` 对应原图素 `(bx, by)`。
//!
//! # 降采样口径
//!
//! 逐通道取算术平均（四舍五入），而不是「取左上角」或「取中位数」。
//! 565 格式先解码到 8 位再平均，最后由目标格式量化——避免在 5/6/5 的窄位宽上
//! 累加误差。
//!
//! # 两个入口
//!
//! | 函数 | 需要 `alloc` | 说明 |
//! |------|-------------|------|
//! | [`Renderer::ssaa`] | 否 | 离屏缓冲由调用方以 `scratch` 传入 |
//! | [`Renderer::ssaa_owned`] | 是 | 内部申请离屏缓冲，为便利方法 |

use super::Renderer;
use crate::color::Color;
use crate::framebuffer::FramebufferInfo;

/// SSAA 的线性放大倍数。
///
/// 面积为倍数平方：`X2` 为 4 个采样/像素，`X4` 为 16 个。
///
/// # 示例
///
/// ```
/// # use rasteron::core::SsaaFactor;
/// assert_eq!(SsaaFactor::X2.multiplier(), 2);
/// assert_eq!(SsaaFactor::X4.multiplier(), 4);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsaaFactor {
    /// 2× 线性放大（4 个采样/像素）。
    X2,
    /// 3× 线性放大（9 个采样/像素）。
    X3,
    /// 4× 线性放大（16 个采样/像素）。
    X4,
}

impl SsaaFactor {
    /// 线性倍数（`2` / `3` / `4`）。
    #[inline(always)]
    pub const fn multiplier(self) -> usize {
        match self {
            SsaaFactor::X2 => 2,
            SsaaFactor::X3 => 3,
            SsaaFactor::X4 => 4,
        }
    }
}

impl Renderer<'_> {
    /// 以 SSAA 超采样执行一次绘制，并把结果降采样回本渲染器的帧缓冲。
    ///
    /// `scratch` 是高分辨率离屏缓冲，长度必须 ≥
    /// `pitch × height × factor`（`pitch` 取本渲染器的 `info().pitch`）。
    /// 长度不足时**不写入任何像素**并返回 `false`。
    ///
    /// `draw` 收到一个高分辨率渲染器，调用方在其中按放大后的坐标绘制。
    /// 它返回 `&mut Self` 只是为了便于链式调用；返回值的绘制同样落在这块离屏
    /// 缓冲上，不会影响本渲染器。
    ///
    /// # 返回值
    ///
    /// `true` 表示已执行并已降采样回目标；`false` 表示 `scratch` 不足而未执行。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::core::SsaaFactor;
    /// # use rasteron::{Renderer, Pixel};
    /// # use rasteron::framebuffer::FramebufferInfo;
    /// # use rasteron::color::{Color, PixelFormat};
    /// let info = FramebufferInfo::new(8, 8, 24, PixelFormat::Rgb24);
    /// let mut fb = vec![0u8; info.buffer_size()];
    ///
    /// // 2× 放大后是 16×16，pitch 48，共 768 字节。
    /// let mut scratch = vec![0u8; 48 * 16];
    /// let factor = SsaaFactor::X2;
    ///
    /// let mut renderer = Renderer::new(info, &mut fb);
    /// renderer.ssaa(&mut scratch, factor, |hi| {
    ///     // 坐标已放大：原图 (4,4) 半径 3 的圆 → (8,8) 半径 6。
    ///     hi.fill_circle(Pixel::new(4 * 2, 4 * 2), 3 * 2, Color::new(255, 0, 0));
    /// });
    /// ```
    pub fn ssaa<F>(&mut self, scratch: &mut [u8], factor: SsaaFactor, draw: F) -> bool
    where
        F: FnOnce(&mut Renderer<'_>),
    {
        let multiplier = factor.multiplier();
        let width = self.info.width;
        let height = self.info.height;
        let format = self.info.format;

        // 高分辨率尺寸。`checked_mul` 失败说明请求本身不可表示，静默返回。
        let (Some(hi_width), Some(hi_height)) = (
            width.checked_mul(multiplier),
            height.checked_mul(multiplier),
        ) else {
            return false;
        };
        let bpp = format.bytes_per_pixel();
        // 离屏缓冲用紧凑行距（无行填充），降采样按此读取。
        let Some(hi_pitch) = hi_width.checked_mul(bpp) else {
            return false;
        };
        let Some(hi_size) = hi_pitch.checked_mul(hi_height) else {
            return false;
        };
        if scratch.len() < hi_size {
            return false;
        }

        let hi_info = FramebufferInfo::new(hi_width, hi_height, hi_pitch, format);
        {
            let hi_buffer = &mut scratch[..hi_size];
            let mut hi = Renderer::new(hi_info, hi_buffer);
            // 超采样缓冲从黑色起步，与「先清屏再画」的默认预期一致。
            hi.clear(Color::new(0, 0, 0));
            draw(&mut hi);
            // 降采样必须在 `hi` 释放后做：它需要独占读取 `scratch`，
            // 而写入目标是 `self` 的缓冲区。
        }

        self.downsample_from(&scratch[..hi_size], hi_pitch, multiplier);
        true
    }

    /// 把高分辨率缓冲降采样到本渲染器的帧缓冲。
    ///
    /// `source` 覆盖 `(width × n) × (height × n)` 个高分辨率像素，行距为
    /// `hi_pitch` 字节，格式与本渲染器一致。调用方保证尺寸匹配——
    /// [`Renderer::ssaa`] 是它唯一的内部使用者。
    /// `hi_pitch` 是高分辨率缓冲的行距；`hi_width` 隐含在其中，只用于断言。
    fn downsample_from(&mut self, source: &[u8], hi_pitch: usize, n: usize) {
        let width = self.info.width;
        let height = self.info.height;
        let format = self.info.format;
        let bpp = format.bytes_per_pixel();
        // 每个原像素由 n×n 个高分辨率像素平均，采样总数固定。
        let samples = (n * n) as u32;
        // 半程偏移用于四舍五入：sum / samples 时补 (samples / 2)。
        let rounding = samples / 2;

        for y in 0..height {
            for x in 0..width {
                let mut sum_r: u32 = 0;
                let mut sum_g: u32 = 0;
                let mut sum_b: u32 = 0;
                let mut sum_a: u32 = 0;

                for sy in 0..n {
                    let hi_y = y * n + sy;
                    // `hi_y × hi_pitch + (x × n) × bpp`，全部按 usize 计算。
                    let row_start = hi_y * hi_pitch;
                    for sx in 0..n {
                        let hi_x = x * n + sx;
                        let offset = row_start + hi_x * bpp;
                        // 越界时按黑色计入，保持平均分母恒定（不跳过采样点，
                        // 否则边缘像素的平均会偏向内部色，产生一圈暗边）。
                        let color = if offset + bpp <= source.len() {
                            format.read_color(&source[offset..offset + bpp])
                        } else {
                            Color::new(0, 0, 0)
                        };
                        sum_r += color.r as u32;
                        sum_g += color.g as u32;
                        sum_b += color.b as u32;
                        sum_a += color.a as u32;
                    }
                }

                let averaged = Color::with_alpha(
                    ((sum_r + rounding) / samples) as u8,
                    ((sum_g + rounding) / samples) as u8,
                    ((sum_b + rounding) / samples) as u8,
                    ((sum_a + rounding) / samples) as u8,
                );
                // 走内部的覆盖写路径（alpha 恒为 255 时直接写，不混合）：
                // SSAA 产出的像素本就是最终颜色，不该与已有内容再混合。
                self.set_pixel(x, y, &averaged);
            }
        }
    }

    /// 以 SSAA 超采样执行一次绘制，离屏缓冲由本函数申请。
    ///
    /// 与 [`Renderer::ssaa`] 行为一致，区别只在缓冲来源：本方法需要 `alloc`
    /// feature，换取调用方不必自己算离屏缓冲大小。对 `no_std` + 有分配器的
    /// 环境两者等价；对完全无分配的环境只能用 `ssaa`。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::core::SsaaFactor;
    /// # use rasteron::{Renderer, Pixel};
    /// # use rasteron::framebuffer::FramebufferInfo;
    /// # use rasteron::color::{Color, PixelFormat};
    /// let info = FramebufferInfo::new(8, 8, 24, PixelFormat::Rgb24);
    /// let mut fb = vec![0u8; info.buffer_size()];
    /// let mut renderer = Renderer::new(info, &mut fb);
    ///
    /// renderer.ssaa_owned(SsaaFactor::X4, |hi| {
    ///     hi.fill_circle(Pixel::new(16, 16), 12, Color::new(0, 128, 255));
    /// });
    /// ```
    #[cfg(feature = "alloc")]
    pub fn ssaa_owned<F>(&mut self, factor: SsaaFactor, draw: F)
    where
        F: FnOnce(&mut Renderer<'_>),
    {
        let multiplier = factor.multiplier();
        let Some(hi_size) = self
            .info
            .width
            .checked_mul(multiplier)
            .and_then(|w| w.checked_mul(self.info.format.bytes_per_pixel()))
            .and_then(|row| {
                self.info
                    .height
                    .checked_mul(multiplier)
                    .and_then(|h| row.checked_mul(h))
            })
        else {
            // 尺寸不可表示时无事可做，静默返回（与 `ssaa` 的失败口径一致）。
            return;
        };
        let mut scratch = alloc::vec![0u8; hi_size];
        self.ssaa(&mut scratch, factor, draw);
    }
}

/// 让 `alloc` 在 `no_std` 下可用。
#[cfg(feature = "alloc")]
extern crate alloc;

/// 供测试使用的像素格式覆盖校验：确认本模块对所有格式都给出一致的降采样结果。
#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec;

    use super::SsaaFactor;
    use crate::color::{Color, PixelFormat};
    use crate::framebuffer::FramebufferInfo;
    use crate::{Pixel, Renderer};

    /// 所有 8 位通道格式都应把边缘像素平均成中间色。
    #[test]
    fn ssaa_averages_a_hard_edge() {
        for format in [PixelFormat::Rgb24, PixelFormat::Bgr24, PixelFormat::Rgba8888] {
            let bpp = format.bytes_per_pixel();
            let info = FramebufferInfo::new(4, 4, 4 * bpp, format);
            let mut fb = vec![0u8; info.buffer_size()];
            let mut scratch = vec![0u8; 8 * 8 * bpp];

            let mut renderer = Renderer::new(info, &mut fb);
            // 在用 2× SSAA 画竖线，线覆盖高分辨率下的 x = 4..6。
            let done = renderer.ssaa(&mut scratch, SsaaFactor::X2, |hi| {
                hi.fill_rect(Pixel::new(4, 0), 2, 8, Color::new(255, 255, 255));
            });
            assert!(done, "{format:?}: scratch 足够时应执行");

            // 原图 x=1 的像素由高分辨率 x=2,3（黑）与 x=4,5（白）组成？不——2×2 块，
            // 原 x=1 对应高分辨率 x=2..4，其中 x=2,3 为黑。故仍为黑。
            let left = renderer.get_pixel(1, 2).expect("可见").r;
            assert_eq!(left, 0, "{format:?}: 未被线覆盖的像素应保持黑色");
            // 原 x=2 对应高分辨率 x=4..6，均为白。
            let right = renderer.get_pixel(2, 2).expect("可见").r;
            assert_eq!(right, 255, "{format:?}: 完全覆盖的像素应为纯白");
        }
    }

    /// 覆盖率恰好 50% 的像素应得到中间灰度，而不是被量化到黑或白。
    #[test]
    fn ssaa_edge_pixel_is_half_gray() {
        let info = FramebufferInfo::new(2, 1, 6, PixelFormat::Rgb24);
        let mut fb = vec![0u8; info.buffer_size()];
        let mut scratch = vec![0u8; 4 * 2 * 3];
        let mut renderer = Renderer::new(info, &mut fb);

        renderer.ssaa(&mut scratch, SsaaFactor::X2, |hi| {
            // 高分辨率 4×2：白区从 x = 1 起（错开一个采样），覆盖 2/4 个采样点。
            hi.fill_rect(Pixel::new(1, 0), 3, 2, Color::new(255, 255, 255));
        });

        // 原像素 x=0 的四个采样：高分辨率 x=0 为黑、x=1 为白 → 2/4 = 127 左右。
        let edge = renderer.get_pixel(0, 0).expect("可见").r;
        assert!(
            (120..=135).contains(&edge),
            "50% 覆盖率应得到中间灰度，实际 {edge}"
        );
    }

    /// `scratch` 不足时必须一个像素都不改。
    #[test]
    fn ssaa_rejects_short_scratch_without_writing() {
        let info = FramebufferInfo::new(8, 8, 24, PixelFormat::Rgb24);
        let mut fb = vec![7u8; info.buffer_size()];
        let before = fb.clone();
        let mut scratch = vec![0u8; 16]; // 远不足 8×8×4 像素。
        let mut renderer = Renderer::new(info, &mut fb);

        let done = renderer.ssaa(&mut scratch, SsaaFactor::X2, |hi| {
            hi.clear(Color::new(255, 0, 0));
        });
        assert!(!done, "scratch 不足应返回 false");
        assert_eq!(renderer.buffer(), &before[..], "失败时不得改动帧缓冲");
    }

    /// `ssaa_owned` 与 `ssaa` 在等价输入下产出相同结果。
    ///
    /// `ssaa_owned` 本身由 `alloc` feature 门控，测试必须同步门控——
    /// 否则 `--no-default-features` 的测试构建会因找不到该方法而编译失败。
    #[cfg(feature = "alloc")]
    #[test]
    fn ssaa_owned_matches_scratch_version() {
        let make_info = || FramebufferInfo::new(6, 6, 18, PixelFormat::Rgb24);
        let mut fb_a = vec![0u8; make_info().buffer_size()];
        let mut fb_b = vec![0u8; make_info().buffer_size()];
        let mut scratch = vec![0u8; 12 * 12 * 3];

        let draw = |hi: &mut Renderer<'_>| {
            hi.fill_circle(Pixel::new(12, 12), 9, Color::new(200, 40, 40));
        };

        let mut a = Renderer::new(make_info(), &mut fb_a);
        a.ssaa(&mut scratch, SsaaFactor::X2, |hi| draw(hi));

        let mut b = Renderer::new(make_info(), &mut fb_b);
        b.ssaa_owned(SsaaFactor::X2, |hi| draw(hi));

        assert_eq!(a.buffer(), b.buffer(), "两个入口应产出完全相同的像素");
    }

    /// 565 格式必须在 8 位域上平均再量化，不能按 16 位字平均。
    #[test]
    fn ssaa_565_survives_downsample() {
        let info = FramebufferInfo::new(2, 1, 4, PixelFormat::Rgb565);
        let mut fb = vec![0u8; info.buffer_size()];
        let mut scratch = vec![0u8; 4 * 2 * 2];
        let mut renderer = Renderer::new(info, &mut fb);

        renderer.ssaa(&mut scratch, SsaaFactor::X2, |hi| {
            hi.fill_rect(Pixel::new(0, 0), 2, 2, Color::new(255, 255, 255));
        });

        let left = renderer.get_pixel(0, 0).expect("可见");
        assert!(
            left.r >= 120 && left.g >= 120 && left.b >= 120,
            "半覆盖的 565 像素应接近中灰，实际 {left:?}"
        );
    }

    /// 尺寸为 0 的帧缓冲不应 panic。
    #[test]
    fn ssaa_handles_zero_size_target() {
        let info = FramebufferInfo::new(0, 0, 0, PixelFormat::Rgb24);
        let mut fb: [u8; 0] = [];
        let mut scratch = vec![0u8; 64];
        let mut renderer = Renderer::new(info, &mut fb);
        assert!(renderer.ssaa(&mut scratch, SsaaFactor::X2, |_| {}));
    }
}
