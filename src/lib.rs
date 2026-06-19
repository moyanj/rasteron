//! **rasteron** — 一个 `no_std` 的软件光栅化库，适用于嵌入式及通用帧缓冲渲染。
//!
//! # 概述
//!
//! rasteron 提供了轻量级的 [`Renderer`]，支持在原始帧缓冲字节切片上直接绘制
//! 像素、线段、三角形、矩形和圆形。支持多种像素格式、可选的双缓冲、Alpha 混合
//! 以及 Cohen–Sutherland 线段裁剪。
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
//! | `programmable`  | 启用着色器顶点抽象（开发中）。 |
#![no_std]

pub mod color;
pub mod core;
pub mod framebuffer;
pub mod pixel;
#[cfg(feature = "simd")]
pub mod simd;
mod utils;
#[cfg(feature = "programmable")]
pub mod vertex;
pub use core::Renderer;
pub use pixel::Pixel;
