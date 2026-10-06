//! 可编程管线端到端：三角形渐变落盘后按采样点断言插值颜色。
//!
//! 这是 S30 里唯一还没覆盖的集成套件。与单元测试不同，这里全程走**公开 API**
//! （不使用 `pub(crate)` 内部路径），验证的是「顶点 → 插值 → 落盘」的完整链路。

use rasteron::Renderer;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::programmable::shader::Passthrough2D;
use rasteron::programmable::{
    FragmentInput, FragmentOutput, FragmentShader, Pipeline, PipelineState, PrimitiveType,
    Vertex2D, Viewport,
};

/// 画布边长（正方形）。
const SIZE: usize = 64;

/// 逐像素读取（走公开的 `PixelFormat::read_color`）。
///
/// 注意不能用 `Color::from_rgba_u32`：它的位布局是 `R24G16B8A0`（R 在**最高**
/// 字节），与 Rgba8888 的 `R, G, B, A` 字节序恰好相反。
fn read(buffer: &[u8], x: usize, y: usize) -> Color {
    let off = y * SIZE * 4 + x * 4;
    PixelFormat::Rgba8888.read_color(&buffer[off..off + 4])
}

/// 未被写过的 Rgba8888 像素的解码结果。
///
/// 缓冲初始为全 0，故未着色处解出 `a == 0`（**不是** 255）——`read_color`
/// 忠实解码磁盘字节，`Color::new` 才会补出不透明 alpha。测试里必须用本常量
/// 判定「未着色」，写成 `Color::new(0,0,0)` 会误判。
const UNTOUCHED: Color = Color::with_alpha(0, 0, 0, 0);

/// 原样透传 varying 的片元着色器。
struct SolidFs;

impl FragmentShader for SolidFs {
    type Varying = Color;
    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
        FragmentOutput::Color(input.varying)
    }
}

/// 画一个覆盖整屏的渐变三角形并返回帧缓冲。
fn draw_gradient(vertices: [Vertex2D<Color>; 3], blend_alpha: bool) -> Vec<u8> {
    let info = FramebufferInfo::new(SIZE, SIZE, SIZE * 4, PixelFormat::Rgba8888);
    let size = info.buffer_size();
    let mut buffer = vec![0u8; size];
    {
        let mut renderer = Renderer::new(info, &mut buffer);
        let state = if blend_alpha {
            PipelineState::alpha_blend(Viewport::new(0, 0, SIZE, SIZE))
        } else {
            PipelineState {
                blend: rasteron::programmable::BlendMode::None,
                scissor: None,
                viewport: Viewport::new(0, 0, SIZE, SIZE),
            }
        };
        let pipeline = Pipeline::new(Passthrough2D::new(), SolidFs, state);
        renderer
            .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
            .unwrap();
    }
    buffer
}

/// 关键契约：纯红顶点的渐变三角形，中间像素应接近红，角落接近未覆盖的黑。
#[test]
fn gradient_triangle_interpolates_across_the_screen() {
    // 左上角红色、右上角绿色、左下角蓝色——覆盖整屏。
    let vertices = [
        Vertex2D {
            position: [0.0, 0.0],
            varying: Color::new(255, 0, 0),
        },
        Vertex2D {
            position: [1.0, 0.0],
            varying: Color::new(0, 255, 0),
        },
        Vertex2D {
            position: [0.0, 1.0],
            varying: Color::new(0, 0, 255),
        },
    ];
    let buffer = draw_gradient(vertices, false);

    // 靠近三个顶点的像素应分别以对应通道占主导。
    let near_red = read(&buffer, 2, 2);
    assert!(near_red.r > 200, "顶点附近应以红色为主，实得 {near_red:?}");
    assert!(near_red.g < 60 && near_red.b < 60, "不应混入其它通道：{near_red:?}");

    // 绿、蓝两个顶点的探测点取 y = 1 / x = 1 而非 2：斜边 `x + y = SIZE` 的
    // 归属按 top-left 规则给了邻侧三角形，`(61, 2)` 的中心 (61.5, 2.5) 和恰为
    // 64.0 正落在斜边上，因而**不归**本三角形（`(2, 61)` 同理）。
    // 这一条也顺带证明：共边三角形不会重复着色，斜边上的像素不会两边都算。
    let near_green = read(&buffer, SIZE - 3, 1);
    assert!(near_green.g > 200, "顶点附近应以绿色为主，实得 {near_green:?}");
    assert!(near_green.r < 60, "不应混入红色：{near_green:?}");

    let near_blue = read(&buffer, 1, SIZE - 3);
    assert!(near_blue.b > 200, "顶点附近应以蓝色为主，实得 {near_blue:?}");
    assert!(near_blue.r < 60, "不应混入红色：{near_blue:?}");

    // 三角形之外的像素（左下角）不应被着色。
    assert_eq!(
        read(&buffer, SIZE - 2, SIZE - 2),
        UNTOUCHED,
        "三角形外的像素不应被写入"
    );
}

