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

#[cfg(test)]
mod tests {
    use super::*;

    /// 角度网格的步长：整圈切成 360 份，便于在均匀网格上逐点对照浮点参考。
    const GRID_STEP: u32 = 1 << 24;

    // ————————————————————————————— 构造与往返 —————————————————————————————

    #[test]
    fn raw_round_trips_identity() {
        assert_eq!(Angle::from_raw(0).raw(), 0);
        assert_eq!(Angle::from_raw(1).raw(), 1);
        assert_eq!(Angle::from_raw(u32::MAX).raw(), u32::MAX);
        // 全圈扫描一圈：所有圈值都满足 from_raw(raw) == raw。
        let mut raw = 0u32;
        for _ in 0..360 {
            assert_eq!(Angle::from_raw(raw).raw(), raw);
            raw = raw.wrapping_add(GRID_STEP);
        }
    }

    #[test]
    fn quadrant_constants_match_degrees() {
        assert_eq!(Angle::ZERO.raw(), 0);
        assert_eq!(Angle::ZERO.degrees(), 0);
        assert_eq!(Angle::RIGHT_DOWN.raw(), 1 << 30);
        assert_eq!(Angle::RIGHT_DOWN.degrees(), 90);
        assert_eq!(Angle::DOWN_LEFT.raw(), 1 << 31);
        assert_eq!(Angle::DOWN_LEFT.degrees(), 180);
        assert_eq!(Angle::LEFT_UP.raw(), 3 << 30);
        assert_eq!(Angle::LEFT_UP.degrees(), 270);
        // 象限常量与 from_degrees 独立构造的结果必须一致。
        assert_eq!(Angle::from_degrees(90), Angle::RIGHT_DOWN);
        assert_eq!(Angle::from_degrees(180), Angle::DOWN_LEFT);
        assert_eq!(Angle::from_degrees(270), Angle::LEFT_UP);
    }

    #[test]
    fn degrees_round_trips_exactly_for_integer_degrees() {
        for deg in 0..360u32 {
            assert_eq!(Angle::from_degrees(deg).degrees(), deg, "整数度往返失败: {deg}");
        }
    }

    #[test]
    fn degrees_wraps_beyond_full_circle() {
        assert_eq!(Angle::from_degrees(360), Angle::ZERO);
        assert_eq!(Angle::from_degrees(360).degrees(), 0);
        assert_eq!(Angle::from_degrees(720), Angle::ZERO);
        assert_eq!(Angle::from_degrees(359).degrees(), 359);
        assert_eq!(Angle::from_degrees(361).degrees(), 1);
        // 多个整圈不改变结果。
        for k in 0..8u32 {
            assert_eq!(Angle::from_degrees(360 * k + 45), Angle::from_degrees(45));
        }
    }

    #[test]
    fn millidegrees_resolves_sub_degree_and_wraps() {
        assert_eq!(Angle::from_millidegrees(0), Angle::ZERO);
        assert_eq!(Angle::from_millidegrees(90_000), Angle::RIGHT_DOWN);
        assert_eq!(Angle::from_millidegrees(180_000), Angle::DOWN_LEFT);
        assert_eq!(Angle::from_millidegrees(270_000), Angle::LEFT_UP);
        assert_eq!(Angle::from_millidegrees(360_000), Angle::ZERO);
        assert_eq!(Angle::from_millidegrees(720_000), Angle::ZERO);
    }

