//! 顶点缓存：按顶点索引复用顶点着色结果（post-transform cache）。
//!
//! 索引缓冲中同一个顶点通常被多个图元引用。若每次都重新执行顶点着色器，重复的
//! 顶点就被重复计算。本模块的 [`VertexCache`] 按顶点索引缓存着色结果：
//!
//! | 类型 | 存储来源 | 需要 `alloc` | 越界 `store` 的语义 |
//! |------|----------|--------------|---------------------|
//! | [`VertexCache`] | 调用方借出的切片 | 否 | 静默丢弃（降级为部分缓存） |
//!
//! 契约：命中值必须与顶点着色器的输出逐位相同，越界永不 panic，顶点数据变化后
//! 由调用方负责清空。库内没有自持有存储的变体，因此不存在 `alloc` 特性。

use crate::programmable::{FragmentInput, Interpolate, VertexShader};

/// 借用调用方槽位的顶点缓存（零分配）。
///
/// 按**顶点索引**缓存顶点着色器的输出，使索引缓冲中重复引用的顶点只着色一次。
/// 槽位由调用方借出（`&'a mut [Option<FragmentInput<V>>]`），本类型自身不做任何
/// 分配，因此在 `no_std` / 无分配器环境下同样可用。
///
/// # 契约
///
/// * 命中返回值必须与 `shader.shade_vertex(&vertices[index])` 逐位相同——这依赖
///   [`VertexShader::shade_vertex`] 是纯函数（见其「副作用约束」段）。
/// * 越界索引不 panic：[`lookup`](VertexCache::lookup) 返回 `None`，
///   [`store`](VertexCache::store) 静默丢弃。因此「缓存短于顶点缓冲」是合法的
///   降级用法：低索引命中、高索引每次重新着色。
/// * 顶点数据发生变化后必须 [`clear`](VertexCache::clear)（或重建缓存），否则会
///   复用上一批顶点的着色结果。
///
/// # 示例
///
/// ```
/// use rasteron::color::Color;
/// use rasteron::programmable::shader::Passthrough2D;
/// use rasteron::programmable::cache::VertexCache;
/// use rasteron::programmable::{FragmentInput, Vertex2D};
///
/// let vertices = [Vertex2D {
///     position: [0.0, 0.0],
///     varying: Color::new(255, 0, 0),
/// }];
/// let shader = Passthrough2D::<Color>::new();
///
/// let mut slots: [Option<FragmentInput<Color>>; 1] = [None; 1];
/// let mut cache = VertexCache::new(&mut slots);
///
/// // 首次未命中 → 执行顶点着色并回填；第二次直接命中。
/// let shaded = cache.resolve(&shader, 0, &vertices[0]);
/// assert_eq!(cache.lookup(0), Some(shaded));
///
/// // 越界索引即未命中，不 panic。
/// assert_eq!(cache.lookup(1), None);
/// ```
pub struct VertexCache<'a, V: Interpolate> {
    slots: &'a mut [Option<FragmentInput<V>>],
}

impl<'a, V: Interpolate> VertexCache<'a, V> {
    /// 借用一片槽位构造缓存。
    ///
    /// 槽位的初始内容即缓存的初始内容，通常传入全为 `None` 的数组。
    pub fn new(slots: &'a mut [Option<FragmentInput<V>>]) -> Self {
        Self { slots }
    }

    /// 槽位数量，即本缓存能覆盖的最大顶点索引 + 1。
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// 是否没有任何槽位（此时任何索引都不会命中）。
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 查询索引对应的已着色顶点；未命中或索引越界返回 `None`。
    #[inline]
    pub fn lookup(&self, index: usize) -> Option<FragmentInput<V>> {
        self.slots.get(index).copied().flatten()
    }

