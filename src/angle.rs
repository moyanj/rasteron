//! 定点角度类型与整数三角函数。
//!
//! 为 [`Renderer`](crate::Renderer) 的圆弧、扇形图元提供无浮点的角度表示，
//! 保持「关闭 `programmable` feature 后几何计算全整数」的约定。
//!
//! # 表示格式
//!
//! [`Angle`] 是一个**圈值**（turn）：整圈 360° 恰为 `2^32`。
//! 之所以不用「度 × 某系数」而不用「毫度」，是因为圈值天然支持三角函数的
//! 象限折叠（`>> 30` 即得象限号），且加减自动环绕，不需要任何取模分支。
//!
//! # 坐标系
//!
//! 与全库一致：**Y 轴向下**，`(0, 0)` 在左上角。因此数学上的逆时针
//! （counter-clockwise）在屏幕上表现为顺时针。本模块的 [`Angle::sin_cos`]
//! 返回的 `cos` 分量对应屏幕 `+x` 方向、`sin` 分量对应屏幕 `+y` 方向，
//! 即角度按屏幕坐标增大，不要与数学惯例混用。
//!
//! # 精度
//!
//! 正弦用 257 项四分之一波表 + 线性插值实现（见 [`Angle::sin_cos`]）。
//! 实测最大误差约 `1.6 / 32767`（约 0.005%），折算成半径 `r` 处的
//! 方向位置误差小于 `0.0001 × r` 像素——远小于一个像素，对像素级
//! 的角度判定绰绰有余。`sin(0°)` / `sin(90°)` 等基准角为精确值。

use core::ops::{Add, AddAssign, Sub, SubAssign};

/// 定点正弦的缩放因子：`SIN_SCALE = 2^15 - 1`，恰好吃满 `i16` 正半轴。
///
/// [`Angle::sin_cos`] 与 [`Angle::sin`] / [`Angle::cos`] 的返回值都以此为分母：
/// 要还原成 `[-1, 1]` 的浮点值，请除以本常量。取 `32767` 而非 `32768`，
/// 是为了让 `sin(90°)` 也能装进 `i16`。
pub const SIN_SCALE: i32 = 32767;

/// 象限长度（90°）的圈值。
const QUARTER: u32 = 1 << 30;

/// 半圈（180°）的圈值。
const HALF: u32 = 1 << 31;

/// 四分之一波表的分段数。
///
/// 每象限切成 [`QUARTER`] 等份，故每段为 `1 << 22` 圈值。
const SEGMENTS: u32 = 256;

/// 波表末项偏移补正：把圈值右移 [`SEG_FRAC_BITS`] 位即得表索引。
const SEG_FRAC_BITS: u32 = 22;

/// 象限内查表时的分数部分掩码。
const FRAC_MASK: u32 = (1 << SEG_FRAC_BITS) - 1;

/// `sin(0° .. 90°)` 的定点波表，缩放到 [`SIN_SCALE`]，含末项 `sin(90°)`。
///
/// 由脚本生成（256 段等分 + 四舍五入），共 257 项，`i16` 足以下载。
/// 逐项严格单调不减，保证线性插值的斜率恒为正。
const SIN_TABLE: [i16; SEGMENTS as usize + 1] = [
    0, 201, 402, 603, 804, 1005, 1206, 1407, 1608, 1809, 2009, 2210, 2410, 2611, 2811, 3012, 3212,
    3412, 3612, 3811, 4011, 4210, 4410, 4609, 4808, 5007, 5205, 5404, 5602, 5800, 5998, 6195, 6393,
    6590, 6786, 6983, 7179, 7375, 7571, 7767, 7962, 8157, 8351, 8545, 8739, 8933, 9126, 9319, 9512,
    9704, 9896, 10087, 10278, 10469, 10659, 10849, 11039, 11228, 11417, 11605, 11793, 11980, 12167,
    12353, 12539, 12725, 12910, 13094, 13279, 13462, 13645, 13828, 14010, 14191, 14372, 14553,
    14732, 14912, 15090, 15269, 15446, 15623, 15800, 15976, 16151, 16325, 16499, 16673, 16846,
    17018, 17189, 17360, 17530, 17700, 17869, 18037, 18204, 18371, 18537, 18703, 18868, 19032,
    19195, 19357, 19519, 19680, 19841, 20000, 20159, 20317, 20475, 20631, 20787, 20942, 21096,
    21250, 21403, 21554, 21705, 21856, 22005, 22154, 22301, 22448, 22594, 22739, 22884, 23027,
    23170, 23311, 23452, 23592, 23731, 23870, 24007, 24143, 24279, 24413, 24547, 24680, 24811,
    24942, 25072, 25201, 25329, 25456, 25582, 25708, 25832, 25955, 26077, 26198, 26319, 26438,
    26556, 26674, 26790, 26905, 27019, 27133, 27245, 27356, 27466, 27575, 27683, 27790, 27896,
    28001, 28105, 28208, 28310, 28411, 28510, 28609, 28706, 28803, 28898, 28992, 29085, 29177,
    29268, 29358, 29447, 29534, 29621, 29706, 29791, 29874, 29956, 30037, 30117, 30195, 30273,
    30349, 30424, 30498, 30571, 30643, 30714, 30783, 30852, 30919, 30985, 31050, 31113, 31176,
    31237, 31297, 31356, 31414, 31470, 31526, 31580, 31633, 31685, 31736, 31785, 31833, 31880,
    31926, 31971, 32014, 32057, 32098, 32137, 32176, 32213, 32250, 32285, 32318, 32351, 32382,
    32412, 32441, 32469, 32495, 32521, 32545, 32567, 32589, 32609, 32628, 32646, 32663, 32678,
    32692, 32705, 32717, 32728, 32737, 32745, 32752, 32757, 32761, 32765, 32766, 32767,
];

