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
    use super::*;

    /// 视口变换的端点语义：`0.0` 是边缘本身，`1.0` 是外侧一像素的边缘。
    #[test]
    #[cfg(feature = "programmable")]
    fn normalized_to_screen_maps_to_viewport_edges() {
        let viewport = Viewport::new(10, 20, 100, 50);
        assert_eq!(normalized_to_screen([0.0, 0.0], viewport), [10.0, 20.0]);
        assert_eq!(normalized_to_screen([1.0, 1.0], viewport), [110.0, 70.0]);
    }

    #[test]
    fn pattern_fill_works() {
        let mut row = [0u8; 16];
        pattern_fill(&mut row, &[1, 2, 3]);
        assert_eq!(row, [1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1]);
    }
}