    #[test]
    fn millidegrees_is_finer_than_degrees_and_truncates_consistently() {
        // 毫度构造与度数构造是两套独立的四舍五入（前者按毫度换算圈值，后者把
        // 圈值换回度），故整千毫度**不必**等于 from_degrees：两次舍入叠加后
        // 在半数度数上会差 1 圈值。这里断言的是「毫度至少不粗于度」——
        // 换算回来的度数与真值最多差 1 度，且方向可预期。
        // 换算回来的度数与真值最多差 1 度。
        for deg in 0..360u32 {
            let recovered = Angle::from_millidegrees(deg * 1000).degrees();
            // 四舍五入后要么落在本度，要么被推到相邻的度（二者都合法）。
            let diff = if recovered >= deg {
                recovered - deg
            } else {
                deg - recovered
            };
            assert!(diff <= 1, "毫度 {deg}° 换算回 {recovered}°，偏差过大");
        }

        // 精确值：对 90/180/270 这类 2^32 可整除的度数，毫度与度数必须完全一致。
        for deg in [0u32, 90, 180, 270] {
            assert_eq!(
                Angle::from_millidegrees(deg * 1000),
                Angle::from_degrees(deg),
                "整除度数 {deg} 的毫度换算应与度数构造一致"
            );
        }

        // 毫度的亚度精度：相邻毫度必须产生不同的圈值（不被吞掉）。
        let a = Angle::from_millidegrees(45_000).raw();
        let b = Angle::from_millidegrees(45_001).raw();
        assert!(b > a, "毫度分辨率丢失：45000 与 45001 毫度圈值相同");

        // degrees() 在毫度边界上的四舍五入行为（实测值，来自换算定义）：
        // 0.5° 仍判 0°，0.501° 起进位到 1°。
        assert_eq!(Angle::from_millidegrees(0).degrees(), 0);
        assert_eq!(Angle::from_millidegrees(500).degrees(), 0);
        assert_eq!(Angle::from_millidegrees(501).degrees(), 1);
        assert_eq!(Angle::from_millidegrees(1999).degrees(), 2);
        assert_eq!(Angle::from_millidegrees(45_000).degrees(), 45);
        assert_eq!(Angle::from_millidegrees(359_999).degrees(), 0);
    }

    // ————————————————————————————— 回绕与算术 —————————————————————————————

    #[test]
    fn subtraction_wraps_through_zero() {
        assert_eq!(Angle::ZERO - Angle::from_raw(1), Angle::from_raw(u32::MAX));
        assert_eq!(
            Angle::from_raw(1) - Angle::from_raw(2),
            Angle::from_raw(u32::MAX)
        );
        // 减法跨整圈：270° - 180° = 90°
        assert_eq!(Angle::LEFT_UP - Angle::DOWN_LEFT, Angle::RIGHT_DOWN);
        // 减出负跨度：0° - 90° = 270°
        assert_eq!(Angle::ZERO - Angle::RIGHT_DOWN, Angle::LEFT_UP);
    }

    #[test]
    fn addition_wraps_through_full_circle() {
        assert_eq!(Angle::LEFT_UP + Angle::RIGHT_DOWN, Angle::ZERO);
        assert_eq!(Angle::DOWN_LEFT + Angle::RIGHT_DOWN, Angle::LEFT_UP);
        assert_eq!(Angle::from_raw(u32::MAX) + Angle::from_raw(1), Angle::ZERO);
    }

    #[test]
    fn assign_operators_agree_with_binary_operators() {
        let start = Angle::from_degrees(300);

        let mut a = start;
        a += Angle::from_degrees(100);
        assert_eq!(a, start + Angle::from_degrees(100));
        assert_eq!(a, Angle::from_degrees(40));

        let mut b = start;
        b -= Angle::from_degrees(100);
        assert_eq!(b, start - Angle::from_degrees(100));
        assert_eq!(b, Angle::from_degrees(200));

        // 回绕路径上同样一致。
        let mut c = Angle::ZERO;
        c -= Angle::from_raw(1);
        assert_eq!(c, Angle::ZERO - Angle::from_raw(1));
    }

    #[test]
    fn angle_traits_are_derived_and_orderable() {
        assert_eq!(Angle::ZERO, Angle::default());
        assert_eq!(Angle::ZERO, Angle::from_raw(0));
        // Ord 按圈值：加减回绕不影响比较语义。
        assert!(Angle::ZERO < Angle::RIGHT_DOWN);
        assert!(Angle::RIGHT_DOWN < Angle::DOWN_LEFT);
        assert!(Angle::DOWN_LEFT < Angle::LEFT_UP);
        assert!(Angle::from_raw(u32::MAX) > Angle::LEFT_UP);
    }

    // ————————————————————————————— 三角函数 —————————————————————————————

