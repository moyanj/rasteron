//! 子视口端到端：父缓冲 + 多面板，断言面板边界外零污染。
//!
//! 面板通过 [`sub_view`] / [`sub_view_with_back_buffer`] 在父帧缓冲上开窗。
//! 关键风险是**跨行写入**：子渲染器的切片按父缓冲的 `pitch` 跨行，若某条写入
//! 路径按「切片是紧凑连续内存」处理，就会写穿到相邻行、污染面板外的像素。
//! 这些测试把每个面板边界外的字节都逐一核对。

use rasteron::Renderer;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;
use rasteron::utils::{sub_view, sub_view_with_back_buffer};

/// 父帧缓冲尺寸。
const W: usize = 64;
const H: usize = 64;

/// 面板外的哨兵值：任何被面板污染都会被发现。
const SENTINEL: u8 = 0x5A;

/// 面板定义：(x, y, 宽, 高, 颜色下标)。
const PANELS: [(usize, usize, usize, usize, u8); 4] = [
    (2, 2, 16, 12, 0),
    (24, 4, 20, 16, 1),
    (4, 30, 24, 20, 2),
    (34, 34, 24, 24, 3),
];

const PALETTE: [Color; 4] = [
    Color::new(255, 0, 0),
    Color::new(0, 255, 0),
    Color::new(0, 0, 255),
    Color::new(255, 255, 255),
];

/// 逐像素断言：面板外的像素必须保持哨兵值。
fn assert_panels_are_isolated(info: &FramebufferInfo, buffer: &[u8]) {
    let bpp = info.bytes_per_pixel();
    for y in 0..H {
        for x in 0..W {
            // 该像素是否落在任一面板内。
            let inside = PANELS
                .iter()
                .any(|&(px, py, pw, ph, _)| x >= px && x < px + pw && y >= py && y < py + ph);
            if inside {
                continue;
            }
            let off = y * info.pitch + x * bpp;
            assert!(
                buffer[off..off + bpp].iter().all(|byte| *byte == SENTINEL),
                "面板外像素 ({x}, {y}) 被污染"
            );
        }
    }
}

/// 关键契约：单缓冲子视口只在面板内写入。
#[test]
fn single_buffered_panels_do_not_paint_outside() {
    let pitch = W * 4;
    let info = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888);
    let mut buffer = vec![SENTINEL; info.buffer_size()];
    {
        for &(x, y, pw, ph, color) in PANELS.iter() {
            let mut panel =
                sub_view(&mut buffer, &info, x, y, pw, ph).expect("面板窗口应当可用");
            // 画满整个面板，边界处不做任何保留——任何越界都会立刻显形。
            panel.fill_rect(Pixel::new(0, 0), pw, ph, PALETTE[color as usize]);
        }
    }
    assert_panels_are_isolated(&info, &buffer);
}

/// 关键契约：双缓冲子视口 swap 后只在面板内呈现。
#[test]
fn double_buffered_panels_do_not_paint_outside() {
    let pitch = W * 4;
    let info = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888);
    let mut buffer = vec![SENTINEL; info.buffer_size()];
    let mut back = vec![0u8; info.buffer_size()];
    {
        for &(x, y, pw, ph, color) in PANELS.iter() {
            let mut panel = sub_view_with_back_buffer(&mut buffer, &mut back, &info, x, y, pw, ph)
                .expect("面板窗口应当可用");
            assert!(panel.is_double_buffered());
            panel.fill_rect(Pixel::new(0, 0), pw, ph, PALETTE[color as usize]);
            panel.swap_buffers();
        }
    }
    assert_panels_are_isolated(&info, &buffer);
}

/// 面板之间的区域不被任一面板覆盖——每个面板的颜色只出现在自己内部。
#[test]
fn each_panel_contains_only_its_own_color() {
    let pitch = W * 4;
    let info = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888);
    let mut buffer = vec![SENTINEL; info.buffer_size()];
    {
        for &(x, y, pw, ph, color) in PANELS.iter() {
            let mut panel =
                sub_view(&mut buffer, &info, x, y, pw, ph).expect("面板窗口应当可用");
            panel.fill_rect(Pixel::new(0, 0), pw, ph, PALETTE[color as usize]);
        }
    }
    // 逐面板核对其内部每个像素都是自己的颜色。
    for &(x, y, pw, ph, color) in PANELS.iter() {
        let expected = PALETTE[color as usize];
        for py in y..y + ph {
            for px in x..x + pw {
                let off = py * pitch + px * 4;
                let got = PixelFormat::Rgba8888.read_color(&buffer[off..off + 4]);
                assert_eq!(
                    got, expected,
                    "面板 ({x},{y}) 内的像素 ({px}, {py}) 颜色不符"
                );
            }
        }
    }
}

