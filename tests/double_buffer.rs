//! 双缓冲端到端：绘制落在后台缓冲区，`swap_buffers` 呈现到前台。
//!
//! 核心不变量是「双缓冲 + swap 后，前台逐字节等于单缓冲直接绘制的结果」——
//! 这条一旦不成立，说明后台写入或 present 的某一路径丢了数据或改了数据。

use rasteron::Renderer;
use rasteron::angle::Angle;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;

/// 画布尺寸。
const W: usize = 48;
const H: usize = 32;

/// 统一色板：每个图元一个可区分的颜色。
const PALETTE: [Color; 6] = [
    Color::new(255, 0, 0),
    Color::new(0, 255, 0),
    Color::new(0, 0, 255),
    Color::new(255, 255, 0),
    Color::new(0, 255, 255),
    Color::new(255, 0, 255),
];

/// 在给定渲染器上画一遍固定场景（图元集合与布局保持确定）。
fn draw_scene(renderer: &mut Renderer<'_>) {
    renderer.clear(Color::new(8, 8, 16));
    renderer.draw_line(Pixel::new(0, 0), Pixel::new(W - 1, H - 1), &PALETTE[0]);
    renderer.draw_line(Pixel::new(0, H - 1), Pixel::new(W - 1, 0), &PALETTE[1]);
    renderer.draw_rect(Pixel::new(2, 2), 12, 8, &PALETTE[2]);
    renderer.fill_rect(Pixel::new(20, 4), 10, 6, PALETTE[3]);
    renderer.fill_circle(Pixel::new(12, 22), 6, PALETTE[4]);
    renderer.fill_arc(
        Pixel::new(32, 20),
        8,
        Angle::ZERO,
        Angle::RIGHT_DOWN,
        PALETTE[5],
    );
    renderer.fill_triangle(
        Pixel::new(2, 28),
        Pixel::new(16, 28),
        Pixel::new(9, 12),
        &PALETTE[2],
    );
}

/// 构造「单缓冲」参考帧缓冲。
fn render_single_buffered(format: PixelFormat) -> Vec<u8> {
    let pitch = W * format.bytes_per_pixel();
    let info = FramebufferInfo::new(W, H, pitch, format);
    let mut buffer = vec![0u8; info.buffer_size()];
    {
        let mut renderer = Renderer::new(info, &mut buffer);
        draw_scene(&mut renderer);
    }
    buffer
}

/// 构造「双缓冲 + swap」帧缓冲。
fn render_double_buffered(format: PixelFormat) -> Vec<u8> {
    let pitch = W * format.bytes_per_pixel();
    let info = FramebufferInfo::new(W, H, pitch, format);
    let size = info.buffer_size();
    let mut storage = vec![0u8; size * 2];
    {
        let (front, back) = storage.split_at_mut(size);
        let mut renderer = Renderer::with_back_buffer(info, front, back);
        draw_scene(&mut renderer);
        renderer.swap_buffers();
    }
    // 只取前半段（前台）。
    storage[..size].to_vec()
}

/// 关键契约：双缓冲 + swap 之后，前台与单缓冲直接绘制逐字节相同。
#[test]
fn double_buffer_matches_single_buffer_for_every_format() {
    for format in [
        PixelFormat::Rgb24,
        PixelFormat::Bgr24,
        PixelFormat::Xrgb8888,
        PixelFormat::Argb8888,
        PixelFormat::Rgba8888,
        PixelFormat::Bgra8888,
        PixelFormat::Rgb565,
        PixelFormat::Bgr565,
    ] {
        assert_eq!(
            render_double_buffered(format),
            render_single_buffered(format),
            "{format:?} 的双缓冲结果与单缓冲不一致"
        );
    }
}