    #[test]
    fn sin_cos_matches_exact_baseline_angles() {
        // Y 轴向下：sin 分量对应 +y，cos 分量对应 +x。
        // SIN_SCALE 是 i32 常量，比较前显式提升到 sin_cos 的 i64 返回类型。
        let scale = SIN_SCALE as i64;
        assert_eq!(Angle::ZERO.sin_cos(), (0, scale));
        assert_eq!(Angle::RIGHT_DOWN.sin_cos(), (scale, 0));
        assert_eq!(Angle::DOWN_LEFT.sin_cos(), (0, -scale));
        assert_eq!(Angle::LEFT_UP.sin_cos(), (-scale, 0));
        // sin / cos 分量与 sin_cos 一致。
        assert_eq!(Angle::ZERO.sin(), 0);
        assert_eq!(Angle::ZERO.cos(), scale);
        assert_eq!(Angle::RIGHT_DOWN.sin(), scale);
        assert_eq!(Angle::RIGHT_DOWN.cos(), 0);
    }

    #[test]
    fn unit_circle_identity_holds_within_fixed_bound() {
        // (sin/SIN_SCALE)² + (cos/SIN_SCALE)² ≈ 1。
        //
        // 误差界由波表精度推导而非拍脑袋：实测 sin/cos 的单分量最大误差为
        // ε = 1 个刻度（稠密网格上亦为 1）。令 s = S + δs、c = S + δc，则
        // s² + c² - S² = 2S(δs + δc) + δs² + δc²，
        // 取 |δs|, |δc| <= ε 得上界 4·S·ε + 2ε² = 4·32767 + 2 = 131070。
        // 该值既覆盖推导上界，又不会宽到放过真正的回归——例如某象限把 sin/cos
        // 写反，偏差量级是 S² ≈ 1e9，比容差大四个数量级。
        const TOLERANCE: i64 = 4 * SIN_SCALE as i64 + 2;
        let scale = SIN_SCALE as i64;
        let mut raw = 0u32;
        for _ in 0..1024 {
            let (sin, cos) = Angle::from_raw(raw).sin_cos();
            let error = (sin * sin + cos * cos - scale * scale).abs();
            assert!(
                error <= TOLERANCE,
                "单位圆恒等式越界于 raw={raw}: sin={sin} cos={cos} 偏差={error}"
            );
            raw = raw.wrapping_add(GRID_STEP);
        }
    }

    #[test]
    fn sin_is_odd_about_zero() {
        // 取负即圈值回绕：`ZERO - angle` 的 raw 为 `0u32.wrapping_sub(raw)`。
        // `Angle` 只提供加减运算，没有实现 `Neg`。
        let mut raw = 0u32;
        for _ in 0..360 {
            let angle = Angle::from_raw(raw);
            let neg = Angle::ZERO - angle;
            assert_eq!(neg.raw(), 0u32.wrapping_sub(raw));
            assert_eq!(
                neg.sin(),
                -angle.sin(),
                "sin 反对称失败于 raw={raw}"
            );
            raw = raw.wrapping_add(GRID_STEP);
        }
    }

    #[test]
    fn sin_shifted_by_quarter_equals_cos() {
        // sin(θ + 90°) == cos(θ)：四分之一波表的象限折叠应给出该恒等式。
        let mut raw = 0u32;
        for _ in 0..360 {
            let angle = Angle::from_raw(raw);
            assert_eq!(
                (angle + Angle::RIGHT_DOWN).sin(),
                angle.cos(),
                "sin(θ+90°)==cos(θ) 失败于 raw={raw}"
            );
            raw = raw.wrapping_add(GRID_STEP);
        }
    }

    #[test]
    fn sin_cos_matches_float_reference_within_tolerance() {
        // 与浮点参考实现在 1° 网格上逐点对照。定点最大误差约 1.6/SIN_SCALE，
        // 容差按「度数 × 舍入误差」放大到 4 个刻度，足以吸收波表插值抖动。
        const TOLERANCE: i64 = 4;
        for deg in 0..360u32 {
            let angle = Angle::from_degrees(deg);
            let (sin, cos) = angle.sin_cos();
            let radians = core::f32::consts::PI * (deg as f32) / 180.0;
            let expected_sin = (radians.sin() * SIN_SCALE as f32).round() as i64;
            let expected_cos = (radians.cos() * SIN_SCALE as f32).round() as i64;
            assert!(
                (sin - expected_sin).abs() <= TOLERANCE,
                "sin({deg}°) = {sin}，浮点参考 {expected_sin}"
            );
            assert!(
                (cos - expected_cos).abs() <= TOLERANCE,
                "cos({deg}°) = {cos}，浮点参考 {expected_cos}"
            );
        }
    }