/// 定点角度，一整圈（360°）为 `2^32` 圈值。
///
/// 角度沿屏幕坐标增大：0° 指向 `+x`（右），90° 指向 `+y`（下），
/// 180° 指向 `-x`（左），270° 指向 `-y`（上）。
///
/// 所有加减都环绕自洽：超过整圈或低于零度会自动回绕，不需要调用方取模。
///
/// # 示例
///
/// ```
/// # use rasteron::angle::Angle;
/// assert_eq!(Angle::from_degrees(90).degrees(), 90);
/// assert_eq!(Angle::from_degrees(360), Angle::ZERO);   // 自动回绕
/// assert_eq!(Angle::from_degrees(45) + Angle::from_degrees(315), Angle::from_degrees(360));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Angle(u32);

impl Angle {
    /// 0°，指向 `+x` 方向（屏幕右）。
    pub const ZERO: Self = Self(0);

    /// 90°，指向 `+y` 方向（屏幕下）。
    pub const RIGHT_DOWN: Self = Self(QUARTER);

    /// 180°，指向 `-x` 方向（屏幕左）。
    pub const DOWN_LEFT: Self = Self(HALF);

    /// 270°，指向 `-y` 方向（屏幕上）。
    pub const LEFT_UP: Self = Self(HALF + QUARTER);

    /// 由原始圈值构造。圈值本身是 `u32`，天然落在 `0 .. 2^32` 内，
    /// 故无需也无法再归一化。
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// 返回原始圈值（`0 ..= 2^32 - 1`）。
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// 由「度」构造（整数度，`deg >= 360` 自动回绕）。
    ///
    /// 需要亚度精度时用 [`Angle::from_raw`] 直接给定点圈值——
    /// 一圈的 `2^32` 个刻度远细于一个像素所需的精度。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::angle::Angle;
    /// assert_eq!(Angle::from_degrees(180), Angle::DOWN_LEFT);
    /// assert_eq!(Angle::from_degrees(720), Angle::ZERO);
    /// ```
    pub const fn from_degrees(deg: u32) -> Self {
        // 先对 360 取模，保证结果落在 `0 .. 2^32`（不做这一步，`deg = 360`
        // 会算出 `2^32`，再 `as u32` 就回绕成 0 而 `deg = 361` 变成 104——
        // 恰好是这里要避免的：模后 `359` 仍是最大合法输入，`359 * 2^32`
        // 约 1.54e12，在 u64 下安全）。
        //
        // 圈值 = `deg * 2^32 / 360`，加 `180` 后整除实现四舍五入。
        // 该式在 u64 下精确，最大偏差小于 0.5 圈值（约 4.1e-8 度）。
        const CIRCLE: u64 = 1 << 32;
        Self(((deg as u64 % 360 * CIRCLE + 180) / 360) as u32)
    }

    /// 由「毫度」构造，用于需要亚度精度的场合（`milli >= 360_000` 自动回绕）。
    ///
    /// 需要任意精度时用 [`Angle::from_raw`] 直接给定点圈值。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::angle::Angle;
    /// assert_eq!(Angle::from_millidegrees(90_000), Angle::RIGHT_DOWN);
    /// ```
    pub const fn from_millidegrees(milli: u32) -> Self {
        // 同 `from_degrees`：先对整圈取模，再按 `milli * 2^32 / 360_000` 换算。
        // `milli % 360_000 <= 359_999`，乘积约 1.54e15，仍在 u64 内。
        const CIRCLE: u64 = 1 << 32;
        Self(((milli as u64 % 360_000 * CIRCLE + 180_000) / 360_000) as u32)
    }

