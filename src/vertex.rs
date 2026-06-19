use crate::color;

/// 用于可编程管线的着色顶点（需要 `feature = "programmable"`）。
///
/// 目前为占位类型，供未来着色器顶点抽象使用。
pub struct Vertex {
    /// 归一化 X 坐标。
    pub x: f32,
    /// 归一化 Y 坐标。
    pub y: f32,
    /// 顶点颜色。
    pub color: color::Color,
}