/// 关键契约：顶点顺序不影响插值结果（重心坐标归一化正确）。
///
/// 这同时守护 `8586ef3` 修过的「面积三倍缩放」回归：若 `area` 被放大 3 倍，
/// 常量 varying 也会被整体缩小，这里用两个三角形的**相同区域**对照即可发现。
#[test]
fn winding_order_does_not_change_interpolation() {
    let make = |swap: bool| {
        let mut v = [
            Vertex2D {
                position: [0.0, 0.0],
                varying: Color::new(255, 0, 0),
            },
            Vertex2D {
                position: [1.0, 0.0],
                varying: Color::new(0, 255, 0),
            },
            Vertex2D {
                position: [0.0, 1.0],
                varying: Color::new(0, 0, 255),
            },
        ];
        if swap {
            v.swap(1, 2);
        }
        v
    };

    let forward = draw_gradient(make(false), false);
    let reversed = draw_gradient(make(true), false);

    // 逐像素对照：两种绕向必须得到完全相同的图像。
    for y in (1..SIZE).step_by(7) {
        for x in (1..SIZE).step_by(7) {
            assert_eq!(
                read(&forward, x, y),
                read(&reversed, x, y),
                "像素 ({x}, {y}) 在两种绕向下插值结果不同"
            );
        }
    }
}

/// 关键契约：常量 varying 不会被面积缩放整体缩小（`8586ef3` 回归护栏）。
#[test]
fn constant_varying_is_preserved_exactly() {
    let constant = Color::new(128, 64, 32);
    let vertices = [
        Vertex2D {
            position: [0.0, 0.0],
            varying: constant,
        },
        Vertex2D {
            position: [1.0, 0.0],
            varying: constant,
        },
        Vertex2D {
            position: [0.0, 1.0],
            varying: constant,
        },
    ];
    let buffer = draw_gradient(vertices, false);

    // 逐像素核对：三角形内每个像素都必须是原值。
    let mut checked = 0;
    for y in 1..SIZE - 1 {
        for x in 1..SIZE - 1 {
            let got = read(&buffer, x, y);
            if got == UNTOUCHED {
                continue; // 三角形外
            }
            assert_eq!(
                got, constant,
                "常量 varying 在 ({x}, {y}) 处被缩放（8586ef3 回归）"
            );
            checked += 1;
        }
    }
    assert!(checked > 1000, "应检查到大量像素，实际 {checked}");
}

/// 索引绘制与顶点缓冲绘制产生相同结果。
#[test]
fn indexed_and_array_draws_agree() {
    let info = FramebufferInfo::new(SIZE, SIZE, SIZE * 4, PixelFormat::Rgba8888);
    let vertices = [
        Vertex2D {
            position: [0.0, 0.0],
            varying: Color::new(255, 0, 0),
        },
        Vertex2D {
            position: [1.0, 0.0],
            varying: Color::new(0, 255, 0),
        },
        Vertex2D {
            position: [0.0, 1.0],
            varying: Color::new(0, 0, 255),
        },
    ];
    let indices = [0u16, 1, 2];

    let mut via_array = vec![0u8; info.buffer_size()];
    {
        let mut renderer = Renderer::new(
            FramebufferInfo::new(SIZE, SIZE, SIZE * 4, PixelFormat::Rgba8888),
            &mut via_array,
        );
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            SolidFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, SIZE, SIZE)),
        );
        renderer
            .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
            .unwrap();
    }

    let mut via_index = vec![0u8; info.buffer_size()];
    {
        let mut renderer = Renderer::new(
            FramebufferInfo::new(SIZE, SIZE, SIZE * 4, PixelFormat::Rgba8888),
            &mut via_index,
        );
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            SolidFs,
            PipelineState::alpha_blend(Viewport::new(0, 0, SIZE, SIZE)),
        );
        renderer
            .draw_indexed(
                PrimitiveType::Triangles,
                &vertices,
                &indices,
                &pipeline,
                None,
            )
            .unwrap();
    }
    assert_eq!(via_array, via_index, "索引绘制应与顶点缓冲绘制结果一致");
}

/// viewport 之外的顶点被裁剪，viewport 变换正确平移了输出。
#[test]
fn viewport_offsets_the_output_region() {
    let info = FramebufferInfo::new(SIZE, SIZE, SIZE * 4, PixelFormat::Rgba8888);
    // viewport 从 (16,16) 开始、大小 32x32。
    let viewport = Viewport::new(16, 16, 32, 32);
    let vertices = [
        Vertex2D {
            position: [0.0, 0.0],
            varying: Color::new(255, 0, 0),
        },
        Vertex2D {
            position: [1.0, 0.0],
            varying: Color::new(255, 0, 0),
        },
        Vertex2D {
            position: [0.0, 1.0],
            varying: Color::new(255, 0, 0),
        },
    ];
    let mut buffer = vec![0u8; info.buffer_size()];
    {
        let mut renderer =
            Renderer::new(FramebufferInfo::new(SIZE, SIZE, SIZE * 4, PixelFormat::Rgba8888), &mut buffer);
        let pipeline = Pipeline::new(
            Passthrough2D::new(),
            SolidFs,
            PipelineState::alpha_blend(viewport),
        );
        renderer
            .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
            .unwrap();
    }

    // viewport 区域内应有像素被着色。
    assert_ne!(read(&buffer, 20, 20), UNTOUCHED, "viewport 内应被着色");
    // viewport 之外必须保持空白——尤其是原点一侧。
    assert_eq!(read(&buffer, 2, 2), UNTOUCHED, "viewport 外不应被着色");
    assert_eq!(read(&buffer, SIZE - 2, SIZE - 2), UNTOUCHED);
}