    /// 转为「度」（整数，四舍五入，结果落在 `0 .. 360`）。
    pub const fn degrees(self) -> u32 {
        // 圈值 → 度 = `raw * 360 / 2^32`，加半圈后整除实现四舍五入。
        // `raw * 360` 最大约 1.55e12，在 u64 下安全。
        let v = (self.0 as u64 * 360 + (1 << 31)) >> 32;
        // 只有 `raw` 逼近整圈时 v 才会达到 360，此时回绕成 0，
        // 保持结果落在 `0 .. 360`。
        if v >= 360 { 0 } else { v as u32 }
    }

    /// 把归一化到 `[0, 2^32)` 的圈值拆成 `(象限, 象限内偏移)`。
    #[inline]
    const fn quadrant(self) -> (u32, u32) {
        (self.0 >> 30, self.0 & (QUARTER - 1))
    }

    /// 计算定点 `sin` 与 `cos`，返回放大 [`SIN_SCALE`] 倍的 `(sin, cos)`。
    ///
    /// # 坐标系
    ///
    /// 角度沿屏幕坐标增大（Y 轴向下），故 `sin` 分量对应屏幕 `+y`、
    /// `cos` 分量对应屏幕 `+x`。`sin(0°) == 0`、`sin(90°) == SIN_SCALE`。
    ///
    /// # 精度
    ///
    /// 由 257 项四分之一波表线性插值得到，最大误差约 `1.6 / SIN_SCALE`。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::angle::Angle;
    /// let (sin, cos) = Angle::ZERO.sin_cos();
    /// assert_eq!((sin, cos), (0, 32767));
    /// ```
    #[inline]
    pub const fn sin_cos(self) -> (i64, i64) {
        let (q, r) = self.quadrant();
        match q {
            // 第一象限 [0,90): sin 查表
            0 => (lookup(r) as i64, lookup(QUARTER - r) as i64),
            // 第二象限 [90,180): sin = cos(180-x)，用补角查表
            1 => (lookup(QUARTER - r) as i64, -lookup(r) as i64),
            // 第三象限 [180,270): sin = -sin(x-180)
            2 => (-lookup(r) as i64, -lookup(QUARTER - r) as i64),
            // 第四象限 [270,360): sin = -cos(x-270)
            _ => (-lookup(QUARTER - r) as i64, lookup(r) as i64),
        }
    }

    /// 返回归一化到 `[0, 2^32)` 的 `sin` 分量。
    #[inline]
    pub const fn sin(self) -> i64 {
        self.sin_cos().0
    }

    /// 返回归一化到 `[0, 2^32)` 的 `cos` 分量。
    #[inline]
    pub const fn cos(self) -> i64 {
        self.sin_cos().1
    }
}

/// 对归一化到 `[0, QUARTER]` 的圈值做波表线性插值，返回 [`SIN_SCALE`] 缩放值。
///
/// `t` 若恰为 [`QUARTER`]，直接返回末项 `SIN_SCALE`，避免越界取 `idx + 1`。
#[inline]
const fn lookup(t: u32) -> i32 {
    if t >= QUARTER {
        return SIN_TABLE[SEGMENTS as usize] as i32;
    }
    let idx = (t >> SEG_FRAC_BITS) as usize;
    let frac = (t & FRAC_MASK) as i32;
    let a = SIN_TABLE[idx] as i32;
    let b = SIN_TABLE[idx + 1] as i32;
    a + (((b - a) * frac) >> SEG_FRAC_BITS)
}