    // ————————————————————————————— 波表 —————————————————————————————

    #[test]
    fn lookup_table_endpoints_are_exact_and_in_bounds() {
        // t == 0 → 表首项 0；t == QUARTER → 末项 SIN_SCALE（不越界读 idx+1）。
        assert_eq!(lookup(0), 0);
        assert_eq!(lookup(QUARTER), SIN_SCALE);
        // QUARTER 之上同样短路到末项，边界不越界。
        assert_eq!(lookup(QUARTER + 1), SIN_SCALE);
        assert_eq!(lookup(u32::MAX), SIN_SCALE);
    }

    #[test]
    fn first_quadrant_table_is_monotonic_non_decreasing() {
        let mut previous = i32::MIN;
        let mut raw = 0u32;
        while raw <= QUARTER {
            let value = lookup(raw);
            assert!(
                value >= previous,
                "波表在 raw={raw} 处下降：{value} < {previous}"
            );
            previous = value;
            raw += 1 << 18;
        }
    }

    #[test]
    fn sin_cos_never_reads_out_of_table_at_quadrant_endpoints() {
        // 象限端点正是 lookup 的越界风险点：逐一确认返回值仍是精确基准角。
        // SIN_SCALE 是 i32 常量，比较前显式提升到 sin_cos 的 i64 返回类型。
        let scale = SIN_SCALE as i64;
        let expectations = [
            (0u32, (0i64, scale)),
            (QUARTER, (scale, 0)),
            (HALF, (0, -scale)),
            (3 * QUARTER, (-scale, 0)),
        ];
        for (base, expected) in expectations {
            assert_eq!(Angle::from_raw(base).sin_cos(), expected, "象限端点 raw={base}");
        }
    }

    // ————————————————————————————— Wedge —————————————————————————————

    #[test]
    fn wedge_contains_the_origin() {
        // 圆心本身没有方向，约定归属扇形内部。
        let wedge = Wedge::new(Angle::from_degrees(30), Angle::from_degrees(60));
        assert!(wedge.contains(0, 0));
        // 整圈楔形同样包含原点。
        assert!(Wedge::new(Angle::ZERO, Angle::ZERO).contains(0, 0));
    }

    #[test]
    fn wedge_is_scale_invariant() {
        // 同一方向上的任意倍数距离必须得到同一判定。
        let wedge = Wedge::new(Angle::from_degrees(30), Angle::from_degrees(60));
        for scale in [1i64, 2, 7, 100, 10_000] {
            assert!(
                wedge.contains(scale, scale),
                "45° 应落在 30°..60° 内，但 scale={scale} 被判为界外"
            );
            assert!(!wedge.contains(-scale, scale), "135° 不应在 30°..60° 内");
            assert!(!wedge.contains(scale, -scale), "-45° 不应在 30°..60° 内");
        }
    }

    #[test]
    fn wedge_crossing_zero_wraps_around() {
        // 350° → 10° 必须跨过 0°，只覆盖靠近正右的窄区。
        let wedge = Wedge::new(Angle::from_degrees(350), Angle::from_degrees(10));
        assert!(!wedge.is_full());
        // 0°（正右）在区间内。
        assert!(wedge.contains(1000, 0));
        // 355°（右上偏右）在区间内：cos(-5°)=0.996, sin(-5°)=-0.087。
        assert!(wedge.contains(996, -87), "355° 应在 350°..10° 内");
        // 5°（右下偏右）在区间内。
        assert!(wedge.contains(996, 87), "5° 应在 350°..10° 内");
        // 340° 与 20° 都在缺口内（跨圈区间的补集）。
        assert!(!wedge.contains(940, -342), "340° 不应在 350°..10° 内");
        assert!(!wedge.contains(940, 342), "20° 不应在 350°..10° 内");
        // 正左（180°）远在缺口内。
        assert!(!wedge.contains(-1000, 0));
    }

