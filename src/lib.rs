//! **rasteron** — 一个 `no_std` 的软件光栅化库，适用于嵌入式及通用帧缓冲渲染。
//!
//! # 概述
//!
//! rasteron 提供了轻量级的 [`Renderer`]，支持在原始帧缓冲字节切片上直接绘制
//! 像素、线段、三角形、矩形、圆形、圆弧与扇形。支持多种像素格式、可选的双缓冲、
//! Alpha 混合以及 Cohen–Sutherland 线段裁剪。
//!
//! 像素格式覆盖 8 位通道布局（[`color::PixelFormat::Rgb24`] /
//! [`color::PixelFormat::Bgr24`] / [`color::PixelFormat::Xrgb8888`] /
//! [`color::PixelFormat::Argb8888`] / [`color::PixelFormat::Rgba8888`] /
//! [`color::PixelFormat::Bgra8888`]）与 16 位 5/6/5 打包布局
//! （[`color::PixelFormat::Rgb565`]，以及字节交换存储的
//! [`color::PixelFormat::Bgr565`]）。565 写入时把 8 位通道量化到 5/6/5 位，
//! 混合时解码到 8 位再用同一套 `blend_channel` 公式计算，最后量化回写。
//!
//! 双缓冲由调用方提供后台缓冲区（[`Renderer::with_back_buffer`]），库自身不做
//! 任何分配，因此在无全局分配器的 `no_std` 环境下同样可用。
//!
//! 圆弧与扇形由定点角度类型 [`Angle`] 驱动（整圈 `2^32` 圈值 + 四分之一波表），
//! 因此即使关闭 `programmable` feature 也能使用，不引入浮点运算。
//!
//! 开启 `programmable` feature 后，还会额外暴露一条显式参数式的 2D 可编程管线：
//! 顶点缓冲、索引缓冲、顶点着色器、varying 插值、片元着色器、viewport、scissor
//! 与 `draw_arrays` / `draw_indexed`。
//!
//! # 最低支持 Rust 版本 (MSRV)
//!
//! Rust 1.85+ —— 下限由 `edition = "2024"` 决定（该 edition 需 1.85+），
//! 而不是由库内用到的语言特性决定。
//!
//! # 特性标志
//!
//! | 标志 | 说明 |
//! |------|------|
//! | `programmable` | 启用完整的 2D 可编程管线。这是唯一引入 `f32` 的特性。 |
//! | `simd`          | 启用 SIMD 优化，提升内存操作性能。支持 AVX-512、AVX2、SSE2、NEON 和 RISC-V 向量扩展。 |
//!
//! 两个标志都可单独或组合开启，**都不引入任何 `[dependencies]`**。
//! 顶点缓存 `VertexCache` 借用调用方借出的槽位，因此不需要 `alloc` 特性：
//! 库内没有自持有存储的便捷容器，也就没有活着的 `alloc` 特性。
//!
//! # 裸机 / 内核选型
//!
//! * **浮点**：关闭 `programmable` 后，整个库不依赖浮点运算（几何计算全部为整数）。
//!   启用 `programmable` 会引入 `f32`，在不支持浮点的目标上不可用；在内核态使用
//!   还需要显式保存/恢复 FPU 状态。
//! * **分配器**：全库零分配——单缓冲、双缓冲、子视口与顶点缓存所需的缓冲区
//!   全部由调用方提供，因此关闭全部特性后也不会链接全局分配器。
//! * **FPU 状态**：`simd` 会经由 `blit` / `clear` / 填充路径使用向量寄存器
//!   （x86 的 XMM/YMM/ZMM、aarch64 的 V0-V31），内核中需配合
//!   `kernel_fpu_begin/end`（或 `kernel_neon_begin/end`）使用。
//! * **缓冲区大小**：[`Renderer::new`] 与 [`Renderer::with_back_buffer`] 要求传入的
//!   切片不小于 `pitch × height`，否则恐慌（详见各函数文档）。
//!
//! # 固定功能图元
//!
//! | 方法 | 形态 |
//! |------|------|
//! | [`Renderer::set_pixel`] / `set_pixel_at` | 单像素 |
//! | [`Renderer::draw_line`] | 线段 |
//! | [`Renderer::draw_rect`] / `fill_rect` | 矩形 |
//! | [`Renderer::draw_triangle`] / `fill_triangle` | 三角形 |
//! | [`Renderer::draw_circle`] / `fill_circle` | 圆形 |
//! | [`Renderer::draw_arc`] / `fill_arc` | 圆弧 / 扇形（角度用 [`Angle`]） |
//!
//! # 坐标约定
//!
//! **Y 轴向下**，`(0,0)` 在帧缓冲左上角。角度沿屏幕坐标增大：`0°` 指向右、
//! `90°` 指向下。详见 [`angle`] 模块文档。
//!
//! # 可编程管线示例
//!
//! ```ignore
//! use rasteron::Renderer;
//! use rasteron::color::Color;
//! use rasteron::framebuffer::{FramebufferInfo, PixelFormat};
//! use rasteron::programmable::{
//!     FragmentInput, FragmentOutput, FragmentShader, Passthrough2D, Pipeline, PipelineState,
//!     PrimitiveType, Vertex2D, Viewport,
//! };
//!
//! struct SolidFs;
//!
//! impl FragmentShader for SolidFs {
//!     type Varying = Color;
//!
//!     fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
//!         FragmentOutput::Color(input.varying)
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
//! renderer.draw_indexed(PrimitiveType::Triangles, &vertices, &indices, &pipeline, None);
//! ```
//!
//! 开启 `programmable` 后，顶点着色器输出的 `FragmentInput.position` 使用 `[0, 1]`
//! 的 normalized viewport 坐标，而不是 OpenGL 风格的 `[-1, 1]` clip space。
#![no_std]
#![allow(unsafe_op_in_unsafe_fn)]

pub mod angle;
pub mod color;
pub mod core;
pub mod error;
pub mod framebuffer;
pub mod pixel;
#[cfg(feature = "programmable")]
pub mod programmable;
#[cfg(feature = "simd")]
pub mod simd;
pub mod utils;
pub use angle::Angle;
pub use core::Renderer;
pub use pixel::Pixel;
