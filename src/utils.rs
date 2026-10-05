use crate::Pixel;
use crate::core::Renderer;
use crate::framebuffer::FramebufferInfo;
#[cfg(feature = "programmable")]
use crate::programmable::{Vec2, Viewport};
#[cfg(feature = "simd")]
use crate::simd;

/// 按可见窗口构造单缓冲子渲染器，实现无状态裁剪。
///
/// 绘制直接写入 `fb`。需要子视口双缓冲（绘制落在独立后台缓冲区、再由
/// `swap_buffers` 呈现回 `fb`）时用 [`sub_view_with_back_buffer`]。
///
/// # 收缩规则
///
/// 越界部分静默收缩，不 panic：
///
/// * `width`/`height` 超出可视区时，按 `info.width - x` / `info.height - y` 收缩。
/// * 行数另受可用字节限制——窗口起点 `off` 之后必须还放得下每行 `pitch` 字节
///   （`new` 的断言按 `pitch × height` 口径要求），故行数收缩到
///   `(fb.len() - off) / pitch`。窗口贴住缓冲区末尾时**可能少最后一行**，
///   这是 `pitch` 行填充被计入的必然结果，不是缺陷。
///
/// # 示例
///
/// ```
/// # use rasteron::{Renderer, Pixel};
/// # use rasteron::framebuffer::FramebufferInfo;
/// # use rasteron::color::{Color, PixelFormat};
/// # use rasteron::utils::sub_view;
/// let pitch = 64 * 4;
/// let info = FramebufferInfo::new(64, 64, pitch, PixelFormat::Rgba8888);
/// let mut fb = vec![0u8; info.buffer_size()];
///
/// // 在 (8, 8) 处开一个 16x16 的窗口
/// let mut panel = sub_view(&mut fb, &info, 8, 8, 16, 16).unwrap();
/// assert_eq!((panel.info().width, panel.info().height), (16, 16));
/// // pitch 不变，故窗口内像素仍落在正确的物理位置
/// assert_eq!(panel.info().pitch, pitch);
///
/// // 画一个远超窗口的矩形，溢出部分被自动裁掉
/// panel.fill_rect(Pixel::new(0, 0), 1000, 1000, Color::new(255, 0, 0));
/// ```
///
/// # 返回值
///
/// 返回 `None` 表示窗口完全不可用：尺寸为 0、起点落在可视区之外、或起点之后
/// 已放不下任何完整行。
pub fn sub_view<'a>(
    fb: &'a mut [u8],
    info: &FramebufferInfo,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Option<Renderer<'a>> {
    let geo = sub_view_geometry(fb.len(), info, x, y, width, height)?;
    let sub_info = FramebufferInfo::new(geo.width, geo.height, info.pitch, info.format);
    Some(Renderer::new(
        sub_info,
        &mut fb[geo.offset..geo.offset + geo.len],
    ))
}

/// 按可见窗口构造**双缓冲**子渲染器，后台缓冲区由调用方提供。
///
/// 与 [`sub_view`] 的唯一区别是绘制落在 `back` 而非 `fb`，需要调用
/// [`Renderer::swap_buffers`] 才会写回父缓冲区。这一点与顶层
/// [`Renderer::with_back_buffer`] 一致。
///
/// `swap_buffers` 只写回窗口内 `width × bytes_per_pixel` 的可见字节，故呈现
/// 不会波及面板以外的像素——子渲染器切片按父缓冲区的 `pitch` 跨行，整段拷贝
/// 会连相邻行一起覆盖。
///
/// # 后台缓冲区口径
///
/// `back` 必须能容纳 `pitch × 收缩后高度` 字节（与前台切片同口径，含行填充）。
/// 不足时返回 `None`，不 panic。
///
/// # 示例
///
/// ```
/// # use rasteron::{Pixel};
/// # use rasteron::framebuffer::FramebufferInfo;
/// # use rasteron::color::{Color, PixelFormat};
/// # use rasteron::utils::sub_view_with_back_buffer;
/// let pitch = 64 * 4;
/// let info = FramebufferInfo::new(64, 64, pitch, PixelFormat::Rgba8888);
/// let mut fb = vec![0u8; info.buffer_size()];
/// let mut back = vec![0u8; info.buffer_size()];
///
/// {
///     let mut panel =
///         sub_view_with_back_buffer(&mut fb, &mut back, &info, 8, 8, 16, 16).unwrap();
///     panel.fill_rect(Pixel::new(0, 0), 16, 16, Color::new(255, 0, 0));
///     assert!(panel.is_double_buffered());
///     panel.swap_buffers();
/// }
/// // 呈现后只有窗口内的像素被写回，窗口外仍是初始值
/// assert_eq!(&fb[8 * pitch + 8 * 4..8 * pitch + 8 * 4 + 3], &[255, 0, 0]);
/// assert_eq!(fb[0], 0);
/// ```
///
/// # 返回值
///
/// 与 [`sub_view`] 相同的收缩规则；此外 `back` 容量不足时也返回 `None`。
pub fn sub_view_with_back_buffer<'a>(
    fb: &'a mut [u8],
    back: &'a mut [u8],
    info: &FramebufferInfo,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Option<Renderer<'a>> {
    let geo = sub_view_geometry(fb.len(), info, x, y, width, height)?;
    // 后台按与前台完全相同的口径要求（含行填充），短一字节就拒绝。
    if back.len() < geo.len {
        return None;
    }
    let sub_info = FramebufferInfo::new(geo.width, geo.height, info.pitch, info.format);
    Some(Renderer::with_back_buffer(
        sub_info,
        &mut fb[geo.offset..geo.offset + geo.len],
        &mut back[..geo.len],
    ))
}

/// 子视口窗口收缩后的几何结果。
struct SubViewGeometry {
    /// 窗口左上角在父缓冲区中的字节偏移。
    offset: usize,
    /// 窗口切片长度，`pitch × height`（含行填充）。
    len: usize,
    /// 收缩后的可见宽度（像素）。
    width: usize,
    /// 收缩后的可见高度（像素）。
    height: usize,
}

