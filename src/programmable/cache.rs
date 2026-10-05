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
