//! **rasteron** — 一个 `no_std` 的软件光栅化库，适用于嵌入式及通用帧缓冲渲染。
//!
//! # 概述
//!
//! rasteron 提供了轻量级的 [`Renderer`]，支持在原始帧缓冲字节切片上直接绘制
//! 像素、线段、三角形、矩形和圆形。支持多种像素格式、可选的双缓冲、Alpha 混合
//! 以及 Cohen–Sutherland 线段裁剪。
//!
//! 开启 `programmable` feature 后，还会额外暴露一条显式参数式的 2D 可编程管线：
//! 顶点缓冲、索引缓冲、顶点着色器、varying 插值、片元着色器、viewport、scissor
//! 与 `draw_arrays` / `draw_indexed`。
//!
//! # 最低支持 Rust 版本 (MSRV)
//!
//! Rust 1.79+ （需要 `usize::min` 在 const 上下文中使用、插值器中的 `i128` 除法
//! 以及 `#![no_std]` 支持）。
//!
//! # 特性标志
//!
//! | 标志 | 说明 |
//! |------|------|
//! | `double_buffer` | 分配后台缓冲区，调用 [`Renderer::swap_buffers`] 交换到前台。 |
//! | `programmable`  | 启用完整的 2D 可编程管线。 |
//!
//! # 可编程管线示例
//!
//! ```ignore
//! use rasteron::Renderer;
//! use rasteron::color::Color;
//! use rasteron::framebuffer::{FramebufferInfo, PixelFormat};
//! use rasteron::programmable::{
//!     FragmentOutput, FragmentShader, Passthrough2D, Pipeline, PipelineState, PrimitiveType,
//!     Vertex2D, Viewport,
//! };
//!
//! struct SolidFs;
//!
//! impl FragmentShader for SolidFs {
//!     type Varying = Color;
//!
//!     fn shade_fragment(&self, varying: &Self::Varying) -> FragmentOutput {
//!         FragmentOutput::Color(*varying)
//!     }
//! }
//!
//! let info = FramebufferInfo::new(128, 128, 128 * 4, PixelFormat::Rgba8888);
//! let mut fb = vec![0u8; info.buffer_size()];
//! let mut renderer = Renderer::new(info, &mut fb);
//!
//! let vertices = [
//!     Vertex2D { position: [0.0, 0.0], varying: Color::new(255, 0, 0) },
//!     Vertex2D { position: [1.0, 0.0], varying: Color::new(0, 255, 0) },
//!     Vertex2D { position: [1.0, 1.0], varying: Color::new(0, 0, 255) },
//!     Vertex2D { position: [0.0, 1.0], varying: Color::new(255, 255, 0) },
//! ];
//! let indices = [0u16, 1, 2, 0, 2, 3];
//! let pipeline = Pipeline::new(
//!     Passthrough2D::new(),
//!     SolidFs,
//!     PipelineState::alpha_blend(Viewport::new(0, 0, 128, 128)),
//! );
//!
//! renderer.draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline);
//! ```
//!
//! 开启 `programmable` 后，顶点着色器输出的 `ProgramVertex.position` 使用 `[0, 1]`
//! 的 normalized viewport 坐标，而不是 OpenGL 风格的 `[-1, 1]` clip space。
#![no_std]

pub mod color;
pub mod core;
pub mod framebuffer;
pub mod pixel;
#[cfg(feature = "simd")]
pub mod simd;
mod utils;
#[cfg(feature = "programmable")]
pub mod programmable;
pub mod vertex;
pub use core::Renderer;
pub use pixel::Pixel;
