//! subviewport — 子渲染器实现无状态裁剪的 UI 面板示例。
//!
//! 核心思想：裁剪不是渲染器上的状态，而是「用底层 buffer 的一个切片 +
//! 不变的 pitch 构造一个受限的 Renderer」。每个面板拿到自己的坐标系，
//! 越界的绘制被各自的 width/height 自然挡掉，无需任何全局 scissor。
//!
//! 运行:
//!   cargo run --example subviewport

use rasteron::Renderer;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;
use rasteron::utils::sub_view;

#[path = "../shared/bmp.rs"]
mod bmp;

/// 面板的窗口几何与内容参数。
///
/// 用结构体收拢而非堆长参数列表——面板本身就是「一个矩形 + 一点内容」的概念。
struct Panel<'a> {
    fb: &'a mut [u8],
    info: &'a FramebufferInfo,
    /// 窗口左上角（父帧缓冲坐标系）。
    x: usize,
    y: usize,
    /// 窗口尺寸（会被 `sub_view` 收缩到可视区内）。
    w: usize,
    h: usize,
    /// 标题色，同时用作强调色。
    accent: Color,
    /// 进度条填充百分比（0-100）。
    progress: u8,
}

/// 在自己的窗口内绘制标题、进度条和一段溢出内容。
fn draw_panel(spec: Panel<'_>) {
    let Panel {
        fb,
        info,
        x,
        y,
        w,
        h,
        accent,
        progress,
    } = spec;

    // 裁剪在这里发生：子渲染器只看得到窗口内那部分字节。
    let Some(mut panel) = sub_view(fb, info, x, y, w, h) else {
        return;
    };

    let (pw, ph) = (panel.info().width, panel.info().height);

    // 面板背景
    panel.fill_rect(Pixel::new(0, 0), pw, ph, Color::new(38, 42, 54));

    // 标题色块
    panel.fill_rect(Pixel::new(0, 0), pw, 4, accent);

    // 进度条：背景 + 已填充部分
    if ph > 10 {
        let bar_y = 6;
        panel.fill_rect(
            Pixel::new(2, bar_y),
            pw.saturating_sub(4),
            4,
            Color::new(70, 76, 94),
        );
        let filled = (pw.saturating_sub(4) * progress as usize) / 100;
        panel.fill_rect(Pixel::new(2, bar_y), filled, 4, accent);
    }

    // 故意溢出的内容：下面两行的尺寸取自面板自身，正常情况下不会溢出；
    // 真正的验证在 `main` 里——我们会在面板外再画一次同样内容，
    // 它必须一点都不泄漏出去。
    panel.fill_rect(Pixel::new(0, ph.saturating_sub(1)), pw, 1, accent);
    panel.fill_rect(
        Pixel::new(pw.saturating_sub(1), 0),
        1,
        ph,
        Color::new(220, 80, 80),
    );

    // 半透明叠加层：验证子渲染器内混合正常工作
    panel.fill_rect(
        Pixel::new(pw / 4, ph / 4),
        pw,
        ph,
        Color::with_alpha(255, 255, 255, 40),
    );

    // 双缓冲下绘制落在面板自己的后台缓冲里，必须 present 才写回父缓冲区；
    // 单缓冲下写入直接落在父缓冲区，没有该方法（编译期即不可用）。
    #[cfg(feature = "double_buffer")]
    panel.swap_buffers();
}

fn main() {
    let width = 640;
    let height = 480;
    let pitch = width * 3;
    let info = FramebufferInfo::new(width, height, pitch, PixelFormat::Bgr24);

    let mut fb = vec![0u8; info.buffer_size()];
    {
        let mut bg = Renderer::new(
            FramebufferInfo::new(width, height, pitch, PixelFormat::Bgr24),
            &mut fb,
        );
        bg.clear(Color::new(20, 22, 30));
        // 双缓冲下 clear 写的是 back_buffer，必须 present 才进 fb。
        // 面板必须在底色 present 之后再画，否则会被底色盖掉。
        #[cfg(feature = "double_buffer")]
        bg.swap_buffers();
    }

    // 三个面板；第三个故意贴住右下角，触发 `sub_view` 的行数收缩。
    draw_panel(Panel {
        fb: &mut fb,
        info: &info,
        x: 20,
        y: 20,
        w: 180,
        h: 120,
        accent: Color::new(80, 160, 255),
        progress: 70,
    });
    draw_panel(Panel {
        fb: &mut fb,
        info: &info,
        x: 220,
        y: 20,
        w: 180,
        h: 120,
        accent: Color::new(255, 170, 70),
        progress: 35,
    });
    draw_panel(Panel {
        fb: &mut fb,
        info: &info,
        x: 420,
        y: 300,
        w: 220,
        h: 180,
        accent: Color::new(120, 230, 140),
        progress: 90,
    });

    // 裁剪自检：面板底边以下若出现面板底色，说明溢出泄漏了。
    let mut leaked = 0usize;
    for (x0, y0, w, h) in [(20usize, 20usize, 180usize, 120usize), (220, 20, 180, 120)] {
        for y in (y0 + h)..(y0 + h + 40).min(height) {
            for x in x0..(x0 + w) {
                let o = y * pitch + x * 3;
                // 面板背景 BGR = (54, 42, 38)
                if fb[o] == 54 && fb[o + 1] == 42 && fb[o + 2] == 38 {
                    leaked += 1;
                }
            }
        }
    }

    bmp::write_bmp(
        std::path::Path::new("subviewport-example.bmp"),
        width,
        height,
        PixelFormat::Bgr24,
        &fb,
    )
    .expect("failed to write BMP");

    println!("subviewport-example.bmp");
    println!(
        "面板外泄漏像素: {leaked} => {}",
        if leaked == 0 {
            "裁剪正确"
        } else {
            "存在泄漏"
        }
    );
}
