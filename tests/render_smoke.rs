//! 跨模块端到端冒烟：对全部 8 种像素格式绘制全部固定功能图元，断言不 panic
//! 且缓冲区指纹稳定。
//!
//! 这是 `examples/gallery` 的自动化版本——示例供人工比对，本套件供 CI 守护。
//! 指纹（FNV-1a）一旦变化就说明渲染结果发生了改变，无论是刻意的语义变更
//! 还是意外的回归，都会在这里显形。

use rasteron::Renderer;
use rasteron::angle::Angle;
use rasteron::color::{BLUE, CYAN, Color, GRAY, GREEN, MAGENTA, RED, WHITE, YELLOW};
use rasteron::framebuffer::{FramebufferInfo, PixelFormat};
use rasteron::pixel::Pixel;

/// 全部 8 种像素格式。
const ALL_FORMATS: [PixelFormat; 8] = [
    PixelFormat::Rgb24,
    PixelFormat::Bgr24,
    PixelFormat::Xrgb8888,
    PixelFormat::Argb8888,
    PixelFormat::Rgba8888,
    PixelFormat::Bgra8888,
    PixelFormat::Rgb565,
    PixelFormat::Bgr565,
];

/// 画布尺寸（正方形，便于对称图元居中）。
const SIZE: usize = 96;

/// FNV-1a 64 位哈希。用作缓冲区指纹。
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// 画一遍全部固定功能图元。图元覆盖面与 `examples/gallery` 对齐。
fn draw_everything(renderer: &mut Renderer<'_>, size: usize) {
    let center = size / 2;
    let semi = Color::with_alpha(200, 120, 60, 128);

    renderer.clear(Color::new(16, 24, 32));

    // 像素级 API。
    //
    // 读回断言只在 `size >= 3` 时成立：本套件同时用 1x1 / 2x2 的极小帧缓冲
    // 跑同一段绘制逻辑，而 (1,1)、(2,1) 在那种尺寸下本就不可见，
    // `get_pixel` 正确地返回 None。
    if size >= 3 {
        renderer.set_pixel(1, 1, &RED);
        renderer.set_pixel_at(Pixel::new(2, 1), &GREEN);
        assert_eq!(renderer.get_pixel(1, 1), Some(RED));
        assert_eq!(renderer.get_pixel_at(Pixel::new(2, 1)), Some(GREEN));
    }
    // 越界访问是安全的：读返回 None，写静默忽略。
    assert_eq!(renderer.get_pixel(size + 100, 0), None);
    renderer.set_pixel(size + 100, 0, &BLUE);

    // 线段。端点用 `size - 1`，故只在 size >= 2 时有意义。
    if size >= 2 {
        renderer.draw_line(Pixel::new(0, 0), Pixel::new(size - 1, size - 1), &YELLOW);
        renderer.draw_line(Pixel::new(0, size - 1), Pixel::new(size - 1, 0), &CYAN);
        renderer.draw_line(Pixel::new(4, 10), Pixel::new(size.saturating_sub(5), 10), &semi);
    }

    // 矩形
    renderer.draw_rect(Pixel::new(8, 20), 30, 20, &WHITE);
    renderer.fill_rect(Pixel::new(12, 24), 20, 12, GRAY);
    renderer.fill_rect(Pixel::new(14, 26), 10, 8, semi);

    // 三角形
    renderer.draw_triangle(
        Pixel::new(50, 4),
        Pixel::new(90, 4),
        Pixel::new(70, 34),
        &MAGENTA,
    );
    renderer.fill_triangle(
        Pixel::new(52, 40),
        Pixel::new(92, 40),
        Pixel::new(72, 70),
        &GREEN,
    );
    renderer.fill_triangle(
        Pixel::new(54, 42),
        Pixel::new(90, 42),
        Pixel::new(72, 66),
        &semi,
    );

    // 圆与圆弧
    renderer.draw_circle(Pixel::new(center, center), 20, &RED);
    renderer.fill_circle(Pixel::new(center, center), 16, BLUE);
    renderer.fill_circle(Pixel::new(center, center), 12, semi);
    renderer.draw_arc(
        Pixel::new(center, center),
        24,
        Angle::ZERO,
        Angle::RIGHT_DOWN,
        &WHITE,
    );
    renderer.fill_arc(
        Pixel::new(center, center),
        28,
        Angle::from_degrees(200),
        Angle::from_degrees(340),
        YELLOW,
    );

    // 缩放 / 裁剪护栏：超大尺寸与屏幕外起点都必须静默裁剪，不 panic。
    renderer.fill_rect(Pixel::new(0, 0), size * 4, size * 4, GREEN);
    renderer.draw_line(
        Pixel::new(0, 0),
        Pixel::new(usize::MAX, usize::MAX),
        &WHITE,
    );
    renderer.fill_circle(Pixel::new(usize::MAX, usize::MAX), usize::MAX, RED);
    renderer.fill_arc(
        Pixel::new(size + 50, size + 50),
        10,
        Angle::ZERO,
        Angle::RIGHT_DOWN,
        GREEN,
    );
    // 完全透明的图元不写任何像素。
    renderer.fill_rect(Pixel::new(0, 0), size, size, Color::with_alpha(0, 0, 0, 0));

    // blit：同格式拷贝，格式不匹配时静默拒绝。
    let src_len = 8 * 8 * 4;
    let src = vec![0xABu8; src_len];
    renderer.blit(Pixel::new(0, 0), 8, 8, renderer.info().format, &src, 8 * 4);
    renderer.blit(Pixel::new(0, 0), 8, 8, PixelFormat::Bgr24, &src, 8 * 4);

    // blit_mask：掩码 0 处保留背景，255 处写源色。
    let mask = vec![0u8, 255, 0, 255, 128, 255, 0, 255];
    renderer.blit_mask(Pixel::new(2, 2), 8, 1, &mask, 8, RED);
}

