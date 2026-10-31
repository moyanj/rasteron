//! 图像编解码：目前只有 BMP。
//!
//! # 概述
//!
//! 本模块把「文件里的字节」和「[`crate::framebuffer::FramebufferInfo`] 描述的
//! 帧缓冲」双向翻译一次，让库的使用者不必在示例或应用里各写一份 BMP 代码。
//!
//! | 方向 | 入口 | 分配 |
//! |------|------|------|
//! | 编码 | [`bmp::encode`] / [`bmp::encode_size`] | **零分配**（调用方给 `&mut [u8]`） |
//! | 解码 | [`bmp::decode`] | **零分配**（1/4/8/16/24/32 位全部就地解码） |
//!
//! # 与库其余部分的关系
//!
//! 解码结果直接落在调用方指定的目标 [`PixelFormat`] 上，因此可以原样交给
//! [`crate::Renderer`] 或 [`crate::programmable::texture::Texture2D`]；
//! 编码则直接接受 `&[u8]` 像素数据（配 [`FramebufferInfo`] 描述布局），
//! 无需先转换成 `Color`。
//!
//! 两条路径都遵守本库的核心契约：**库内不做任何分配**。解码 8 位调色板时，
//! 索引先落在输出的每个像素上（1bpp 按 8 的倍数、4bpp 按 2 的倍数），再用
//! 最多 4 次「两两交换」把高索引换到低位，因此不需要中间缓冲。
//!
//! [`PixelFormat`]: crate::color::PixelFormat
//! [`FramebufferInfo`]: crate::framebuffer::FramebufferInfo

pub mod bmp;
