/// 帧缓冲像素网格中的二维坐标。
///
/// `x`（列）和 `y`（行）均从 **0 开始**。`(0, 0)` 为帧缓冲左上角。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pixel {
    /// 列（水平位置）。
    pub x: usize,
    /// 行（垂直位置）。
    pub y: usize,
}

impl Pixel {
    /// 创建新的像素坐标。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::Pixel;
    /// let p = Pixel::new(10, 20);
    /// assert_eq!(p.x, 10);
    /// assert_eq!(p.y, 20);
    /// ```
    pub const fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }

    /// 解构为 `(x, y)` 元组。
    pub const fn to_coord(self) -> (usize, usize) {
        (self.x, self.y)
    }
}

/// 便捷宏：从可能非 `usize` 的字面量创建 [`Pixel`]。
///
/// 参数会自动转换为 `usize`。
///
/// # 示例
///
/// ```
/// # use rasteron::pixel;
/// let p = pixel!(320, 240);
/// assert_eq!(p.x, 320);
/// ```
#[macro_export]
macro_rules! pixel {
    ($x:expr, $y:expr) => {
        $crate::pixel::Pixel::new($x as usize, $y as usize)
    };
}

// 给这玩意加注释的司马