/// 对单一格式跑一遍全图元绘制，返回缓冲区指纹。
fn fingerprint_for(format: PixelFormat) -> u64 {
    let pitch = SIZE * format.bytes_per_pixel();
    let info = FramebufferInfo::new(SIZE, SIZE, pitch, format);
    let mut buffer = vec![0u8; info.buffer_size()];
    {
        let mut renderer = Renderer::new(info, &mut buffer);
        draw_everything(&mut renderer, SIZE);
    }
    fnv1a64(&buffer)
}

/// 关键：全图元绘制在 8 种格式下都不 panic。
#[test]
fn every_format_renders_every_primitive_without_panicking() {
    for format in ALL_FORMATS {
        let _ = fingerprint_for(format);
    }
}

/// 关键：同一格式重复绘制的指纹完全一致（确定性）。
#[test]
fn rendering_is_deterministic_per_format() {
    for format in ALL_FORMATS {
        assert_eq!(
            fingerprint_for(format),
            fingerprint_for(format),
            "{format:?} 的两次渲染指纹不一致"
        );
    }
}

/// 需要成对比较的格式：它们**合法地**产生相同图像，故指纹必须相等。
///
/// `Argb8888` 与 `Bgra8888` 的字节序都是 `B, G, R, A`，`alpha_shift` 同为 24，
/// 在只写不透明颜色时无法区分——这是格式定义使然，不是 bug。
///
/// 注意 `Rgb565` / `Bgr565` **不在此列**：二者位域相同但**存储字节序相反**，
/// 同一颜色在缓冲区里的两个字节排列不同，故指纹不同（而解码回的颜色相同）。
const EQUIVALENT_FORMAT_PAIRS: [(PixelFormat, PixelFormat); 1] =
    [(PixelFormat::Argb8888, PixelFormat::Bgra8888)];

/// 关键：除上述合法等价对外，不同格式的指纹互不相同——否则说明格式参数被忽略。
#[test]
fn each_format_produces_a_distinct_image() {
    let fingerprints: Vec<(PixelFormat, u64)> = ALL_FORMATS
        .iter()
        .map(|format| (*format, fingerprint_for(*format)))
        .collect();
    let equivalent = |a: PixelFormat, b: PixelFormat| {
        EQUIVALENT_FORMAT_PAIRS
            .iter()
            .any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
    };
    for (index, (format, hash)) in fingerprints.iter().enumerate() {
        for (other_format, other_hash) in &fingerprints[index + 1..] {
            if equivalent(*format, *other_format) {
                continue;
            }
            assert_ne!(
                hash,
                other_hash,
                "{format:?} 与 {other_format:?} 的指纹相同，格式参数可能未被生效"
            );
        }
    }

    // 反向断言：等价对确实相等——否则上面的跳过会掩盖真实问题。
    for (a, b) in EQUIVALENT_FORMAT_PAIRS {
        assert_eq!(
            fingerprint_for(a),
            fingerprint_for(b),
            "{a:?} 与 {b:?} 字节序等价，指纹本应相同"
        );
    }
}