/// 等价性：子视口绘制 == 父视口上按偏移绘制（同图元、同颜色）。
#[test]
fn sub_view_drawing_equals_offset_drawing_on_the_parent() {
    let pitch = W * 4;
    let size = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888).buffer_size();

    // 面板尺寸 (16, 12)。两侧画**同样大小**的矩形，否则裁剪范围不同、结果必然不等：
    // 子视口会把图元裁到 16x12 的窗口内，而父视口没有这层窗口。
    let (x, y, pw, ph, _) = PANELS[0];

    // (a) 用子视口画恰好填满窗口的矩形。
    let mut via_panel = vec![SENTINEL; size];
    {
        let info = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888);
        let mut panel = sub_view(&mut via_panel, &info, x, y, pw, ph).unwrap();
        panel.fill_rect(Pixel::new(0, 0), pw, ph, PALETTE[0]);
    }
    // (b) 直接在父缓冲上按同样的偏移、同样的尺寸绘制。
    let mut via_parent = vec![SENTINEL; size];
    {
        let mut renderer = Renderer::new(FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888), &mut via_parent);
        renderer.fill_rect(Pixel::new(x, y), pw, ph, PALETTE[0]);
    }
    assert_eq!(
        via_panel, via_parent,
        "子视口绘制应与父视口上的偏移绘制逐字节相同"
    );

    // 换一个尺寸再验一次，确认不是巧合。
    let mut via_panel = vec![SENTINEL; size];
    {
        let info = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888);
        let mut panel = sub_view(&mut via_panel, &info, x, y, pw, ph).unwrap();
        // 只画窗口的左上半部分。
        panel.fill_rect(Pixel::new(0, 0), pw / 2, ph / 2, PALETTE[1]);
    }
    let mut via_parent = vec![SENTINEL; size];
    {
        let mut renderer = Renderer::new(FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888), &mut via_parent);
        renderer.fill_rect(Pixel::new(x, y), pw / 2, ph / 2, PALETTE[1]);
    }
    assert_eq!(via_panel, via_parent, "半窗口矩形也应逐字节一致");
}

/// 行填充字节不被子视口写入触碰。
#[test]
fn sub_view_writes_never_touch_row_padding() {
    let bpp = 4;
    let row_len = W * bpp;
    let pitch = row_len + 6; // 刻意制造行填充
    let info = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888);
    let mut buffer = vec![SENTINEL; info.buffer_size()];
    {
        for &(x, y, pw, ph, color) in PANELS.iter() {
            let mut panel =
                sub_view(&mut buffer, &info, x, y, pw, ph).expect("面板窗口应当可用");
            panel.fill_rect(Pixel::new(0, 0), pw, ph, PALETTE[color as usize]);
        }
    }
    for y in 0..H {
        for index in row_len..pitch {
            assert_eq!(
                buffer[y * pitch + index],
                SENTINEL,
                "第 {y} 行填充字节 [{row_len}, {pitch}) 被子视口污染"
            );
        }
    }
}

/// 越界的子视口窗口按契约收缩，且收缩后仍不污染面板外区域。
#[test]
fn oversized_panel_shrinks_and_stays_contained() {
    let pitch = W * 4;
    let info = FramebufferInfo::new(W, H, pitch, PixelFormat::Rgba8888);
    let mut buffer = vec![SENTINEL; info.buffer_size()];
    {
        // 起点 (56, 52)，请求 40x40——远超右下角可用空间。
        let mut panel = sub_view(&mut buffer, &info, 56, 52, 40, 40).expect("应收缩而非拒绝");
        let (pw, ph) = (panel.info().width, panel.info().height);
        assert!(pw <= W - 56 && ph <= H - 52, "收缩后的尺寸必须落在父帧缓冲内");
        panel.fill_rect(Pixel::new(0, 0), pw, ph, PALETTE[1]);
    }
    // 整个缓冲里只应有 (56..64, 52..64) 这个 8x12 区域被涂，其余保持哨兵。
    for y in 0..H {
        for x in 0..W {
            let inside = x >= 56 && x < 64 && y >= 52 && y < 64;
            if inside {
                continue;
            }
            let off = y * pitch + x * 4;
            assert_eq!(
                &buffer[off..off + 4],
                &[SENTINEL; 4][..],
                "收缩窗口仍污染了 ({x}, {y})"
            );
        }
    }
}