/// 双缓冲模式下，绘制只落在后台缓冲区；前台在 swap 前保持原样。
#[test]
fn draws_do_not_touch_the_front_buffer_before_swap() {
    let format = PixelFormat::Rgba8888;
    let pitch = W * format.bytes_per_pixel();
    let info = FramebufferInfo::new(W, H, pitch, format);
    let size = info.buffer_size();
    let mut storage = vec![0u8; size * 2];
    let (front, back) = storage.split_at_mut(size);

    // 前台预置为可辨识的哨兵值。
    front.fill(0xC7);
    let before = front.to_vec();

    {
        let mut renderer = Renderer::with_back_buffer(info, front, back);
        assert!(renderer.is_double_buffered());
        // `buffer()` 在双缓冲下返回**后台**缓冲区。
        renderer.clear(Color::new(1, 2, 3));
        // `buffer()` 在双缓冲下返回**后台**缓冲区：它的内容已被 clear 改写，
        // 不再是初始化时的 0xC7 哨兵。
        assert!(
            renderer.buffer().iter().all(|byte| *byte != 0xC7),
            "绘制应落在后台缓冲区"
        );
    }
    assert_eq!(
        &storage[..size][..],
        &before[..],
        "swap 之前前台不得被触碰"
    );
}

/// 单缓冲模式下 `swap_buffers` 是空操作。
#[test]
fn swap_buffers_is_a_noop_for_single_buffered_renderers() {
    let format = PixelFormat::Rgba8888;
    let pitch = W * format.bytes_per_pixel();
    let info = FramebufferInfo::new(W, H, pitch, format);
    let mut buffer = vec![0u8; info.buffer_size()];
    let mut renderer = Renderer::new(info, &mut buffer);
    assert!(!renderer.is_double_buffered());
    draw_scene(&mut renderer);
    let snapshot = renderer.buffer().to_vec();
    renderer.swap_buffers();
    assert_eq!(
        renderer.buffer(),
        &snapshot[..],
        "单缓冲的 swap_buffers 必须是无操作"
    );
}

/// `swap_buffers` 保留行填充字节——不得用整段 memcpy 覆盖填充区。
#[test]
fn swap_buffers_preserves_row_padding_bytes() {
    let sentinel = 0x5Au8;
    for format in [
        PixelFormat::Rgb24,
        PixelFormat::Bgr24,
        PixelFormat::Rgba8888,
        PixelFormat::Rgb565,
    ] {
        let bpp = format.bytes_per_pixel();
        let row_len = W * bpp;
        // 刻意让 pitch 大于行宽，制造行填充区。
        let pitch = row_len + 7;
        let info = FramebufferInfo::new(W, H, pitch, format);
        let size = info.buffer_size();
        let mut storage = vec![sentinel; size * 2];
        let (front, back) = storage.split_at_mut(size);
        {
            let mut renderer = Renderer::with_back_buffer(info, front, back);
            draw_scene(&mut renderer);
            renderer.swap_buffers();
        }
        // 前台的每一行填充字节都必须保持哨兵值。
        for y in 0..H {
            for index in row_len..pitch {
                assert_eq!(
                    storage[y * pitch + index],
                    sentinel,
                    "{format:?} 第 {y} 行填充字节 [{row_len}, {pitch}) 被 swap 覆盖"
                );
            }
        }
    }
}

/// 连续两次 swap：后台内容不变，前台被同样地重写一次（单向拷贝语义）。
#[test]
fn repeated_swaps_are_idempotent() {
    let format = PixelFormat::Rgba8888;
    let pitch = W * format.bytes_per_pixel();
    let info = FramebufferInfo::new(W, H, pitch, format);
    let size = info.buffer_size();
    let mut storage = vec![0u8; size * 2];
    // `Renderer::with_back_buffer` 取得 `info` 所有权，两次构造各需一份。
    // `FramebufferInfo` 未实现 Clone，故用同样的字段重新构造。
    let rebuild = || FramebufferInfo::new(W, H, pitch, format);
    let (front, back) = storage.split_at_mut(size);
    // 渲染器持有两个切片的可变借用，故快照必须在它离开作用域之后取。
    {
        let info = rebuild();
        let mut renderer = Renderer::with_back_buffer(info, front, back);
        draw_scene(&mut renderer);
        renderer.swap_buffers();
    }
    let after_first = front.to_vec();
    // 后台缓冲区内容保持不变（swap 是拷贝，不是交换）。
    let back_snapshot = back.to_vec();
    {
        let mut renderer = Renderer::with_back_buffer(rebuild(), front, back);
        renderer.swap_buffers();
    }
    assert_eq!(front, &after_first[..], "重复 swap 应得到相同的前台内容");
    assert_eq!(back, &back_snapshot[..], "swap 不得修改后台缓冲区");
}