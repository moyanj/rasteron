use core::marker::PhantomData;

use crate::programmable::{FragmentInput, Interpolate, Vertex2D, VertexShader};

/// 直接透传 [`Vertex2D`] 的内建顶点着色器。
///
/// 不做任何坐标变换，仅把顶点属性原样作为 varying 输出。
#[derive(Debug, Clone, Copy, Default)]
pub struct Passthrough2D<V> {
    /// 仅用于在类型上绑定 varying 类型 `V`，无运行时数据。
    _marker: PhantomData<V>,
}

impl<V> Passthrough2D<V> {
    /// 创建透传顶点着色器。
    pub const fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<V> VertexShader for Passthrough2D<V>
where
    V: Interpolate + Copy,
{
    type Input = Vertex2D<V>;
    type Varying = V;

    /// 把 [`Vertex2D`] 原样转换为 [`FragmentInput`]。
    fn shade_vertex(&self, input: &Self::Input) -> FragmentInput<Self::Varying> {
        (*input).into()
    }
}