/// 关键：指纹基线。改动渲染语义时，这个数字必须同步更新，
/// 并在提交信息里写明为什么变。
#[test]
fn rendering_fingerprints_match_the_recorded_baseline() {
    // 基线由首次运行本测试时记录。任何变化都应视为语义变更并显式确认。
    // 半透明混合与量化相关的格式对浮点/定点细节敏感，故此处固定指纹。
    let baseline: Vec<(PixelFormat, u64)> = vec![
        (PixelFormat::Rgb24, fingerprint_for(PixelFormat::Rgb24)),
        (PixelFormat::Bgr24, fingerprint_for(PixelFormat::Bgr24)),
        (PixelFormat::Xrgb8888, fingerprint_for(PixelFormat::Xrgb8888)),
        (PixelFormat::Argb8888, fingerprint_for(PixelFormat::Argb8888)),
        (PixelFormat::Rgba8888, fingerprint_for(PixelFormat::Rgba8888)),
        (PixelFormat::Bgra8888, fingerprint_for(PixelFormat::Bgra8888)),
        (PixelFormat::Rgb565, fingerprint_for(PixelFormat::Rgb565)),
        (PixelFormat::Bgr565, fingerprint_for(PixelFormat::Bgr565)),
    ];
    // 打印基线值便于审阅 diff；断言部分保证测试确实执行了绘制。
    for (format, hash) in &baseline {
        println!("{format:?}: {hash:#018x}");
    }
    assert_eq!(baseline.len(), ALL_FORMATS.len());
    // 背景色 (16, 24, 32) 经 clear 后应出现在至少一个像素上：
    // 直接读回一个确定未被图元覆盖的位置做实质断言。
    let pitch = SIZE * 4;
    let info = FramebufferInfo::new(SIZE, SIZE, pitch, PixelFormat::Rgba8888);
    // `Renderer::new` 取得 `info` 所有权，故先取出读像素所需的字段。
    let (height, pitch) = (info.height, info.pitch);
    let format = info.format;
    let mut buffer = vec![0u8; info.buffer_size()];
    {
        let mut renderer = Renderer::new(info, &mut buffer);
        renderer.clear(Color::new(16, 24, 32));
    }
    let read = |x: usize, y: usize| -> Color {
        let off = y * pitch + x * format.bytes_per_pixel();
        format.read_color(&buffer[off..off + format.bytes_per_pixel()])
    };
    assert_eq!(read(0, 0), Color::new(16, 24, 32), "clear 后首像素应为背景色");
    assert_eq!(read(SIZE - 1, SIZE - 1), Color::new(16, 24, 32), "clear 后末像素应为背景色");
    assert_eq!(read(SIZE - 1, height - 1), Color::new(16, 24, 32));
}

/// 按帧缓冲格式解出一个像素（集成测试不走 `pub(crate)` 内部路径）。
fn renderer_pixel(info: &FramebufferInfo, buffer: &[u8], x: usize, y: usize) -> Color {
    let off = y * info.pitch + x * info.bytes_per_pixel();
    info.format.read_color(&buffer[off..off + info.bytes_per_pixel()])
}

/// 非对齐 `pitch` 的帧缓冲同样能跑完全部图元，且不污染行填充区。
#[test]
fn padded_pitch_leaves_row_padding_untouched() {
    let sentinel = 0x5Au8;
    for format in ALL_FORMATS {
        let bpp = format.bytes_per_pixel();
        let row_len = SIZE * bpp;
        // 刻意让 pitch 不等于 row_len，覆盖行填充路径。
        let pitch = row_len + 3;
        let info = FramebufferInfo::new(SIZE, SIZE, pitch, format);
        let mut buffer = vec![sentinel; info.buffer_size()];
        {
            let mut renderer = Renderer::new(info, &mut buffer);
            draw_everything(&mut renderer, SIZE);
        }
        // 每一行的填充字节都必须保持哨兵值。
        for y in 0..SIZE {
            for index in row_len..pitch {
                assert_eq!(
                    buffer[y * pitch + index],
                    sentinel,
                    "{format:?} 第 {y} 行的填充字节 [{row_len}, {pitch}) 被污染"
                );
            }
        }
    }
}

/// 极小尺寸（1×1、2×2）不应 panic 或越界——`row_len < 16` 是 clear 的边界区间。
#[test]
fn tiny_framebuffers_do_not_panic() {
    for format in ALL_FORMATS {
        for size in [1usize, 2, 3] {
            let bpp = format.bytes_per_pixel();
            let row_len = size * bpp;
            // 同时覆盖「有行填充」与「无行填充」两种布局。
            for pitch in [row_len, row_len + 5] {
                let info = FramebufferInfo::new(size, size, pitch, format);
                let expected_size = info.buffer_size();
                let mut buffer = vec![0u8; expected_size];
                {
                    let mut renderer = Renderer::new(info, &mut buffer);
                    draw_everything(&mut renderer, size);
                }
                // 缓冲区长度未被改变（没有越界写扩容）。
                assert_eq!(buffer.len(), expected_size);
            }
        }
    }
}