/// 按可见窗口把子视口收缩到 `fb_len` 字节之内。
///
/// [`sub_view`] 与 [`sub_view_with_back_buffer`] 共用此收缩逻辑，两者的窗口
/// 判定完全一致。返回 `None` 表示窗口完全不可用（见 [`sub_view`] 文档）。
fn sub_view_geometry(
    fb_len: usize,
    info: &FramebufferInfo,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Option<SubViewGeometry> {
    if width == 0 || height == 0 || x >= info.width || y >= info.height {
        return None;
    }

    // 上述检查保证 `info.width > 0`，故 `info.pitch >= info.width * bpp > 0`，
    // 下面的除法不会除零。
    let pitch = info.pitch;
    if pitch == 0 {
        return None;
    }
    let bpp = info.bytes_per_pixel();

    // 先收缩到可视区：窗口可能比屏幕还大。
    let sub_w = width.min(info.width - x);
    let sub_h = height.min(info.height - y);

    // 窗口左上角在缓冲区中的字节偏移。用 checked_* 而非裸算术，因为
    // `FramebufferInfo::new` 不限制 width/height，`y * pitch` 在极端输入下会溢出。
    let offset = y.checked_mul(pitch)?.checked_add(x.checked_mul(bpp)?)?;
    let avail = fb_len.checked_sub(offset)?;

    // 可用完整行数：每行必须占据完整的 `pitch` 字节，含行填充。
    let rows = avail / pitch;
    let sub_h = sub_h.min(rows);
    if sub_h == 0 {
        return None;
    }

    // 切片长度取 `pitch × sub_h`：`Renderer::new` 的断言按此口径要求可用字节。
    // 可见像素的写入上界是 `(sub_h - 1) * pitch + sub_w * bpp`，故切片末尾那段
    // 属于父缓冲区下一行的字节不会被任何绘制或 present 触碰（见 `swap_buffers`）。
    let len = pitch.checked_mul(sub_h)?;
    if len > avail {
        return None;
    }

    Some(SubViewGeometry {
        offset,
        len,
        width: sub_w,
        height: sub_h,
    })
}

/// Cohen–Sutherland 线段裁剪算法，针对矩形视口。
///
/// 当线段部分或完全可见时，返回裁剪后的端点 `(Pixel, Pixel)` 对；
/// 当整条线段位于视口之外时，返回 `None`。
///
/// # 坐标系
///
/// Y 轴向下递增（行优先帧缓冲约定）。`(0, 0)` 为左上角。
/// 可见区域 X 范围为 `[0, width)`，Y 范围为 `[0, height)`。
pub fn clip_line(p1: Pixel, p2: Pixel, width: usize, height: usize) -> Option<(Pixel, Pixel)> {
    // Cohen–Sutherland 算法的区域码。
    const INSIDE: u8 = 0; // 0000
    const LEFT: u8 = 1; // 0001
    const RIGHT: u8 = 2; // 0010
    const BOTTOM: u8 = 4; // 0100  (Y 在左上角以下 → 0)
    const TOP: u8 = 8; // 1000    (Y 在底部以上)

    fn compute_code(x: i64, y: i64, width: usize, height: usize) -> u8 {
        let mut code = INSIDE;
        if x < 0 {
            code |= LEFT;
        } else if x >= width as i64 {
            code |= RIGHT;
        }
        if y < 0 {
            code |= BOTTOM;
        } else if y >= height as i64 {
            code |= TOP;
        }
        code
    }

    let (mut x1, mut y1) = (sat_i64(p1.x), sat_i64(p1.y));
    let (mut x2, mut y2) = (sat_i64(p2.x), sat_i64(p2.y));
    let mut code1 = compute_code(x1, y1, width, height);
    let mut code2 = compute_code(x2, y2, width, height);

    loop {
        // 两个点都在内部 → 成功
        if code1 == INSIDE && code2 == INSIDE {
            return Some((
                Pixel::new(x1 as usize, y1 as usize),
                Pixel::new(x2 as usize, y2 as usize),
            ));
        }
        // 两个点在同一外部区域（逻辑与不为0） → 完全不可见
        if (code1 & code2) != 0 {
            return None;
        }

        // 选一个在区域外的点进行裁剪
        let code_out = if code1 != INSIDE { code1 } else { code2 };
        let (mut x, mut y) = (0, 0);

        // 找到线段与边界的交点。
        //
        // 交点公式形如 `a + (b - a) * t / (d - c)`，其中 `(b - a) * t` 的中间
        // 乘积可能远超 `i64` 范围：端点来自 `usize`（`saturating_usize_to_i64`
        // 可产生 `i64::MAX`），而 `t` 最大可达 `height - 1`，两者相乘会溢出。
        // 因此用 `i128` 做中间运算，再把结果收窄回 `i64`。
        if (code_out & TOP) != 0 {
            // y = height - 1  (上边界，假设 y 增大向下)
            x = intersect_axis(x1, x2, height as i64 - 1 - y1, y2 - y1);
            y = height as i64 - 1;
        } else if (code_out & BOTTOM) != 0 {
            // y = 0
            x = intersect_axis(x1, x2, 0 - y1, y2 - y1);
            y = 0;
        } else if (code_out & RIGHT) != 0 {
            // x = width - 1
            y = intersect_axis(y1, y2, width as i64 - 1 - x1, x2 - x1);
            x = width as i64 - 1;
        } else if (code_out & LEFT) != 0 {
            // x = 0
            y = intersect_axis(y1, y2, 0 - x1, x2 - x1);
            x = 0;
        }

        // 更新被裁剪的点及其区域码
        if code_out == code1 {
            x1 = x;
            y1 = y;
            code1 = compute_code(x1, y1, width, height);
        } else {
            x2 = x;
            y2 = y;
            code2 = compute_code(x2, y2, width, height);
        }
    }
}

/// 求解 `a + (b - a) * num / den`，用 `i128` 中间运算避免 `i64` 溢出。
///
/// 用于 Cohen–Sutherland 裁剪中计算线段与边界线的交点。端点由 `usize` 饱和
/// 转换而来，可能取到 `i64::MAX`，直接相乘会溢出。
///
/// `den` 为 0 时（线段平行于该边界且退化）返回 `a`。
#[inline]
fn intersect_axis(a: i64, b: i64, num: i64, den: i64) -> i64 {
    if den == 0 {
        return a;
    }
    let value = a as i128 + (b as i128 - a as i128) * num as i128 / den as i128;
    // 交点在裁剪边界上，理论上必落在 i64 范围内；饱和处理仅为防御。
    value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// 饱和转换：从 `usize` 转换为 `i64`。
///
/// 超过 [`i64::MAX`] 的值会被截断为 [`i64::MAX`]。
/// 内部用于在插值运算中避免负数。
#[inline(always)]
pub(crate) fn sat_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}

/// 将 `f32` 向下取整后转换为 `usize`。
///
/// 负值会被截断为 `0`。用于光栅化坐标转换。
/// 对于非负值，截断（向零取整）等价于 floor。
#[cfg(feature = "programmable")]
#[inline(always)]
pub(crate) fn floor_to_usize(value: f32) -> usize {
    if value < 0.0 { 0 } else { value as usize }
}

/// 向上取整后转为 `usize`；非正值返回 `0`。
///
/// 不能直接用 `f32 as usize`：后者向零取整，对 `2.0` 会得到 2 而非上取整
/// 应有的 3，对 `-1.0` 还会因负数转无符号而得到 0（依赖 Rust 的饱和转换）。
/// 这里显式区分整数值与非整数值，用于三角形包围盒的右/下边界。
#[cfg(feature = "programmable")]
#[inline(always)]
pub fn ceil_to_usize(value: f32) -> usize {
    if value <= 0.0 {
        return 0;
    }
    let truncated = value as usize;
    if truncated as f32 == value {
        truncated
    } else {
        truncated.saturating_add(1)
    }
}

/// 用重复模式填充整个切片：先写入 `pattern`，再按其周期铺满 `row` 剩余部分。
///
/// `pattern` 是**唯一**的数据源——`row` 的内容（含首字节）完全由它决定。
/// 尾部不足一个周期的部分按周期截断。
///
/// # 重叠约束
///
/// `pattern` 不得与 `row` 重叠，唯一例外是恰好等于 `row[..pattern.len()]`
/// （此时首步写入是自赋值，等价于空操作，安全）。
///
/// # 铺开策略
///
/// 对 4bpp 等能整除 16 的格式，模式固定为 16 字节：按 16 字节块**直接重复写入**，
/// 而不是"复制已填部分来翻倍"。后者需要 `O(log n)` 次串行 `memcpy`，
/// 每次的源区域都不同、无法向量化，且总写入量随步数累积；直接块写是
/// 单一 steady-state 循环，流水线友好。
///
/// 3bpp 无法整除 16，退回逐次倍增复制：源始终是已就位的前缀 `row[..filled]`，
/// 因此首步写入模式是这条路径正确的前提。
#[inline(always)]
pub(crate) fn pattern_fill(row: &mut [u8], pattern: &[u8]) {
    let len = row.len();
    let pattern_len = pattern.len();
    if pattern_len == 0 || len == 0 {
        return;
    }

    // 写入首个模式。倍增路径的复制源就是 `row` 前缀，故此步不可省略。
    if len <= pattern_len {
        row.copy_from_slice(&pattern[..len]);
        return;
    }
    row[..pattern_len].copy_from_slice(pattern);

    // 16 字节定长块：直接铺满，无倍增链
    if pattern_len == 16 {
        let mut block = [0u8; 16];
        block.copy_from_slice(&row[..16]);
        let mut offset = 16;
        while offset + 16 <= len {
            row[offset..offset + 16].copy_from_slice(&block);
            offset += 16;
        }
        if offset < len {
            let tail = len - offset;
            row[offset..].copy_from_slice(&block[..tail]);
        }
        return;
    }

    // 非 16 字节模式（3bpp）：逐次倍增复制
    let base = row.as_mut_ptr();
    let mut filled = pattern_len;
    while filled < len {
        let copy_len = core::cmp::min(filled, len - filled);
        unsafe {
            #[cfg(feature = "simd")]
            simd::memcpy(base.add(filled), base, copy_len);
            #[cfg(not(feature = "simd"))]
            core::ptr::copy_nonoverlapping(base, base.add(filled), copy_len);
        }
        filled += copy_len;
    }
}

/// 把 normalized `[0,1]²` 的 viewport 坐标变换为浮点像素坐标。
///
/// # 语义：边缘空间（edge space），与主流图形 API 一致
///
/// 本函数输出的是**连续区间** `[x, x + width]` 上的坐标，而非像素中心坐标：
///
/// # 退化视口
///
/// viewport 宽或高为 0 时按 1 处理，避免除零与负跨度；此时 `1.0` 恰好落在
/// `x + 1`，仍是半开区间的开边界。
///
/// # 示例
///
/// ```
/// # use rasteron::programmable::Viewport;
/// # use rasteron::utils::normalized_to_screen;
/// let viewport = Viewport::new(10, 20, 100, 50);
/// // [0, 0] 是左上边缘本身。
/// assert_eq!(normalized_to_screen([0.0, 0.0], viewport), [10.0, 20.0]);
/// // [1, 1] 是右下边缘，已越过末像素一像素。
/// assert_eq!(normalized_to_screen([1.0, 1.0], viewport), [110.0, 70.0]);
/// // 0.5 是几何中心。
/// assert_eq!(normalized_to_screen([0.5, 0.5], viewport), [60.0, 45.0]);
/// ```
#[cfg(feature = "programmable")]
pub fn normalized_to_screen(position: Vec2, viewport: Viewport) -> Vec2 {
    let vx = viewport.x as f32;
    let vy = viewport.y as f32;
    let vw = viewport.width.max(1) as f32;
    let vh = viewport.height.max(1) as f32;
    [vx + position[0] * vw, vy + position[1] * vh]
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use crate::color::{Color, PixelFormat};
    use alloc::vec;
    use alloc::vec::Vec;

    // ══════════════════════════════ S18 · 子视口 ══════════════════════════════

    /// 父帧缓冲的固定规格：64×64、4bpp、pitch 对齐到 256 字节（每行 64 字节填充）。
    /// 带填充是刻意的——它能让「窗口偏移是否按 pitch 跨行」成为可观测的差异。
    const FB_W: usize = 64;
    const FB_H: usize = 64;
    const FB_PITCH: usize = FB_W * 4;

    /// 造一个填满哨兵值的父帧缓冲，`Color` 覆盖后仍能靠字节识别未写区域。
    fn make_parent() -> (FramebufferInfo, Vec<u8>) {
        let info = FramebufferInfo::new(FB_W, FB_H, FB_PITCH, PixelFormat::Rgba8888);
        let fb = vec![0xAB; info.buffer_size()];
        (info, fb)
    }

    /// 正常窗口：`info()` 的宽高与请求一致，`pitch` 原样继承父帧缓冲。
    #[test]
    fn sub_view_normal_window_reports_requested_geometry() {
        let (info, mut fb) = make_parent();
        let panel = sub_view(&mut fb, &info, 8, 8, 16, 16).expect("窗口完全可用");
        assert_eq!((panel.info().width, panel.info().height), (16, 16));
        // pitch 不变，故窗口内像素仍落在正确的物理位置。
        assert_eq!(panel.info().pitch, FB_PITCH);
        assert_eq!(panel.info().format, PixelFormat::Rgba8888);
    }

    /// `None` 的三种判定：零尺寸（宽/高任一）、起点越界（x/y 越界）。
    /// 这些是**完全不绘制**的窗口，必须返回 `None` 而不是 panic。
    #[test]
    fn sub_view_rejects_zero_size_and_out_of_range_origin() {
        let (info, mut fb) = make_parent();
        // 零宽 / 零高。
        assert!(sub_view(&mut fb, &info, 8, 8, 0, 16).is_none(), "零宽应拒绝");
        assert!(sub_view(&mut fb, &info, 8, 8, 16, 0).is_none(), "零高应拒绝");
        // 起点恰在右/下边界（`>=` 而非 `>`）：越出可见区一像素即不可用。
        assert!(
            sub_view(&mut fb, &info, FB_W, 8, 16, 16).is_none(),
            "x == width 应拒绝"
        );
        assert!(
            sub_view(&mut fb, &info, 8, FB_H, 16, 16).is_none(),
            "y == height 应拒绝"
        );
        // 起点远超边界。
        assert!(
            sub_view(&mut fb, &info, 9999, 8, 16, 16).is_none(),
            "x 远超应拒绝"
        );
    }

    /// 窗口比屏幕还大时按 `info.width - x` / `info.height - y` 收缩，
    /// 而不是拒绝——这是「静默裁剪」契约的一部分。
    #[test]
    fn sub_view_shrinks_to_visible_area() {
        let (info, mut fb) = make_parent();
        // 起点在 (0, 0)：窗口只需按可视区收缩到 64×64，不受可用行数限制。
        let panel = sub_view(&mut fb, &info, 0, 0, 1000, 1000).expect("应收缩而非拒绝");
        assert_eq!((panel.info().width, panel.info().height), (64, 64));

        // 只在宽度方向超出：60 + 16 越过 64，收缩到 4。
        let panel = sub_view(&mut fb, &info, 60, 8, 16, 16).expect("仅宽度收缩");
        assert_eq!((panel.info().width, panel.info().height), (4, 16));
    }

    /// 收缩受**两个上限**约束，取更小者：可视区（`info.height - y`）与可用完整行数
    /// （`(fb.len() - off) / pitch`）。
    ///
    /// 父帧缓冲 64×64、pitch 256、bpp 4，故 `buffer_size() = 16384`。按
    /// `rows = (pitch × height - y × pitch - x × bpp) / pitch` 展开：
    ///
    /// * `x == 0` 时 `rows == 64 - y`，与可视高度相同，行数上限不额外收紧；
    /// * `x > 0` 时多出 `x × bpp / pitch` 的非整行偏移，整除后**恒定少一行**——
    ///   这正是 `sub_view` 文档所说「窗口贴住缓冲区末尾时可能少最后一行」的成因。
    #[test]
    fn sub_view_shrinks_to_smaller_of_visible_area_and_available_rows() {
        let (info, mut fb) = make_parent();
        assert_eq!(info.buffer_size(), FB_PITCH * FB_H, "本测试以 pitch × height 为口径");

        // x == 0：行数上限与可视高度一致，不额外收紧。
        let panel = sub_view(&mut fb, &info, 0, 8, 1000, 1000).expect("应收缩");
        assert_eq!(
            (panel.info().width, panel.info().height),
            (64, 56),
            "x = 0 时高度由可视区决定（64 - 8 = 56）"
        );

        // x > 0：可视区允许 56 行，但可用整行只有 55 行，取小者。
        let panel = sub_view(&mut fb, &info, 8, 8, 1000, 1000).expect("应收缩");
        assert_eq!(
            (panel.info().width, panel.info().height),
            (56, 55),
            "x = 8 时宽度取可视区 56，高度取可用行数 55"
        );

        // 起点上移一行，可视区放宽到 60 行，可用行数同步放宽到 59 行——仍差 1。
        let panel = sub_view(&mut fb, &info, 8, 4, 1000, 1000).expect("应收缩");
        assert_eq!(
            (panel.info().width, panel.info().height),
            (56, 59),
            "x = 8 恒定比可视高度少一行（64 - 4 - 1 = 59）"
        );

        // 宽高请求都小于上限时，行数限制不生效。
        let panel = sub_view(&mut fb, &info, 60, 8, 16, 16).expect("应收缩");
        assert_eq!((panel.info().width, panel.info().height), (4, 16));
    }

    /// 窗口贴住缓冲区末尾时行数收缩到 `(fb.len() - off) / pitch`，**可能少最后一行**。
    /// 文档明确这是 `pitch` 行填充被计入的必然结果，不是缺陷。
    #[test]
    fn sub_view_row_count_shrinks_at_buffer_end() {
        let (info, mut fb) = make_parent();
        // 最后一行（y = 63）只剩 1 个完整行：高度 4 收缩到 1。
        let panel = sub_view(&mut fb, &info, 0, FB_H - 1, 4, 4).expect("末行仍可用");
        assert_eq!(panel.info().height, 1, "贴末尾时应收缩到 1 行");

        // 往上挪一行则拿到 2 行，验证收缩量确实由可用字节除以 pitch 决定。
        let panel = sub_view(&mut fb, &info, 0, FB_H - 2, 4, 4).expect("倒数第二行起可用 2 行");
        assert_eq!(panel.info().height, 2);
    }

    /// 放不下任何**完整行**时返回 `None`（不做「半行也算」的宽松处理）。
    /// 窗口偏移 2080 字节（8 行 × pitch + 8 像素 × 4bpp）处需要整整一个 pitch。
    #[test]
    fn sub_view_rejects_when_no_complete_row_fits() {
        let info = FramebufferInfo::new(FB_W, FB_H, FB_PITCH, PixelFormat::Rgba8888);
        let offset = 8 * FB_PITCH + 8 * 4;
        // 短一个字节：放不下完整行。
        let mut too_short = vec![0u8; offset + FB_PITCH - 1];
        assert!(
            sub_view(&mut too_short, &info, 8, 8, 16, 16).is_none(),
            "差一字节放不下整行应拒绝"
        );
        // 恰好一个整行：接受，高度收缩到 1。
        let mut exactly_one = vec![0u8; offset + FB_PITCH];
        let panel = sub_view(&mut exactly_one, &info, 8, 8, 16, 16).expect("恰一整行应接受");
        assert_eq!(panel.info().height, 1);
        // 零字节可用：拒绝。
        let mut empty = vec![0u8; offset];
        assert!(sub_view(&mut empty, &info, 8, 8, 16, 16).is_none(), "无完整行应拒绝");
    }

    /// `sub_view_with_back_buffer` 的窗口判定与收缩几何**完全等同** `sub_view`：
    /// 同样的输入必须给出同样的 `width`/`height`（否则两个 API 会画到不同区域）。
    #[test]
    fn sub_view_variants_share_identical_geometry() {
        let cases: [(usize, usize, usize, usize); 5] = [
            (8, 8, 16, 16),
            (8, 8, 1000, 1000),
            (60, 8, 16, 16),
            (0, FB_H - 1, 4, 4),
            (0, FB_H - 2, 4, 4),
        ];
        for (x, y, w, h) in cases {
            let (info, mut fb) = make_parent();
            // 两个子渲染器都要可变借用 `fb`，故分处两个作用域，只把几何取出来比较。
            let single = {
                let single = sub_view(&mut fb, &info, x, y, w, h);
                single.map(|r| (r.info().width, r.info().height, r.info().pitch))
            };
            let mut back = vec![0u8; info.buffer_size()];
            let double = {
                let double = sub_view_with_back_buffer(&mut fb, &mut back, &info, x, y, w, h);
                double.map(|r| (r.info().width, r.info().height, r.info().pitch))
            };
            match (single, double) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert_eq!(a, b, "窗口 ({x},{y},{w},{h}) 两个变体几何不一致");
                }
                _ => panic!("窗口 ({x},{y},{w},{h}) 两个变体可用性不一致"),
            }
        }
    }

    /// 双缓冲的 `back` 口径是 `pitch × 收缩后高度`（**含行填充**）：短一字节即拒绝。
    #[test]
    fn sub_view_back_buffer_requires_exact_pitch_times_height() {
        let (info, mut fb) = make_parent();
        let needed = FB_PITCH * 16; // 16 行 × 256 字节
        let mut back = vec![0u8; needed];
        assert!(
            sub_view_with_back_buffer(&mut fb, &mut back, &info, 8, 8, 16, 16).is_some(),
            "恰好 pitch × height 应接受"
        );

        let mut back = vec![0u8; needed - 1];
        assert!(
            sub_view_with_back_buffer(&mut fb, &mut back, &info, 8, 8, 16, 16).is_none(),
            "短一字节应拒绝（口径含行填充）"
        );
    }

    /// 收缩后的窗口其 `back` 需求按**收缩后**高度算：56×55 的窗口只需
    /// `pitch × 55`（14080 字节），而非请求时的 1000 行。
    #[test]
    fn sub_view_back_buffer_uses_shrunk_height() {
        let (info, mut fb) = make_parent();
        // 收缩后 55 行 → 14080 字节（见 `sub_view_shrinks_to_smaller_of_...`）。
        let needed = FB_PITCH * 55;
        let mut back = vec![0u8; needed];
        assert!(
            sub_view_with_back_buffer(&mut fb, &mut back, &info, 8, 8, 1000, 1000).is_some(),
            "back 按收缩后高度 {needed} 应足够"
        );
        let mut back = vec![0u8; needed - 1];
        assert!(
            sub_view_with_back_buffer(&mut fb, &mut back, &info, 8, 8, 1000, 1000).is_none(),
            "收缩后容量差一字节应拒绝"
        );
    }

    /// `swap_buffers` 只写回窗口内 `width × bpp` 的**可见**字节：
    /// 面板外的像素、以及行填充字节，都必须逐字节保持原值。
    #[test]
    fn swap_buffers_writes_only_visible_panel_pixels() {
        let (info, mut fb) = make_parent();
        let mut back = vec![0u8; info.buffer_size()];
        let panel_snapshot = {
            let mut panel =
                sub_view_with_back_buffer(&mut fb, &mut back, &info, 8, 8, 16, 16).unwrap();
            panel.fill_rect(Pixel::new(0, 0), 16, 16, Color::new(255, 0, 0));
            panel.swap_buffers();
            // 面板内确实是红像素。
            panel.info().width
        };
        assert_eq!(panel_snapshot, 16);

        // 窗口左上角像素为红。
        assert_eq!(&fb[8 * FB_PITCH + 8 * 4..8 * FB_PITCH + 8 * 4 + 4], &[255, 0, 0, 255]);
        // 窗口外一像素仍是哨兵 0xAB。
        assert_eq!(fb[0], 0xAB, "面板外首字节被改写");
        assert_eq!(fb[7 * FB_PITCH + 7 * 4], 0xAB, "窗口左上角外侧被改写");
        // 行填充字节（第 0 行 x = 16 之后）必须原样保留。
        assert_eq!(fb[64 * 4], 0xAB, "行填充字节被整段 memcpy 覆盖");

        // 窗口内未被覆盖到的位置同样保持哨兵：只写 visible 宽度。
        assert_eq!(fb[8 * FB_PITCH + 24 * 4], 0xAB, "窗口右侧行填充被改写");
    }

    /// 等价性：子视口内的绘制 == 父视口上按窗口偏移的同色绘制。
    /// 这是 `sub_view` 的核心价值——调用方应能把它当普通 `Renderer` 用。
    #[test]
    fn sub_view_drawing_equals_offset_parent_drawing() {
        let (info, mut fb_sub) = make_parent();
        let (_, mut fb_ref) = make_parent();

        {
            let mut panel = sub_view(&mut fb_sub, &info, 8, 8, 16, 16).unwrap();
            panel.fill_rect(Pixel::new(2, 3), 5, 4, Color::new(10, 200, 30));
        }
        {
            let mut parent = Renderer::new(info, &mut fb_ref);
            // 同样的图元，按窗口偏移平移 (8 + 2, 8 + 3)。
            parent.fill_rect(Pixel::new(10, 11), 5, 4, Color::new(10, 200, 30));
        }

        assert_eq!(
            fb_sub, fb_ref,
            "子视口绘制应与父视口偏移绘制逐字节一致"
        );
    }

    /// 子视口的填充不得越界写到窗口之外：用一个**远大于**窗口的矩形填充，
    /// 只应影响窗口覆盖的 16×16 区域。
    #[test]
    fn sub_view_clips_drawing_to_window_bounds() {
        let (info, mut fb) = make_parent();
        {
            let mut panel = sub_view(&mut fb, &info, 8, 8, 16, 16).unwrap();
            // 远超窗口的矩形，溢出部分必须被裁掉。
            panel.fill_rect(Pixel::new(0, 0), 1000, 1000, Color::new(255, 0, 0));
        }
        // 窗口左上角被写。
        assert_eq!(fb[8 * FB_PITCH + 8 * 4], 255);
        // 紧邻窗口右侧的一像素未被写。
        assert_eq!(fb[8 * FB_PITCH + 24 * 4], 0xAB, "窗口右侧溢出被写入");
        // 窗口上方一行未被写。
        assert_eq!(fb[7 * FB_PITCH + 8 * 4], 0xAB, "窗口上方溢出被写入");
    }

    // ══════════════════════ S19 · clip_line（Cohen–Sutherland）══════════════════════

    /// 判定返回值端点是否落在可见半开区间 `[0, width) × [0, height)`。
    fn assert_in_bounds(result: Option<(Pixel, Pixel)>, width: usize, height: usize, label: &str) {
        let (a, b) = result.unwrap_or_else(|| panic!("{label}: 期望 Some，实际 None"));
        for (name, p) in [("p1", a), ("p2", b)] {
            assert!(
                p.x < width && p.y < height,
                "{label}: 端点 {name} = ({}, {}) 越界，应落在 [0,{width}) × [0,{height})",
                p.x,
                p.y
            );
        }
    }

    /// 验证「共线且方向保持」。
    ///
    /// **容差说明**：`intersect_axis` 用整数除法算交点，`(b-a)*num/den` 向零截断，
    /// 误差最大为 1 像素。故按**垂直距离**判定共线：
    /// `|cross(q - p1, d)| / |d| <= 1`，其中 `d = p2 - p1`。
    /// 截断误差沿法向投影后恒 ≤ 1 像素，故这个界是紧的且能覆盖全部截断误差。
    fn assert_collinear_and_oriented(p1: Pixel, p2: Pixel, q1: Pixel, q2: Pixel, label: &str) {
        // 端点先经 `sat_i64` 饱和再参与运算，与算法内部一致；差值用 `i128`
        // 承载，否则 `usize::MAX` 级别的端点会让 `i64` 差值溢出。
        let (x1, y1, x2, y2) = (
            sat_i64(p1.x) as i128,
            sat_i64(p1.y) as i128,
            sat_i64(p2.x) as i128,
            sat_i64(p2.y) as i128,
        );
        let d = (x2 - x1, y2 - y1);
        let len = ((d.0 as f64).powi(2) + (d.1 as f64).powi(2)).sqrt();
        let perp = |q: Pixel| -> f64 {
            let (qx, qy) = (sat_i64(q.x) as i128, sat_i64(q.y) as i128);
            let cross = (qx - x1) as f64 * d.1 as f64 - (qy - y1) as f64 * d.0 as f64;
            cross.abs() / len
        };
        for (name, q) in [("q1", q1), ("q2", q2)] {
            assert!(
                perp(q) <= 1.0 + f64::EPSILON,
                "{label}: {name} 偏离原直线 {:.3} 像素（整数截断容差为 1 像素）",
                perp(q)
            );
        }
        // 方向保持：裁剪后的方向向量与原方向点积非负（只截短，不反向）。
        let clipped = (
            sat_i64(q2.x) as i128 - sat_i64(q1.x) as i128,
            sat_i64(q2.y) as i128 - sat_i64(q1.y) as i128,
        );
        assert!(
            d.0 * clipped.0 + d.1 * clipped.1 >= 0,
            "{label}: 裁剪后方向与原方向相反"
        );
    }

    /// 完全在视口内的线段原样返回，端点逐字不变。
    #[test]
    fn clip_line_returns_interior_segments_unchanged() {
        let cases = [
            (Pixel::new(2, 3), Pixel::new(10, 8)),
            // 恰好贴住左上角原点。
            (Pixel::new(0, 0), Pixel::new(5, 5)),
            // 恰好贴住右下角像素 (width-1, height-1)。
            (Pixel::new(19, 14), Pixel::new(10, 8)),
            (Pixel::new(19, 14), Pixel::new(0, 0)),
        ];
        for (p1, p2) in cases {
            assert_eq!(
                clip_line(p1, p2, 20, 15),
                Some((p1, p2)),
                "完全在内的 ({}, {})→({}, {}) 应原样返回",
                p1.x,
                p1.y,
                p2.x,
                p2.y
            );
        }
    }

    /// 完全在外的线段返回 `None`。
    ///
    /// **重要**：`Pixel` 的字段是 `usize`，负坐标**无法表达**——所以
    /// 「左外 / 上外」这两个方向在公开 API 上不可构造，`compute_code` 里的
    /// `LEFT` / `BOTTOM` 分支只能由内部（`sat_i64` 之后仍为负的情形）触达。
    /// 这里覆盖 `usize` 能表示的三个方向：右外、下外、右下同码。
    #[test]
    fn clip_line_returns_none_for_fully_exterior_segments() {
        let cases = [
            // 右外：x >= width，两点同带 RIGHT 码。
            (Pixel::new(21, 3), Pixel::new(30, 7)),
            (Pixel::new(100, 3), Pixel::new(300, 9)),
            // 下外：y >= height。
            (Pixel::new(3, 16), Pixel::new(7, 20)),
            // 右下外：RIGHT | TOP 同码，直接被 `code1 & code2` 拒绝。
            (Pixel::new(25, 25), Pixel::new(30, 30)),
        ];
        for (p1, p2) in cases {
            assert_eq!(
                clip_line(p1, p2, 20, 15),
                None,
                "完全在外的 ({}, {})→({}, {}) 应返回 None",
                p1.x,
                p1.y,
                p2.x,
                p2.y
            );
        }
    }

    /// 退化线段 `p1 == p2`：在视口内返回 `Some`（同点），在外返回 `None`。
    #[test]
    fn clip_line_handles_degenerate_points() {
        let inside = Pixel::new(7, 7);
        assert_eq!(
            clip_line(inside, inside, 20, 15),
            Some((inside, inside)),
            "视口内的退化点应原样返回"
        );
        // 退化点恰好落在右边界外 / 下边界外。
        assert_eq!(clip_line(Pixel::new(20, 7), Pixel::new(20, 7), 20, 15), None);
        assert_eq!(clip_line(Pixel::new(7, 15), Pixel::new(7, 15), 20, 15), None);
    }

    /// 跨越边界时，返回端点必须落在可见区内，且**保留未越界的那个端点**。
    #[test]
    fn clip_line_trims_crossing_segments_into_viewport() {
        let cases = [
            // 跨右边界。
            (Pixel::new(10, 10), Pixel::new(30, 5), "跨右"),
            // 跨下边界。
            (Pixel::new(5, 10), Pixel::new(10, 30), "跨下"),
            // 水平穿过右边界。
            (Pixel::new(7, 7), Pixel::new(30, 7), "水平穿右"),
            // 垂直穿过下边界。
            (Pixel::new(10, 10), Pixel::new(10, 30), "垂直穿下"),
            // 对角穿过右下角。
            (Pixel::new(0, 0), Pixel::new(30, 30), "对角穿"),
            // 斜穿。
            (Pixel::new(2, 3), Pixel::new(25, 19), "斜穿"),
            // 擦过右上角。
            (Pixel::new(18, 1), Pixel::new(30, 13), "擦右上"),
            // 擦过左下角。
            (Pixel::new(1, 13), Pixel::new(14, 30), "擦左下"),
        ];
        for (p1, p2, label) in cases {
            let result = clip_line(p1, p2, 20, 15);
            assert_in_bounds(result, 20, 15, label);
            let (q1, q2) = result.unwrap();
            // 方向保持：原始的「内→外」顺序下，q1 必须是内点、q2 是裁出的边界点。
            assert_eq!(q1, p1, "{label}: 未越界端点应原样保留");
            assert_collinear_and_oriented(p1, p2, q1, q2, label);
        }
    }

    /// 只露出一角 / 只露出一个点的情形：结果仍是合法线段（长度可为 0）。
    #[test]
    fn clip_line_handles_corner_and_single_pixel_exposure() {
        // 只露一个角：从左上角擦过。
        let result = clip_line(Pixel::new(0, 3), Pixel::new(4, 0), 20, 15);
        assert_in_bounds(result, 20, 15, "切左上角");
        // 只露一个点：两端都被裁到同一点，线段退化但仍为 Some。
        let result = clip_line(Pixel::new(0, 14), Pixel::new(3, 17), 20, 15);
        assert_eq!(result, Some((Pixel::new(0, 14), Pixel::new(0, 14))), "擦过下角应收缩为单点");
        // 切右下角。
        let result = clip_line(Pixel::new(16, 14), Pixel::new(20, 11), 20, 15);
        assert_in_bounds(result, 20, 15, "切右下角");
        // 单像素宽的角落暴露。
        let result = clip_line(Pixel::new(2, 2), Pixel::new(3, 0), 20, 15);
        assert_in_bounds(result, 20, 15, "单像素角");
    }

    /// 参数化验证共线与方向保持，覆盖一批斜率与穿越方向各异的线段。
    #[test]
    fn clip_line_preserves_collinearity_and_orientation() {
        let p1 = Pixel::new(1, 2);
        // 终点沿一整圈扫过 `(dx, dy)`，覆盖浅坡到陡坡、以及全部象限。
        for dx in 1..=40usize {
            for dy in 1..=30usize {
                let p2 = Pixel::new(p1.x + dx, p1.y + dy);
                let Some((q1, q2)) = clip_line(p1, p2, 20, 15) else {
                    // 完全在外是合法结果；此时不要求共线。
                    continue;
                };
                assert_in_bounds(Some((q1, q2)), 20, 15, "参数化");
                assert_collinear_and_oriented(p1, p2, q1, q2, "参数化");
            }
        }
    }

    /// 巨大端点（`usize::MAX`）走 `i128` 中间运算路径：不溢出、不 panic，
    /// 结果仍落在可见区内。这是 `intersect_axis` 的 `i128` 护栏的直接验证。
    #[test]
    fn clip_line_handles_huge_endpoints_without_overflow() {
        let cases: [(Pixel, Pixel); 6] = [
            // 两轴都取 usize::MAX：`(b - a) * num` 的中间乘积远超 i64。
            (Pixel::new(usize::MAX, usize::MAX), Pixel::new(5, 5)),
            // 原点连向 usize::MAX。
            (Pixel::new(0, 0), Pixel::new(usize::MAX, usize::MAX)),
            // 单轴巨大：水平与垂直各一。
            (Pixel::new(usize::MAX, 3), Pixel::new(4, 3)),
            (Pixel::new(3, usize::MAX), Pixel::new(3, 4)),
            // `i64::MAX`（`sat_i64` 的饱和上界）。
            (Pixel::new(i64::MAX as usize, i64::MAX as usize), Pixel::new(2, 2)),
            // 巨大且完全在外：同码直接拒绝，不进入插值。
            (Pixel::new(usize::MAX, usize::MAX), Pixel::new(usize::MAX, usize::MAX)),
        ];
        for (p1, p2) in cases {
            let result = clip_line(p1, p2, 20, 15);
            if p1 == p2 {
                assert_eq!(result, None, "巨大的同点在视口外应返回 None");
                continue;
            }
            assert_in_bounds(result, 20, 15, "巨大端点");
            let (q1, q2) = result.unwrap();
            assert_collinear_and_oriented(p1, p2, q1, q2, "巨大端点");
        }
    }

    /// 零尺寸视口：可见区为空，任何线段都不可见，一律 `None`（不 panic、不死循环）。
    #[test]
    fn clip_line_on_empty_viewport_always_none() {
        for (w, h) in [(0usize, 10usize), (10, 0), (0, 0)] {
            assert_eq!(
                clip_line(Pixel::new(0, 0), Pixel::new(5, 5), w, h),
                None,
                "{w}×{h} 视口应拒绝一切线段"
            );
            assert_eq!(
                clip_line(Pixel::new(1, 1), Pixel::new(9, 9), w, h),
                None,
                "{w}×{h} 视口应拒绝一切线段"
            );
        }
    }

    /// `intersect_axis` 的 `den == 0` 平行退化分支：直接返回 `a`（不做除法）。
    ///
    /// 这是私有内核，但 `clip_line` 的 `den == 0` 分支唯一可由公开 API 触达的方式
    /// 需要 `usize` 无法表达的负坐标，故此处直接测内核（测试与实现同模块）。
    #[test]
    fn intersect_axis_returns_a_when_denominator_is_zero() {
        assert_eq!(intersect_axis(7, 99, 123, 0), 7, "den == 0 应返回 a");
        assert_eq!(intersect_axis(-5, 99, 3, 0), -5, "den == 0 应原样返回 a（含负值）");
        assert_eq!(intersect_axis(0, 0, 0, 0), 0);
        assert_eq!(intersect_axis(i64::MAX, i64::MIN, 5, 0), i64::MAX);
    }

    /// `intersect_axis` 的正常路径：`a + (b - a) * num / den`，整数除法向零截断。
    #[test]
    fn intersect_axis_truncates_toward_zero() {
        // 10 + (20 - 10) × 5 / 2 = 10 + 25 = 35（此处整除，无误差）。
        assert_eq!(intersect_axis(10, 20, 5, 2), 35);
        // 10 + 10 × 1 / 3 = 13.33… → 13。
        assert_eq!(intersect_axis(10, 20, 1, 3), 13);
        // 负数同样向零截断（不是向下取整）：0 + 10 × (-1) / 4 = -2.5 → -2。
        assert_eq!(intersect_axis(0, 10, -1, 4), -2);
        // 负 `num` 搭配正 `den` 也是向零：10 + 10 × (-1) / 3 = 10 - 3.33… → 7。
        assert_eq!(intersect_axis(10, 20, -1, 3), 7);
        // `i128` 中间量不溢出：`(i64::MAX - 0) * i64::MAX` 远超 i64 范围，
        // 结果被 clamp 回 i64::MAX 而非回绕成负数。
        assert_eq!(intersect_axis(0, i64::MAX, i64::MAX, 1), i64::MAX);
    }

    // ═══════════════════ S20 · pattern_fill 与数值助手 ═══════════════════

    /// 模式长度整除行长：铺满后每 `k` 个字节重复一次，无残留。
    #[test]
    fn pattern_fill_when_pattern_divides_row_length() {
        let mut row = [0u8; 12];
        pattern_fill(&mut row, &[1, 2, 3, 4]);
        assert_eq!(row, [1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4]);
    }

    /// 模式长度不整除行长：尾部按周期截断，不补零、不越界。
    #[test]
    fn pattern_fill_when_pattern_does_not_divide_row_length() {
        let mut row = [0u8; 10];
        pattern_fill(&mut row, &[1, 2, 3]);
        assert_eq!(row, [1, 2, 3, 1, 2, 3, 1, 2, 3, 1], "尾部应按周期截断");
    }

    /// 行长短于模式长：只写入 `row` 的长度，绝不越界 panic。
    #[test]
    fn pattern_fill_when_row_is_shorter_than_pattern() {
        let mut row = [0xFFu8; 3];
        pattern_fill(&mut row, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(row, [1, 2, 3], "只写入行长范围");

        // 行长为 1。
        let mut row = [0xFFu8; 1];
        pattern_fill(&mut row, &[9, 8]);
        assert_eq!(row, [9]);
    }

    /// 空模式或空行：直接返回，不写入任何字节。
    #[test]
    fn pattern_fill_with_empty_pattern_or_row_is_noop() {
        // 空模式：行保持原样（首字节也不被写——`pattern` 是唯一数据源）。
        let mut row = [0x5Au8; 8];
        pattern_fill(&mut row, &[]);
        assert_eq!(row, [0x5A; 8], "空模式不应写入任何字节");

        // 空行：即使有模式也不 panic。
        let mut row: [u8; 0] = [];
        pattern_fill(&mut row, &[1, 2, 3]);
        assert_eq!(row.len(), 0);
    }

    /// 单字节模式：整行铺成该值（`pattern_len == 1` 走倍增路径）。
    #[test]
    fn pattern_fill_with_single_byte_pattern() {
        let mut row = [0u8; 33]; // 故意用奇数长度覆盖倍增尾步
        pattern_fill(&mut row, &[0xAB]);
        assert_eq!(row, [0xAB; 33]);

        // 行长为 1（`len <= pattern_len` 的提前返回分支）。
        let mut row = [0u8; 1];
        pattern_fill(&mut row, &[7]);
        assert_eq!(row, [7]);
    }

    /// 16 字节模式：走「16 字节定长块直接重复」路径，非 16 的倍数长度由尾块补齐。
    #[test]
    fn pattern_fill_with_sixteen_byte_pattern() {
        let pattern: [u8; 16] = core::array::from_fn(|i| i as u8);
        for len in [16usize, 17, 32, 48, 64, 100] {
            let mut row = vec![0u8; len];
            pattern_fill(&mut row, &pattern);
            for (i, byte) in row.iter().enumerate() {
                assert_eq!(
                    *byte,
                    pattern[i % 16],
                    "len = {len} 时第 {i} 字节应为模式的第 {} 字节",
                    i % 16
                );
            }
        }
    }

    /// 非 16 字节且非 1 的模式（3bpp 真实场景）：倍增复制路径，结果必须仍是
    /// 正确周期铺开——这是该路径存在的唯一理由。
    #[test]
    fn pattern_fill_doubling_path_yields_correct_period() {
        // 3bpp 行在 16 像素处为 48 字节，不是 16 的倍数。
        let pattern = [0x11u8, 0x22, 0x33];
        for len in [49usize, 50, 96, 97] {
            let mut row = vec![0u8; len];
            pattern_fill(&mut row, &pattern);
            for (i, byte) in row.iter().enumerate() {
                assert_eq!(*byte, pattern[i % 3], "len = {len} 第 {i} 字节");
            }
        }
    }

    /// 倍增路径不得依赖「复制已填前缀」的顺序性：模式首字节必须来自 `pattern`，
    /// 而不是行里原有的值。
    #[test]
    fn pattern_fill_overwrites_existing_content() {
        let mut row = [0xEEu8; 9];
        pattern_fill(&mut row, &[1, 2, 3]);
        assert_eq!(row, [1, 2, 3, 1, 2, 3, 1, 2, 3]);
    }

    /// `sat_i64`：`usize → i64` 的唯一合法入口。`0` 与正常值恒等，
    /// 超过 `i64::MAX` 的值饱和（而非像 `as` 那样回绕成负数）。
    #[test]
    fn sat_i64_saturates_instead_of_wrapping() {
        assert_eq!(sat_i64(0), 0);
        assert_eq!(sat_i64(42), 42);
        assert_eq!(sat_i64(i64::MAX as usize), i64::MAX);
        // `i64::MAX as usize` 是 `usize` 里能精确表示的最大值，原样通过。
        assert_eq!(sat_i64((i64::MAX as usize) + 1), i64::MAX);
        // 关键对照：`usize::MAX as i64` 会回绕成 -1（这正是禁止裸 `as` 的原因）。
        assert_eq!(usize::MAX as i64, -1, "`as` 回绕的前提，用于对比");
        assert_eq!(sat_i64(usize::MAX), i64::MAX, "饱和转换不得回绕成负数");
        assert!(sat_i64(usize::MAX) > 0);
    }

    // ── 以下 f32 助手受 `programmable` feature 门控 ──

    /// `floor_to_usize`：负值归零（`value < 0.0` 分支），非负值向零截断。
    ///
    /// **按代码实际语义断言**：`NaN < 0.0` 为 `false`，故 `NaN` 落到 `value as usize`；
    /// Rust 的饱和浮点→整数转换把 `NaN` 定义为 0，所以返回 0。
    #[test]
    #[cfg(feature = "programmable")]
    fn floor_to_usize_handles_negative_nan_and_infinity() {
        // 负值 → 0（`-0.5` 属于 `value < 0.0`，`-0.0` 不属于）。
        assert_eq!(floor_to_usize(-1.0), 0);
        assert_eq!(floor_to_usize(-0.5), 0);
        assert_eq!(floor_to_usize(f32::NEG_INFINITY), 0);
        // `-0.0 < 0.0` 为 false，落到 `as usize` → 0。
        assert_eq!(floor_to_usize(-0.0), 0);
        // 非负小数向零截断（等价于 floor）。
        assert_eq!(floor_to_usize(0.0), 0);
        assert_eq!(floor_to_usize(0.5), 0);
        assert_eq!(floor_to_usize(1.9), 1);
        assert_eq!(floor_to_usize(2.0), 2);
        // `NaN` → 0：饱和转换对 `NaN` 的定义值。
        assert_eq!(floor_to_usize(f32::NAN), 0);
        // `+∞` 饱和到 `usize::MAX`。
        assert_eq!(floor_to_usize(f32::INFINITY), usize::MAX);
    }

    /// `ceil_to_usize`：整数值不 +1，非整数值 +1，负值与 `-∞` → 0。
    ///
    /// **注意一个反直觉的实际行为**：`ceil_to_usize(f32::NAN)` 返回 **1**，
    /// 不是 0。原因是 `NaN <= 0.0` 为 `false`，于是绕过早退分支，
    /// `(NaN as usize)` 饱和为 0，而 `0 as f32 == NaN` 为 `false`，
    /// 最终走 `truncated.saturating_add(1)` 得到 1。此处按实现断言并加注释，
    /// 以便将来若修正该行为时能立刻看到契约变化。
    #[test]
    #[cfg(feature = "programmable")]
    fn ceil_to_usize_handles_integer_fractional_nan_and_infinity() {
        // 整数值不加 1。
        assert_eq!(ceil_to_usize(0.0), 0);
        assert_eq!(ceil_to_usize(1.0), 1);
        assert_eq!(ceil_to_usize(2.0), 2);
        // 非整数值加 1（这正是不能用 `f32 as usize` 的原因）。
        assert_eq!(ceil_to_usize(0.5), 1);
        assert_eq!(ceil_to_usize(2.0_f32 + 0.25), 3);
        // 负值 → 0（含 `-0.5`、`-0.0`、`-∞`，都满足 `value <= 0.0`）。
        assert_eq!(ceil_to_usize(-1.0), 0);
        assert_eq!(ceil_to_usize(-0.5), 0);
        assert_eq!(ceil_to_usize(-0.0), 0);
        assert_eq!(ceil_to_usize(f32::NEG_INFINITY), 0);
        // `NaN` → 1（反直觉，见文档注释）。
        assert_eq!(ceil_to_usize(f32::NAN), 1);
        // `+∞`：`(∞ as usize)` 饱和到 usize::MAX，且 `usize::MAX as f32 == ∞`
        // 成立，故整数分支返回 usize::MAX 而不再 +1。
        assert_eq!(ceil_to_usize(f32::INFINITY), usize::MAX);
    }

    /// 两个取整助手的分工契约：非负有限值上 `ceil - floor ∈ {0, 1}`。
    #[test]
    #[cfg(feature = "programmable")]
    fn floor_and_ceil_bracket_non_negative_finite_values() {
        for v in [0.0f32, 0.25, 1.0, 1.5, 2.0, 3.75, 7.0, 7.999] {
            let f = floor_to_usize(v);
            let c = ceil_to_usize(v);
            assert!(c >= f, "ceil({v}) = {c} 不应小于 floor({v}) = {f}");
            assert!(c - f <= 1, "ceil({v}) = {c} 与 floor({v}) = {f} 相差超过 1");
        }
    }

    /// 视口变换的端点语义：`0.0` 是边缘本身，`1.0` 是外侧一像素的边缘。
    #[test]
    #[cfg(feature = "programmable")]
    fn normalized_to_screen_maps_to_viewport_edges() {
        let viewport = Viewport::new(10, 20, 100, 50);
        assert_eq!(normalized_to_screen([0.0, 0.0], viewport), [10.0, 20.0]);
        assert_eq!(normalized_to_screen([1.0, 1.0], viewport), [110.0, 70.0]);
    }

    /// 视口变换的中点与偏移：`0.5` 是几何中心，跨度乘**全宽**而非 `width - 1`。
    #[test]
    #[cfg(feature = "programmable")]
    fn normalized_to_screen_maps_midpoint_to_geometry_center() {
        let viewport = Viewport::new(10, 20, 100, 50);
        // 10 + 0.5 × 100 = 60；20 + 0.5 × 50 = 45。
        assert_eq!(normalized_to_screen([0.5, 0.5], viewport), [60.0, 45.0]);
        // 任意偏移按同一仿射关系叠加。
        assert_eq!(normalized_to_screen([0.25, 0.0], viewport), [35.0, 20.0]);
        assert_eq!(normalized_to_screen([0.0, 0.25], viewport), [10.0, 32.5]);
    }

    /// 退化视口（宽或高为 0）按 1 处理，避免除零与负跨度。
    #[test]
    #[cfg(feature = "programmable")]
    fn normalized_to_screen_falls_back_to_one_for_degenerate_viewport() {
        // 宽 0：跨度按 1，`1.0` 落在 x + 1（仍是半开区间的开边界）。
        let zero_w = Viewport::new(5, 5, 0, 10);
        assert_eq!(normalized_to_screen([0.0, 0.0], zero_w), [5.0, 5.0]);
        assert_eq!(normalized_to_screen([1.0, 0.0], zero_w), [6.0, 5.0]);

        // 高 0：同理。
        let zero_h = Viewport::new(5, 5, 10, 0);
        assert_eq!(normalized_to_screen([0.0, 1.0], zero_h), [5.0, 6.0]);

        // 两者皆 0。
        let zero_both = Viewport::new(0, 0, 0, 0);
        assert_eq!(normalized_to_screen([1.0, 1.0], zero_both), [1.0, 1.0]);
    }

    /// 视口变换的线性契约：`f(a) - f(0) == (f(b) - f(a)) * k`，用于确认它是仿射映射
    /// 而非其他非线性变换。
    #[test]
    #[cfg(feature = "programmable")]
    fn normalized_to_screen_is_affine_in_span() {
        let viewport = Viewport::new(7, 11, 64, 32);
        let origin = normalized_to_screen([0.0, 0.0], viewport);
        let quarter = normalized_to_screen([0.25, 0.25], viewport);
        let half = normalized_to_screen([0.5, 0.5], viewport);
        assert_eq!(origin, [7.0, 11.0]);
        assert_eq!(quarter, [7.0 + 16.0, 11.0 + 8.0]);
        // 中点正好是原点与 1.0 端点的均值（线性）。
        let full = normalized_to_screen([1.0, 1.0], viewport);
        for axis in 0..2 {
            assert_eq!(
                half[axis],
                (origin[axis] + full[axis]) / 2.0,
                "轴 {axis} 上中点应是两端均值"
            );
        }
    }

    #[test]
    fn pattern_fill_works() {
        let mut row = [0u8; 16];
        pattern_fill(&mut row, &[1, 2, 3]);
        assert_eq!(row, [1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1]);
    }
}