    /// 记录索引对应的着色结果；索引越界时静默丢弃（不 panic）。
    #[inline]
    pub fn store(&mut self, index: usize, vertex: FragmentInput<V>) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = Some(vertex);
        }
    }

    /// 清空全部条目（槽位数量不变）。
    ///
    /// 顶点数据变化后调用；此后所有索引都回到未命中状态。
    pub fn clear(&mut self) {
        self.slots.fill(None);
    }

    /// 命中则复用，未命中则调用顶点着色器并把结果回填。
    ///
    /// 索引越界时每次都重新着色且永不回填——这是「缓存短于顶点缓冲」时的降级
    /// 语义（部分缓存），不会 panic。
    #[inline]
    pub fn resolve<VS>(&mut self, shader: &VS, index: usize, input: &VS::Input) -> FragmentInput<V>
    where
        VS: VertexShader<Varying = V>,
    {
        if let Some(cached) = self.lookup(index) {
            return cached;
        }
        let shaded = shader.shade_vertex(input);
        self.store(index, shaded);
        shaded
    }
}

impl<'a, V: Interpolate> From<&'a mut [Option<FragmentInput<V>>]> for VertexCache<'a, V> {
    fn from(slots: &'a mut [Option<FragmentInput<V>>]) -> Self {
        Self::new(slots)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec::Vec;

    use super::*;
    use crate::color::Color;
    use crate::programmable::Vertex2D;
    use crate::programmable::shader::Passthrough2D;

    type Shaded = FragmentInput<Color>;

    /// 构造一个测试顶点。
    fn vertex(x: f32, y: f32, color: Color) -> Vertex2D<Color> {
        Vertex2D {
            position: [x, y],
            varying: color,
        }
    }

    /// 记录调用次数的顶点着色器，用于验证「命中不再执行 VS」。
    #[derive(Clone)]
    struct CountingVs {
        calls: core::cell::Cell<usize>,
    }

    impl CountingVs {
        fn new() -> Self {
            Self {
                calls: core::cell::Cell::new(0),
            }
        }
    }

    impl VertexShader for CountingVs {
        type Input = Vertex2D<Color>;
        type Varying = Color;
        fn shade_vertex(&self, input: &Self::Input) -> Shaded {
            self.calls.set(self.calls.get() + 1);
            *input
        }
    }

    // ————————————————————————————— 基本存取 —————————————————————————————

    #[test]
    fn empty_cache_lookups_all_miss() {
        let mut slots: [Option<Shaded>; 0] = [];
        let cache = VertexCache::new(&mut slots);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        // 空缓存的任意索引都未命中，且不 panic。
        for index in [0usize, 1, 7, usize::MAX] {
            assert_eq!(cache.lookup(index), None);
        }
    }

    #[test]
    fn len_is_empty_and_from_impl_agree_with_new() {
        let mut slots: [Option<Shaded>; 4] = [None; 4];
        let mut cache = VertexCache::from(&mut slots[..]);
        assert_eq!(cache.len(), 4);
        assert!(!cache.is_empty());

        // 预填一个槽位后 len / is_empty 不变（槽位数与占用数无关）。
        let v = vertex(0.0, 0.0, Color::new(1, 2, 3));
        cache.store(0, v);
        assert_eq!(cache.len(), 4);
        assert!(!cache.is_empty());
    }

    #[test]
    fn store_then_lookup_round_trips_every_slot() {
        let mut slots: [Option<Shaded>; 5] = [None; 5];
        let vertices = [
            vertex(0.0, 0.0, Color::new(10, 0, 0)),
            vertex(1.0, 0.0, Color::new(0, 10, 0)),
            vertex(0.0, 1.0, Color::new(0, 0, 10)),
            vertex(0.5, 0.5, Color::new(5, 5, 5)),
            vertex(1.0, 1.0, Color::new(255, 255, 255)),
        ];
        {
            let mut cache = VertexCache::new(&mut slots);
            assert_eq!(cache.lookup(0), None, "初始应全部未命中");
            for (index, v) in vertices.iter().enumerate() {
                cache.store(index, *v);
            }
            for (index, v) in vertices.iter().enumerate() {
                assert_eq!(cache.lookup(index), Some(*v), "槽位 {index} 往返失败");
            }
        }
        // 缓存离开作用域后，借出的槽位本身也带着写入结果。
        for (index, v) in vertices.iter().enumerate() {
            assert_eq!(slots[index], Some(*v));
        }
    }

    #[test]
    fn out_of_range_lookup_is_none_and_store_is_dropped() {
        let mut slots: [Option<Shaded>; 2] = [None; 2];
        let mut cache = VertexCache::new(&mut slots);
        let v = vertex(0.0, 0.0, Color::new(1, 2, 3));
        // 越界 store 静默丢弃，不 panic、不写任何内存。
        for index in [2usize, 3, 100, usize::MAX] {
            cache.store(index, v);
        }
        for index in [2usize, 3, 100, usize::MAX] {
            assert_eq!(cache.lookup(index), None, "越界索引 {index} 应未命中");
        }
        // 前两个槽位仍未被越界写污染。
        assert_eq!(slots[0], None);
        assert_eq!(slots[1], None);
    }

    #[test]
    fn clear_resets_all_slots() {
        let mut slots: [Option<Shaded>; 3] = [None; 3];
        let mut cache = VertexCache::new(&mut slots);
        for index in 0..3 {
            cache.store(index, vertex(0.0, 0.0, Color::new(index as u8, 0, 0)));
        }
        assert_eq!(cache.lookup(0), Some(vertex(0.0, 0.0, Color::new(0, 0, 0))));
        cache.clear();
        for index in 0..3 {
            assert_eq!(
                cache.lookup(index),
                None,
                "clear 后槽位 {index} 应回到未命中"
            );
        }
        // clear 不改变槽位数量。
        assert_eq!(cache.len(), 3);
    }

    // ————————————————————————————— resolve —————————————————————————————

    #[test]
    fn resolve_misses_once_then_hits_without_calling_the_shader() {
        let shader = CountingVs::new();
        let v = vertex(0.25, 0.75, Color::new(9, 8, 7));
        let mut slots: [Option<Shaded>; 1] = [None; 1];
        let mut cache = VertexCache::new(&mut slots);

        let first = cache.resolve(&shader, 0, &v);
        assert_eq!(shader.calls.get(), 1, "首次未命中应调用一次顶点着色器");
        assert_eq!(cache.lookup(0), Some(first), "resolve 应把结果回填");

        // 连续多次命中：不再调用顶点着色器。
        for _ in 0..5 {
            let again = cache.resolve(&shader, 0, &v);
            assert_eq!(again, first);
        }
        assert_eq!(shader.calls.get(), 1, "命中时不得重复调用顶点着色器");
    }

    #[test]
    fn short_cache_serves_low_indices_and_reshades_high_ones() {
        // 缓存短于顶点缓冲是合法的降级用法：低索引命中、高索引每次重新着色。
        let shader = CountingVs::new();
        let vertices = [
            vertex(0.0, 0.0, Color::new(1, 0, 0)),
            vertex(0.1, 0.1, Color::new(2, 0, 0)),
            vertex(0.2, 0.2, Color::new(3, 0, 0)),
            vertex(0.3, 0.3, Color::new(4, 0, 0)),
        ];
        // 只有 2 个槽位，索引 2/3 越界。
        let mut slots: [Option<Shaded>; 2] = [None; 2];
        let mut cache = VertexCache::new(&mut slots);

        // 低索引 0：首次着色后命中。
        cache.resolve(&shader, 0, &vertices[0]);
        assert_eq!(shader.calls.get(), 1);
        cache.resolve(&shader, 0, &vertices[0]);
        assert_eq!(shader.calls.get(), 1, "低索引应命中");

        // 高索引 3：每次都重新着色，永不回填。
        cache.resolve(&shader, 3, &vertices[3]);
        cache.resolve(&shader, 3, &vertices[3]);
        cache.resolve(&shader, 3, &vertices[3]);
        assert_eq!(shader.calls.get(), 4, "高索引每次都应重新着色（1 + 3 次）");
        assert_eq!(cache.lookup(3), None, "越界索引永不缓存");
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn resolve_out_of_range_index_never_caches_but_still_returns_a_value() {
        let shader = CountingVs::new();
        let v = vertex(0.5, 0.5, Color::new(7, 7, 7));
        let mut slots: [Option<Shaded>; 1] = [None; 1];
        let mut cache = VertexCache::new(&mut slots);
        for index in [1usize, 9, usize::MAX] {
            let got = cache.resolve(&shader, index, &v);
            // `resolve` 返回 `FragmentInput`（`Passthrough` 语义下等于输入顶点）。
            assert_eq!(got, v, "越界索引仍应返回着色结果（降级为部分缓存）");
        }
        assert_eq!(shader.calls.get(), 3, "越界索引每次都重新着色");
        assert_eq!(slots[0], None, "越界索引不得写进任何槽位");
    }

    #[test]
    fn cached_value_matches_a_fresh_shade_exactly() {
        // 命中值必须与 `shader.shade_vertex` 的输出逐位相同——
        // 这是 `Interpolate::interpolate(a, a, a, ..) == a` 恒等契约的端到端体现。
        let cache_shader = CountingVs::new();
        let fresh_shader = Passthrough2D::<Color>::new();
        let vertices: Vec<Vertex2D<Color>> = (0..6)
            .map(|i| {
                vertex(
                    i as f32 / 6.0,
                    1.0 - i as f32 / 6.0,
                    Color::new(i as u8 * 40, 255 - i as u8 * 40, 7),
                )
            })
            .collect();
        let mut slots: [Option<Shaded>; 6] = [None; 6];
        let mut cache = VertexCache::new(&mut slots);
        for (index, v) in vertices.iter().enumerate() {
            let cached = cache.resolve(&cache_shader, index, v);
            let fresh = fresh_shader.shade_vertex(v);
            assert_eq!(cached, fresh, "索引 {index} 的命中值与重新着色不一致");
        }
    }

    #[test]
    fn stale_entries_persist_until_clear_is_called() {
        // **已知契约**：库不会自动失效。顶点数据变化后，调用方必须 `clear()`
        // 或改传 `None` 缓存，否则会复用上一批顶点的着色结果。
        // 这里把该行为固化为测试，避免将来被误当成缺陷而"顺手修掉"。
        let shader = CountingVs::new();
        let original = vertex(0.0, 0.0, Color::new(255, 0, 0));
        let updated = vertex(0.0, 0.0, Color::new(0, 0, 255));
        let mut slots: [Option<Shaded>; 1] = [None; 1];
        let mut cache = VertexCache::new(&mut slots);

        cache.resolve(&shader, 0, &original);
        // 顶点数据变化，但未 clear：命中旧值。
        let stale = cache.resolve(&shader, 0, &updated);
        assert_eq!(stale, original, "未 clear 时命中的是陈旧值（已知契约）");
        assert_eq!(
            shader.calls.get(),
            1,
            "陈旧命中不触发顶点着色器——这正是必须由调用方 clear 的原因"
        );

        // 调用方 clear 后重新着色，得到新值。
        cache.clear();
        let fresh = cache.resolve(&shader, 0, &updated);
        assert_eq!(fresh, updated, "clear 后应重新着色");
        assert_eq!(shader.calls.get(), 2);
    }

    #[test]
    fn overwrite_updates_an_existing_slot() {
        // `store` 是覆盖语义而非 merge：同一索引可反复写入，命中最新值。
        let shader = CountingVs::new();
        let mut slots: [Option<Shaded>; 1] = [None; 1];
        let mut cache = VertexCache::new(&mut slots);
        for step in 0..4u8 {
            cache.store(0, vertex(step as f32, 0.0, Color::new(step, 0, 0)));
            let hit = cache.resolve(&shader, 0, &vertex(0.0, 0.0, Color::new(0, 0, 0)));
            assert_eq!(hit.varying.r, step, "槽位应反映最新写入的值");
        }
        // 全部为命中，着色器从未被调用。
        assert_eq!(shader.calls.get(), 0);
    }
}