    #[test]
    fn wedge_with_equal_endpoints_is_full_circle() {
        assert!(Wedge::new(Angle::ZERO, Angle::ZERO).is_full());
        assert!(Wedge::new(Angle::from_degrees(90), Angle::from_degrees(90)).is_full());
        // 整圈包含任意方向，含负象限。
        let full = Wedge::new(Angle::from_degrees(37), Angle::from_degrees(37));
        assert!(full.is_full());
        for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1), (-3, 7), (5, -9)] {
            assert!(full.contains(dx, dy), "整圈未包含 ({dx}, {dy})");
        }
    }

    #[test]
    fn wedge_includes_start_ray_and_excludes_end_ray_exactly() {
        // 0°..90° 的楔形：起始射线（正右）含，终止射线（正下）含。
        //
        // 注意：与「半开区间」的直觉不同，本库的 `Wedge` 对**两条**边界射线都判定
        // 为界内——`minor_wedge` 的叉积判据是 `>= -容差`，且 `Wedge` 的文档明确
        // 写明「判定包含两条边界射线」。该语义对扇形光栅化是必需的：恰好落在
        // 边界上的像素（如 `(1, 0)` 相对圆心）必须被着色，否则四分之一扇形会
        // 缺掉边界像素。
        let wedge = Wedge::new(Angle::ZERO, Angle::RIGHT_DOWN);
        // 严格位于两条射线之间的方向被包含。
        assert!(wedge.contains(1000, 1000));
        // 两条边界射线本身都包含（这是契约，不是缺陷）。
        assert!(wedge.contains(1000, 0), "起始射线（正右）应被包含");
        assert!(wedge.contains(0, 1000), "终止射线（正下）应被包含");
        // 射线两侧的象限明确判为界外。
        assert!(!wedge.contains(0, -1000), "负 y 方向应在界外");
        assert!(!wedge.contains(-1000, 1000), "负 x 正 y 应在界外");
        assert!(!wedge.contains(-1000, 0), "负 x 方向应在界外");

        // 对称的四分之一楔形验证同一契约：270°..360°（右上象限）含正右与正上。
        let up_right = Wedge::new(Angle::LEFT_UP, Angle::ZERO);
        assert!(up_right.contains(1000, 0), "起始射线（正右）应被包含");
        assert!(up_right.contains(0, -1000), "终止射线（正上）应被包含");
        assert!(!up_right.contains(0, 1000), "正下应在界外");
        assert!(!up_right.contains(-1000, 0), "正左应在界外");
    }

    #[test]
    fn quarter_wedges_cover_their_own_screen_quadrant() {
        // Y 轴向下：0°→+x(右)、90°→+y(下)。四个四分之一楔形必须各守一个象限。
        let right_down = Wedge::new(Angle::ZERO, Angle::RIGHT_DOWN);
        let down_left = Wedge::new(Angle::RIGHT_DOWN, Angle::DOWN_LEFT);
        let left_up = Wedge::new(Angle::DOWN_LEFT, Angle::LEFT_UP);
        let up_right = Wedge::new(Angle::LEFT_UP, Angle::ZERO);

        // 右下象限 (dx>0, dy>0)
        assert!(right_down.contains(500, 500));
        assert!(!right_down.contains(-500, 500));
        assert!(!right_down.contains(500, -500));
        // 左下象限 (dx<0, dy>0)
        assert!(down_left.contains(-500, 500));
        assert!(!down_left.contains(500, 500));
        // 左上象限 (dx<0, dy<0)
        assert!(left_up.contains(-500, -500));
        assert!(!left_up.contains(500, -500));
        // 右上象限 (dx>0, dy<0)
        assert!(up_right.contains(500, -500));
        assert!(!up_right.contains(-500, -500));

        // 四个楔形的并集恰好覆盖四个象限的斜向。
        let diagonals = [(500i64, 500i64), (-500, 500), (-500, -500), (500, -500)];
        for (dx, dy) in diagonals {
            let hits = [
                right_down.contains(dx, dy),
                down_left.contains(dx, dy),
                left_up.contains(dx, dy),
                up_right.contains(dx, dy),
            ];
            assert_eq!(hits.iter().filter(|hit| **hit).count(), 1, "象限 ({dx}, {dy}) 命中数异常");
        }
    }

    #[test]
    fn wedge_tolerance_does_not_widen_with_span() {
        // 容差随像素向量长度线性增长，但不随角度跨度变宽。
        // 对同一方向（45°），把跨度从 1° 放大到 90°，判定结果都不应改变。
        for span in [1u32, 5, 30, 89, 90] {
            let start = Angle::from_degrees(30);
            let end = Angle::from_degrees(30 + span);
            let wedge = Wedge::new(start, end);
            // 45° 恒在 30°..(30+span) 内（span >= 15 时）。
            if span >= 15 {
                for scale in [1i64, 50, 5_000] {
                    assert!(
                        wedge.contains(scale, scale),
                        "45° 应在 30°..{}° 内，scale={scale}",
                        30 + span
                    );
                }
            }
            // 远离边界的方向恒在界外。
            for (dx, dy) in [(-1000i64, 1000i64), (-1000, -1000), (1000, -1000)] {
                assert!(
                    !wedge.contains(dx, dy),
                    "反向象限误判为界内，跨度 {span}°"
                );
            }
        }
    }

    #[test]
    fn wedge_matches_float_atan2_reference_on_angle_grid() {
        // 与浮点 atan2 参考实现逐点对照。
        //
        // 判定规则：`Wedge` 是半开区间 [start, end)，把方向向量换算成
        // `[0, 360)` 内的角度（屏幕坐标系，Y 向下故用 `atan2(dy, dx)`），
        // 落在区间内即期望 `contains` 为真。
        //
        // 定点三角函数有约 1.6/SIN_SCALE 的抖动，方向向量的量化也会引入
        // 约 0.03° 的角误差，故仅在边界 ±0.1° 的窄带内允许判定差异——
        // 带外出现差异说明是真实的语义错误。
        const TOLERANCE_DEG: f32 = 0.1;

        /// 把屏幕方向向量换算为 `[0, 360)` 内的角度（度）。
        fn reference_degrees(dx: i64, dy: i64) -> f32 {
            let radians = (dy as f32).atan2(dx as f32);
            let degrees = radians * 180.0 / core::f32::consts::PI;
            if degrees < 0.0 { degrees + 360.0 } else { degrees }
        }

        for span_deg in [10u32, 45, 90, 180, 270] {
            let start_deg = 15u32;
            let end_deg = start_deg + span_deg;
            let wedge = Wedge::new(Angle::from_degrees(start_deg), Angle::from_degrees(end_deg));

            for probe in 0..360u32 {
                let radians = core::f32::consts::PI * (probe as f32) / 180.0;
                // 用与 probe 度一致的整数方向向量，与 Wedge::contains 的入参类型对齐。
                let dx = (radians.cos() * 4096.0) as i64;
                let dy = (radians.sin() * 4096.0) as i64;
                let actual = wedge.contains(dx, dy);

                let angle = reference_degrees(dx, dy);
                let expected = angle >= start_deg as f32 && angle < end_deg as f32;

                if actual != expected {
                    // 只在两条边界射线的容差带内放行。
                    let near_start = (angle - start_deg as f32).abs() <= TOLERANCE_DEG;
                    let near_end = (angle - end_deg as f32).abs() <= TOLERANCE_DEG;
                    assert!(
                        near_start || near_end,
                        "跨度 {span_deg}° 下 {probe}°（实际 {angle:.3}°）判定为 {actual}，\
                         浮点参考为 {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn minor_wedge_rejects_points_outside_a_small_span() {
        // 极窄跨度：1° 楔形只含极靠近 0° 的方向。
        let wedge = Wedge::new(Angle::from_degrees(44), Angle::from_degrees(46));
        assert!(wedge.contains(4096, 4096));   // 45°
        assert!(!wedge.contains(0, 4096));      // 90°
        assert!(!wedge.contains(4096, 0));      // 0°
    }
}
