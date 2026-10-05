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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn passthrough_forwards_position_and_varying_verbatim() {
        let shader = Passthrough2D::<Color>::new();
        let cases = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: Color::new(0, 0, 0),
            },
            Vertex2D {
                position: [1.0, 1.0],
                varying: Color::new(255, 255, 255),
            },
            // 非整数的 normalized 坐标必须原样透传，不做任何取整或变换。
            Vertex2D {
                position: [0.123_456, 0.987_654],
                varying: Color::with_alpha(1, 2, 3, 4),
            },
            // 视口外的负坐标同样原样透传（裁剪是光栅化阶段的职责）。
            Vertex2D {
                position: [-0.5, 2.5],
                varying: Color::new(7, 8, 9),
            },
        ];
        for input in cases {
            let out = shader.shade_vertex(&input);
            assert_eq!(out.position, input.position, "position 被变换了");
            assert_eq!(out.varying, input.varying, "varying 被变换了");
        }
    }

    #[test]
    fn default_matches_new() {
        // `#[derive(Default)]` 生成的 `impl` 带有 `V: Default` 约束，
        // 故只有当 varying 类型实现 `Default` 时才可用。
        // `Color` 没有实现 `Default`，这里改用 `f32`（实现 Default）验证。
        let via_new = Passthrough2D::<f32>::new();
        let via_default = Passthrough2D::<f32>::default();
        let input = Vertex2D {
            position: [0.25, 0.75],
            varying: 42.0f32,
        };
        assert_eq!(
            via_new.shade_vertex(&input),
            via_default.shade_vertex(&input),
            "Default 与 new 的着色结果应完全一致"
        );

        // 对没有 `Default` 的 varying 类型，`default()` 不可用——
        // 这是派生实现的固有约束，此处记录以免误用。
        // （无法用编译期断言表达，只能靠类型检查本身保证。）
    }

    #[test]
    fn passthrough_is_copy_and_reusable() {
        // 无运行时状态，故可自由复用与按值复制。
        let shader = Passthrough2D::<Color>::new();
        let copy = shader; // Copy：原值仍可用
        let input = Vertex2D {
            position: [0.5, 0.5],
            varying: Color::new(5, 5, 5),
        };
        assert_eq!(shader.shade_vertex(&input), copy.shade_vertex(&input));
    }
}
