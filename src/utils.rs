use crate::Pixel;
#[cfg(feature = "simd")]
use crate::simd;

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

/// 用重复模式填充整个切片。
///
/// 对 4bpp 等能整除 16 的格式，模式固定为 16 字节：按 16 字节块**直接重复写入**，
/// 而不是"复制已填部分来翻倍"。后者需要 `O(log n)` 次串行 `memcpy`，
/// 每次的源区域都不同、无法向量化，且总写入量随步数累积；直接块写是
/// 单一 steady-state 循环，流水线友好。
///
/// 3bpp 无法整除 16，退回逐次倍增复制。
#[inline(always)]
pub fn pattern_fill(row: &mut [u8], pattern: &[u8]) {
    let len = row.len();
    let pattern_len = pattern.len();
    if pattern_len == 0 || len <= pattern_len {
        return;
    }

    // 16 字节定长块：直接铺满，无倍增链
    if pattern_len == 16 {
        let mut block = [0u8; 16];
        block.copy_from_slice(&pattern[..16]);
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
