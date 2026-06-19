use crate::Pixel;

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

    let (mut x1, mut y1) = (
        saturating_usize_to_i64(p1.x),
        saturating_usize_to_i64(p1.y),
    );
    let (mut x2, mut y2) = (
        saturating_usize_to_i64(p2.x),
        saturating_usize_to_i64(p2.y),
    );
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

        // 找到线段与边界的交点
        if (code_out & TOP) != 0 {
            // y = height - 1  (上边界，假设 y 增大向下)
            x = x1 + (x2 - x1) * (height as i64 - 1 - y1) / (y2 - y1);
            y = height as i64 - 1;
        } else if (code_out & BOTTOM) != 0 {
            // y = 0
            x = x1 + (x2 - x1) * (0 - y1) / (y2 - y1);
            y = 0;
        } else if (code_out & RIGHT) != 0 {
            // x = width - 1
            y = y1 + (y2 - y1) * (width as i64 - 1 - x1) / (x2 - x1);
            x = width as i64 - 1;
        } else if (code_out & LEFT) != 0 {
            // x = 0
            y = y1 + (y2 - y1) * (0 - x1) / (x2 - x1);
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

/// 饱和转换：从 `usize` 转换为 `i64`。
///
/// 超过 [`i64::MAX`] 的值会被截断为 [`i64::MAX`]。
/// 内部用于在插值运算中避免负数。
#[inline(always)]
pub(crate) fn saturating_usize_to_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}