/// 起始角到终止角之间的角度区间（扇形 / 圆弧的扫掠范围）。
///
/// 构造时预计算两端与中点的单位向量，使 [`Wedge::contains`] 每次判定
/// 只需 2 次叉积，不重复三角运算。角度沿屏幕坐标增大（Y 轴向下）。
///
/// # 判定算法
///
/// 区间跨度 `<= 180°` 时，点落在区间内当且仅当同时位于两条边界射线的
/// 正确半平面（两个叉积的符号判定）；跨度 `> 180°` 时，把区间从中间
/// 拆成两个 `< 180°` 的子楔形再取**并集**。
///
/// 不能对大跨度直接取补集：恰好落在起点射线上的点位于缺口楔形的
/// 边界上，会被错误排除。
///
/// # 边界与数值容差
///
/// 判定包含两条边界射线。叉积带一个与操作数同阶的容差，用于吸收定点
/// 三角函数在"点恰好落在边界射线上"时的舍入抖动——否则轴向像素
/// （如 `(1, 0)` 相对圆心）会被随机判到界外。容差随像素向量长度线性
/// 增长，不影响远离边界的判定。
///
/// # 示例
///
/// ```
/// # use rasteron::angle::{Angle, Wedge};
/// // 0° 到 90°：屏幕右下象限
/// let w = Wedge::new(Angle::ZERO, Angle::RIGHT_DOWN);
/// assert!(w.contains(10, 10));
/// assert!(!w.contains(-10, 10));
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Wedge {
    /// 区间起点的单位向量（缩放到 [`SIN_SCALE`]）。
    start: (i64, i64),
    /// 区间终点的单位向量。
    end: (i64, i64),
    /// 区间中点的单位向量，仅跨度 `> 180°` 时有效。
    mid: (i64, i64),
    /// 跨度是否大于半圈。大于时用 `start..mid` 与 `mid..end` 的并集判定。
    split: bool,
    /// 是否为整圈。`start == end` 时约定为整圈（而非空区间）。
    full: bool,
}

impl Wedge {
    /// 构造角度区间。`start == end` 表示整圈。
    #[inline]
    pub const fn new(start: Angle, end: Angle) -> Self {
        // `span` 取模得到 `[0, 2^32)` 的正向跨度。
        let span = end.raw().wrapping_sub(start.raw());
        let full = span == 0;
        let split = span > HALF;
        // 跨度 > 半圈时中点取真实中点，跨度 <= 半圈时中点置零（不被读取）。
        let mid_angle = if split {
            Angle::from_raw(start.raw().wrapping_add(span / 2))
        } else {
            Angle::ZERO
        };

        let (s, c) = start.sin_cos();
        let (e, ce) = end.sin_cos();
        let (m, cm) = mid_angle.sin_cos();

        Self {
            start: (c, s),
            end: (ce, e),
            mid: (cm, m),
            split,
            full,
        }
    }

    /// 判断方向向量 `(dx, dy)`（相对圆心的像素偏移）是否落在区间内。
    ///
    /// `dx == 0 && dy == 0`（即圆心本身）判定为**在区间内**——它是任何
    /// 扇形的一部分。
    ///
    /// # 示例
    ///
    /// ```
    /// # use rasteron::angle::{Angle, Wedge};
    /// let w = Wedge::new(Angle::from_degrees(30), Angle::from_degrees(60));
    /// assert!(w.contains(1, 1));
    /// assert!(w.contains(0, 0));
    /// assert!(!w.contains(-1, 0));
    /// ```
    #[inline]
    pub fn contains(&self, dx: i64, dy: i64) -> bool {
        if self.full {
            return true;
        }
        // 圆心（零向量）没有任何方向，归属扇形内部。
        if dx == 0 && dy == 0 {
            return true;
        }
        let p = (dx, dy);
        if self.split {
            // 大跨度：两个小楔形取并集
            minor_wedge(self.start, self.mid, p) || minor_wedge(self.mid, self.end, p)
        } else {
            minor_wedge(self.start, self.end, p)
        }
    }

    /// 区间是否为整圈（`start == end`）。
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.full
    }
}

/// 判断点 `p` 是否落在 `a` 到 `b` 的小楔形内（含边界）。
///
/// **前置条件**：`a` 到 `b` 的跨度 `<= 180°`，否则判定不成立
/// （`Wedge::contains` 通过 `split` 保证该前提）。
#[inline]
fn minor_wedge(a: (i64, i64), b: (i64, i64), p: (i64, i64)) -> bool {
    let (ax, ay) = a;
    let (bx, by) = b;
    let (px, py) = p;

    // cross(a, p) 与 cross(p, b) 均需 >= 0 才落在楔形内。
    let c1 = ax * py - ay * px;
    let c2 = px * by - py * bx;

    // 容差与操作数同阶：`a` 缩放到 SIN_SCALE，`p` 是像素偏移（量级可达半径）。
    // 二者任一为零时容差为零，退化为精确判定。
    let p_len = px.unsigned_abs().max(py.unsigned_abs()) as i64;
    let tol_a = (ax.abs() + ay.abs()).saturating_mul(p_len) >> 15;
    let tol_b = (bx.abs() + by.abs()).saturating_mul(p_len) >> 15;

    c1 >= -tol_a && c2 >= -tol_b
}

impl Add for Angle {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}

impl Sub for Angle {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.wrapping_sub(rhs.0))
    }
}

impl AddAssign for Angle {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_add(rhs.0);
    }
}

impl SubAssign for Angle {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_sub(rhs.0);
    }
}
