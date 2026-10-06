//! 可编程管线基准：顶点/片元着色、图元装配、混合模式、纹理采样与顶点缓存。
//!
//! 本文件的**全部**内容由 `#[cfg(feature = "programmable")]` 门控，
//! 因为 `rasteron::programmable` 整个模块随该 feature 消失
//! （`lib.rs`：`#[cfg(feature = "programmable")] pub mod programmable;`）。
//! `Cargo.toml` 里对应的 `[[bench]]` 也声明了 `required-features`，
//! 使 `cargo bench --no-default-features` 直接跳过而不是编译失败。
//!
//! 覆盖的公开面：
//!
//! * `Pipeline` / `PipelineState` / `Viewport` / `ScissorRect`
//! * `Renderer::{draw_arrays, draw_indexed, viewport}`
//! * 三种图元：`Points` / `Lines` / `Triangles`
//! * 五种混合：`None` / `Alpha` / `Add` / `Multiply` / `PremultipliedAlpha`
//! * 四种内建着色器：`Passthrough2D` / `SolidFs` / `GradientFs` / `TextureFs`
//! * 四种 varying 类型：`()` / `Color` / `Vec2` / `Flat<T>`
//! * `Texture2D::{sample, sample_nearest, sample_bilinear, *_repeat}` × 4 种过滤/寻址组合
//! * `VertexCache` 的命中与落空

#![cfg(feature = "programmable")]

mod shared;

use core::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use rasteron::color::{Color, PixelFormat};
use rasteron::programmable::cache::VertexCache;
use rasteron::programmable::shader::{GradientFs, Passthrough2D, SolidFs, TextureFs};
use rasteron::programmable::texture::{FilterMode, Texture2D, WrapMode};
use rasteron::programmable::{
    BlendMode, Flat, FragmentInput, FragmentOutput, FragmentShader, Pipeline, PipelineState,
    PrimitiveType, ScissorRect, Vec2, Vertex2D, Viewport,
};

use shared::{
    CORE_FORMATS, OPAQUE, SIZE_HD, SIZE_SD, TRANSLUCENT, criterion_config, gradient_texture, PixFb,
};

/// 覆盖全部五种 [`BlendMode`] 的取值表。
const BLEND_MODES: [(&str, BlendMode); 5] = [
    ("none", BlendMode::None),
    ("alpha", BlendMode::Alpha),
    ("add", BlendMode::Add),
    ("multiply", BlendMode::Multiply),
    ("premultiplied", BlendMode::PremultipliedAlpha),
];

/// 片元着色器：直接输出 varying 颜色。
///
/// 与内建 `SolidFs` 的区别是颜色来自 varying（逐片元插值），
/// 用于把「插值成本」与「着色器分派成本」分开测量。
#[derive(Debug, Clone, Copy)]
struct VaryingFs;

impl FragmentShader for VaryingFs {
    type Varying = Color;

    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
        FragmentOutput::Color(input.varying)
    }
}

/// 片元着色器：一半片元丢弃。
///
/// `FragmentOutput::Discard` 只影响当前片元，用于测丢弃分支是否真的
/// 省下了写入成本。
#[derive(Debug, Clone, Copy)]
struct DiscardHalfFs;

impl FragmentShader for DiscardHalfFs {
    type Varying = Color;

    fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
        // 按 x 坐标奇偶丢弃：不引入任何额外的数据依赖。
        let bucket = (input.position[0] * 64.0) as u32;
        if bucket % 2 == 0 {
            FragmentOutput::Color(input.varying)
        } else {
            FragmentOutput::Discard
        }
    }
}

/// 构造一张铺满整屏、覆盖全屏的三角形扇（两个三角形组成的四边形）。
fn full_screen_quad() -> [Vertex2D<Color>; 4] {
    [
        Vertex2D { position: [0.0, 0.0], varying: Color::new(255, 0, 0) },
        Vertex2D { position: [1.0, 0.0], varying: Color::new(0, 255, 0) },
        Vertex2D { position: [1.0, 1.0], varying: Color::new(0, 0, 255) },
        Vertex2D { position: [0.0, 1.0], varying: Color::new(255, 255, 0) },
    ]
}

/// 生成 `count` 个随机分布的 `Points` 顶点（归一化坐标）。
fn point_cloud(count: usize) -> Vec<Vertex2D<()>> {
    (0..count)
        .map(|i| {
            // 用整数哈希散布坐标，避免依赖 rand；结果落在 [0, 1)。
            let hash_x = (i.wrapping_mul(2_654_435_761)) % 1024;
            let hash_y = (i.wrapping_mul(40_503)) % 1024;
            Vertex2D {
                position: [hash_x as f32 / 1024.0, hash_y as f32 / 1024.0],
                varying: (),
            }
        })
        .collect()
}

/// 生成 `count` 条线段（归一化坐标），端点散布在全屏。
fn line_list(count: usize) -> Vec<Vertex2D<()>> {
    let mut vertices = Vec::with_capacity(count * 2);
    for i in 0..count {
        let hash_a = (i.wrapping_mul(2_654_435_761)) % 1024;
        let hash_b = (i.wrapping_mul(40_503)) % 1024;
        let hash_c = (i.wrapping_mul(22_695_477)) % 1024;
        let hash_d = (i.wrapping_mul(1_103_515_245)) % 1024;
        vertices.push(Vertex2D { position: [hash_a as f32 / 1024.0, hash_b as f32 / 1024.0], varying: () });
        vertices.push(Vertex2D { position: [hash_c as f32 / 1024.0, hash_d as f32 / 1024.0], varying: () });
    }
    vertices
}

/// `Points` 图元：顶点数扫描。
///
/// 点是最小的图元，成本几乎全部落在「顶点着色 + 浮点可见判定 + 单像素写入」上，
/// 因此它是管线固定开销的基准尺。
fn bench_draw_arrays_points(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_arrays_points");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, SIZE_SD.0, SIZE_SD.1);
    let pipeline = Pipeline::new(
        Passthrough2D::<()>::new(),
        SolidFs::new(OPAQUE),
        PipelineState::alpha_blend(viewport),
    );

    for count in [64usize, 1_024, 16_384] {
        let vertices = point_cloud(count);
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Points, black_box(&vertices), &pipeline)
                    .unwrap();
            });
        });
    }

    group.finish();
}

/// `Lines` 图元：线段数扫描。
///
/// 线段路径含参数域裁剪与**按像素去重**（短线段会有多个采样点映射到同一像素，
/// 不去重则半透明线段比同位置的点更亮）。这组基准的长线段走主路径，
/// 另一组专测短线段去重。
fn bench_draw_arrays_lines(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_arrays_lines");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, SIZE_SD.0, SIZE_SD.1);
    let pipeline = Pipeline::new(
        Passthrough2D::<()>::new(),
        SolidFs::new(TRANSLUCENT),
        PipelineState::alpha_blend(viewport),
    );

    for count in [16usize, 256, 2_048] {
        let vertices = line_list(count);
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Lines, black_box(&vertices), &pipeline)
                    .unwrap();
            });
        });
    }

    group.finish();
}

/// 极短线段：去重路径的压力测试。
///
/// 每条线段的两个端点几乎重合，DDA 采样会多次映射到同一像素。若去重被破坏，
/// 这里的耗时不降反升（重复着色）且半透明结果会变亮——基准与
/// `short_line_shades_each_pixel_once` 测试互为印证。
fn bench_draw_arrays_short_lines(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_arrays_short_lines");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, SIZE_SD.0, SIZE_SD.1);

    let mut vertices = Vec::with_capacity(4_096);
    for i in 0..2_048usize {
        let x = ((i.wrapping_mul(2_654_435_761)) % 1000) as f32 / 1000.0;
        let y = ((i.wrapping_mul(40_503)) % 1000) as f32 / 1000.0;
        // 端点偏移一个亚像素量级：必然多次映射到同一整数像素。
        vertices.push(Vertex2D { position: [x, y], varying: () });
        vertices.push(Vertex2D { position: [x + 0.0003, y + 0.0002], varying: () });
    }

    let pipeline = Pipeline::new(
        Passthrough2D::<()>::new(),
        SolidFs::new(TRANSLUCENT),
        PipelineState::alpha_blend(viewport),
    );

    group.throughput(Throughput::Elements(2_048));
    group.bench_function("2048_subpixel_lines", |b| {
        b.iter(|| {
            fb.renderer
                .draw_arrays(PrimitiveType::Lines, black_box(&vertices), &pipeline)
                .unwrap();
        });
    });

    // 对照组：同样数量但端点完全重合的退化线段，应在参数域裁剪处直接返回。
    let degenerate: Vec<Vertex2D<()>> = (0..4_096)
        .map(|i| Vertex2D { position: [((i % 100) as f32) / 100.0, 0.5], varying: () })
        .collect();
    group.bench_function("2048_zero_length_lines", |b| {
        b.iter(|| {
            fb.renderer
                .draw_arrays(PrimitiveType::Lines, black_box(&degenerate), &pipeline)
                .unwrap();
        });
    });

    group.finish();
}

/// `Triangles` 图元：覆盖整屏的两个三角形。
///
/// 这是管线吞吐的上限场景——每个像素恰好被一个三角形覆盖一次。
/// 分辨率为 640×480 与 1920×1080 两档，用于确认成本随像素数线性扩展。
fn bench_draw_arrays_triangles(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_arrays_triangles");

    for (label, (width, height)) in [("640x480", SIZE_SD), ("1920x1080", SIZE_HD)] {
        let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
        let viewport = Viewport::new(0, 0, width, height);
        // 两个三角形拼成整屏四边形。
        let vertices = [
            Vertex2D { position: [0.0, 0.0], varying: () },
            Vertex2D { position: [1.0, 0.0], varying: () },
            Vertex2D { position: [1.0, 1.0], varying: () },
            Vertex2D { position: [0.0, 0.0], varying: () },
            Vertex2D { position: [1.0, 1.0], varying: () },
            Vertex2D { position: [0.0, 1.0], varying: () },
        ];
        group.throughput(Throughput::Elements((width * height) as u64));

        for (alpha_label, color) in [("opaque", OPAQUE), ("alpha", TRANSLUCENT)] {
            let pipeline = Pipeline::new(
                Passthrough2D::<()>::new(),
                SolidFs::new(color),
                PipelineState::alpha_blend(viewport),
            );
            let id = format!("solid_{label}_{alpha_label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer
                        .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                        .unwrap();
                });
            });
        }
    }

    group.finish();
}

/// 五种混合模式在同一图元上的对比。
///
/// `None` 与 `Alpha` 走的是「不读目标像素」的快速路径（后者还委托给字节层），
/// `Add` / `Multiply` / `PremultipliedAlpha` 必须读-改-写。这组的差值
/// 就是目标读取 + 每像素 blend 的真实成本。
fn bench_blend_modes(c: &mut Criterion) {
    let mut group = c.benchmark_group("blend_modes");
    let (width, height) = SIZE_SD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, width, height);
    let vertices = [
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 1.0], varying: () },
    ];
    group.throughput(Throughput::Elements((width * height) as u64));

    for (label, blend) in BLEND_MODES {
        for (alpha_label, color) in [("opaque", OPAQUE), ("alpha", TRANSLUCENT)] {
            let state = PipelineState { blend, scissor: None, viewport };
            let pipeline = Pipeline::new(Passthrough2D::<()>::new(), SolidFs::new(color), state);
            let id = format!("{label}_{alpha_label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer
                        .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                        .unwrap();
                });
            });
        }
    }

    group.finish();
}

/// 混合模式 × 像素格式：验证 `Alpha` 模式的字节层委托在不同格式上的成本。
///
/// 4bpp 格式在热路径里由 `set_pixel_unchecked` 直接落盘，565 需要
/// 解码-混合-量化，3bpp 跨字节错位逐通道回退。
fn bench_blend_modes_formats(c: &mut Criterion) {
    let mut group = c.benchmark_group("blend_modes_formats");
    let (width, height) = SIZE_SD;
    let vertices = [
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 1.0], varying: () },
    ];
    group.throughput(Throughput::Elements((width * height) as u64));

    for format in CORE_FORMATS {
        let mut fb = PixFb::new(width, height, format);
        let viewport = Viewport::new(0, 0, width, height);
        let pipeline = Pipeline::new(
            Passthrough2D::<()>::new(),
            SolidFs::new(TRANSLUCENT),
            PipelineState::alpha_blend(viewport),
        );
        group.bench_with_input(BenchmarkId::from_parameter(format!("{format:?}")), &format, |b, _| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                    .unwrap();
            });
        });
    }

    group.finish();
}

/// varying 插值：`()` / `Color` / `Vec2` / `Flat<Color>` 四种属性宽度。
///
/// 顶点数据宽度不同 → 顶点缓冲访存与插值算量不同。`Flat` 恒取第一个顶点，
/// 不做加权求和，因此应与真实插值有明显差距。
fn bench_varying_types(c: &mut Criterion) {
    let mut group = c.benchmark_group("varying_types");
    let (width, height) = SIZE_SD;
    let viewport = Viewport::new(0, 0, width, height);
    group.throughput(Throughput::Elements((width * height) as u64));

    let positions = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

    // 零属性：`SolidFs` 的 Varying = ()。
    {
        let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
        let vertices: Vec<Vertex2D<()>> =
            positions.iter().map(|&position| Vertex2D { position, varying: () }).collect();
        let pipeline = Pipeline::new(
            Passthrough2D::<()>::new(),
            SolidFs::new(OPAQUE),
            PipelineState::alpha_blend(viewport),
        );
        group.bench_function("unit", |b| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                    .unwrap();
            });
        });
    }

    // Color varying：逐通道插值。
    {
        let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
        let quad = full_screen_quad();
        let vertices = [quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]];
        let pipeline = Pipeline::new(
            Passthrough2D::<Color>::new(),
            VaryingFs,
            PipelineState::alpha_blend(viewport),
        );
        group.bench_function("color", |b| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                    .unwrap();
            });
        });
    }

    // Vec2 varying：两分量插值（纹理 UV 的典型形态）。
    {
        let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
        let uv = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let vertices: Vec<Vertex2D<Vec2>> = positions
            .iter()
            .zip(uv)
            .map(|(&position, varying)| Vertex2D { position, varying })
            .collect();
        // 用一个只输出 UV 转灰度的着色器，避免混入纹理采样成本。
        #[derive(Debug, Clone, Copy)]
        struct UvFs;
        impl FragmentShader for UvFs {
            type Varying = Vec2;
            fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
                let u = (input.varying[0] * 255.0) as u8;
                let v = (input.varying[1] * 255.0) as u8;
                FragmentOutput::Color(Color::with_alpha(u, v, 128, 255))
            }
        }
        let pipeline =
            Pipeline::new(Passthrough2D::<Vec2>::new(), UvFs, PipelineState::alpha_blend(viewport));
        group.bench_function("vec2", |b| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                    .unwrap();
            });
        });
    }

    // Flat varying：恒取 provoking vertex，不做加权求和。
    {
        let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
        let vertices: Vec<Vertex2D<Flat<Color>>> = positions
            .iter()
            .map(|&position| Vertex2D { position, varying: Flat(OPAQUE) })
            .collect();

        #[derive(Debug, Clone, Copy)]
        struct FlatFs;
        impl FragmentShader for FlatFs {
            type Varying = Flat<Color>;
            fn shade_fragment(&self, input: FragmentInput<Self::Varying>) -> FragmentOutput {
                FragmentOutput::Color(input.varying.0)
            }
        }

        let pipeline = Pipeline::new(
            Passthrough2D::<Flat<Color>>::new(),
            FlatFs,
            PipelineState::alpha_blend(viewport),
        );
        group.bench_function("flat_color", |b| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                    .unwrap();
            });
        });
    }

    group.finish();
}

/// 片元丢弃：`FragmentOutput::Discard` 是否真的省下写入。
fn bench_fragment_discard(c: &mut Criterion) {
    let mut group = c.benchmark_group("fragment_discard");
    let (width, height) = SIZE_SD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, width, height);
    let quad = full_screen_quad();
    let vertices = [quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]];
    group.throughput(Throughput::Elements((width * height) as u64));

    let keep = Pipeline::new(
        Passthrough2D::<Color>::new(),
        VaryingFs,
        PipelineState::alpha_blend(viewport),
    );
    group.bench_function("keep_all", |b| {
        b.iter(|| {
            fb.renderer
                .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &keep)
                .unwrap();
        });
    });

    let discard = Pipeline::new(
        Passthrough2D::<Color>::new(),
        DiscardHalfFs,
        PipelineState::alpha_blend(viewport),
    );
    group.bench_function("discard_half", |b| {
        b.iter(|| {
            fb.renderer
                .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &discard)
                .unwrap();
        });
    });

    group.finish();
}

/// 视口与裁剪：视口尺寸与 scissor 对片元数量的影响。
///
/// `visible_bounds` = 帧缓冲 ∩ viewport ∩ scissor。视口缩小应让成本按面积下降；
/// 交集为空时应**连顶点着色都不执行**，成本塌到接近零。
fn bench_viewport_scissor(c: &mut Criterion) {
    let mut group = c.benchmark_group("viewport_scissor");
    let (width, height) = SIZE_SD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let quad = full_screen_quad();
    let vertices = [quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]];
    let pipeline = Pipeline::new(
        Passthrough2D::<Color>::new(),
        VaryingFs,
        PipelineState::alpha_blend(Viewport::new(0, 0, width, height)),
    );

    for (label, viewport) in [
        ("full", Viewport::new(0, 0, width, height)),
        ("half", Viewport::new(0, 0, width / 2, height / 2)),
        ("quarter", Viewport::new(0, 0, width / 4, height / 4)),
        ("offset", Viewport::new(100, 100, 400, 300)),
        // 空视口：可见矩形为空，绘制调用应直接成功返回。
        ("empty", Viewport::new(0, 0, 0, 0)),
    ] {
        let mut state = pipeline.state;
        state.viewport = viewport;
        let pipeline_local = Pipeline::new(Passthrough2D::<Color>::new(), VaryingFs, state);
        group.bench_with_input(BenchmarkId::from_parameter(label), &label, |b, _| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline_local)
                    .unwrap();
            });
        });
    }

    // scissor 裁在着色器之前：半屏 scissor 应显著低于全屏。
    for (label, rect) in [
        ("scissor_half", ScissorRect::new(0, 0, width / 2, height)),
        ("scissor_tiny", ScissorRect::new(0, 0, 32, 32)),
        ("scissor_outside", ScissorRect::new(width + 10, 0, 100, 100)),
    ] {
        let mut state = pipeline.state;
        state.scissor = Some(rect);
        let pipeline_local = Pipeline::new(Passthrough2D::<Color>::new(), VaryingFs, state);
        group.bench_with_input(BenchmarkId::from_parameter(label), &label, |b, _| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline_local)
                    .unwrap();
            });
        });
    }

    let _ = &pipeline;
    group.finish();
}

/// 索引绘制：有缓存与无缓存的对比。
///
/// 用一条三角形带：顶点少、引用多。缓存命中时顶点着色只执行一次，
/// 故缓存版应明显更快；`None` 版本每次都重新着色，是零分配基线。
///
/// 索引规模取 16 个重复四边形（32 个三角形）。本组的目的是暴露
/// **顶点着色次数**的差异，而不是光栅化吞吐——每个三角形都覆盖整屏，
/// 再多个几倍就变成纯填屏基准（单条迭代要几百毫秒），
/// 让 criterion 为了凑够采样数跑上几分钟，反而不如把规模压小、
/// 让它在合理时间内给出更紧的区间。
fn bench_draw_indexed_cache(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_indexed_cache");
    let (width, height) = SIZE_SD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, width, height);
    group.throughput(Throughput::Elements((width * height) as u64));

    // 覆盖整屏的 4 顶点四边形，索引反复引用它们。
    let quad = full_screen_quad();
    let quads = 16u16;
    let indices: Vec<u16> = (0..quads).flat_map(|_| [0u16, 1, 2, 0, 2, 3]).collect();
    let pipeline = Pipeline::new(
        Passthrough2D::<Color>::new(),
        VaryingFs,
        PipelineState::alpha_blend(viewport),
    );

    group.bench_function("no_cache", |b| {
        b.iter(|| {
            fb.renderer
                .draw_indexed(
                    PrimitiveType::Triangles,
                    black_box(&quad),
                    black_box(&indices),
                    &pipeline,
                    None,
                )
                .unwrap();
        });
    });

    let mut slots: [Option<FragmentInput<Color>>; 4] = [None; 4];
    let mut cache = VertexCache::new(&mut slots);
    group.bench_function("with_cache", |b| {
        b.iter(|| {
            fb.renderer
                .draw_indexed(
                    PrimitiveType::Triangles,
                    black_box(&quad),
                    black_box(&indices),
                    &pipeline,
                    Some(&mut cache),
                )
                .unwrap();
        });
    });

    // 缓存清空后重新着色：模拟顶点数据每帧变化的场景。
    group.bench_function("with_cache_cleared_each_iter", |b| {
        b.iter(|| {
            cache.clear();
            fb.renderer
                .draw_indexed(
                    PrimitiveType::Triangles,
                    black_box(&quad),
                    black_box(&indices),
                    &pipeline,
                    Some(&mut cache),
                )
                .unwrap();
        });
    });

    group.finish();
}

/// 索引越界：整批校验先于任何绘制的成本。
///
/// 契约是「只要有一个索引越界，整次调用不产生任何片元」，
/// 故这组基准应稳定在一个只做线性扫描的低位。
fn bench_draw_indexed_oob(c: &mut Criterion) {
    let mut group = c.benchmark_group("draw_indexed_oob");
    let mut fb = PixFb::new(SIZE_SD.0, SIZE_SD.1, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, SIZE_SD.0, SIZE_SD.1);
    let quad = full_screen_quad();
    let pipeline = Pipeline::new(
        Passthrough2D::<Color>::new(),
        VaryingFs,
        PipelineState::alpha_blend(viewport),
    );

    for count in [6usize, 600, 6_000] {
        // 全部合法索引 + 末尾一个越界索引。
        let mut indices: Vec<u16> = (0..count as u16 - 1).map(|i| i % 4).collect();
        indices.push(99);
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                let result = fb.renderer.draw_indexed(
                    PrimitiveType::Triangles,
                    black_box(&quad),
                    black_box(&indices),
                    &pipeline,
                    None,
                );
                black_box(result.is_err());
            });
        });
    }

    group.finish();
}

/// 内建片元着色器：`SolidFs` 与 `GradientFs` 的对比。
///
/// 两者都是 `Varying = ()`（零属性顶点），差别只在片元阶段：
/// `SolidFs` 返回常量，`GradientFs` 做一次点积 + `Color::lerp`。
fn bench_builtin_shaders(c: &mut Criterion) {
    let mut group = c.benchmark_group("builtin_shaders");
    let (width, height) = SIZE_SD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, width, height);
    group.throughput(Throughput::Elements((width * height) as u64));

    let vertices = [
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 0.0], varying: () },
        Vertex2D { position: [1.0, 1.0], varying: () },
        Vertex2D { position: [0.0, 1.0], varying: () },
    ];

    let solid = Pipeline::new(
        Passthrough2D::<()>::new(),
        SolidFs::new(OPAQUE),
        PipelineState::alpha_blend(viewport),
    );
    group.bench_function("solid_fs", |b| {
        b.iter(|| {
            fb.renderer
                .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &solid)
                .unwrap();
        });
    });

    for (label, dir) in [("horizontal", [1.0f32, 0.0]), ("diagonal", [1.0, 1.0])] {
        let gradient = Pipeline::new(
            Passthrough2D::<()>::new(),
            GradientFs::new(Color::new(0, 0, 0), Color::new(255, 255, 255), dir),
            PipelineState::alpha_blend(viewport),
        );
        group.bench_with_input(BenchmarkId::from_parameter(label), &label, |b, _| {
            b.iter(|| {
                fb.renderer
                    .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &gradient)
                    .unwrap();
            });
        });
    }

    group.finish();
}

/// 纹理采样：4 种过滤 × 寻址组合的直接调用。
///
/// 采样是纯粹的「坐标 → texel 解码」，不含绘制开销，因此这组数字
/// 就是纹理管线的下限。半 texel 约定与手写 `floor_to_i32` 都在这里。
fn bench_texture_sample(c: &mut Criterion) {
    let mut group = c.benchmark_group("texture_sample");

    // 纹理尺寸只取两档：采样成本主要由「是否跨 texel 边界」与寻址模式决定，
    // 与纹理总像素数关系不大（无 mipmap），中间档位信息量低。
    for side in [4usize, 512] {
        let texels = gradient_texture(side, side);
        let texture = Texture2D::new(side, side, &texels, PixelFormat::Rgba8888);

        // 覆盖纹理内部与边界外的 UV，确保 clamp/repeat 的边界分支被走到。
        let uvs: Vec<Vec2> = (0..1024usize)
            .map(|i| {
                let u = ((i.wrapping_mul(2_654_435_761)) % 1024) as f32 / 1024.0;
                let v = ((i.wrapping_mul(40_503)) % 1024) as f32 / 1024.0;
                // 约 1/4 的采样落在 [0,1) 之外，触发寻址分支。
                if i % 4 == 0 {
                    [u * 1.6 - 0.3, v * 1.6 - 0.3]
                } else {
                    [u, v]
                }
            })
            .collect();

        group.throughput(Throughput::Elements(uvs.len() as u64));

        for (label, filter, wrap) in [
            ("nearest_clamp", FilterMode::Nearest, WrapMode::Clamp),
            ("bilinear_clamp", FilterMode::Bilinear, WrapMode::Clamp),
            ("nearest_repeat", FilterMode::Nearest, WrapMode::Repeat),
            ("bilinear_repeat", FilterMode::Bilinear, WrapMode::Repeat),
        ] {
            let id = format!("side{side}_{label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    let mut acc = 0u32;
                    for &uv in &uvs {
                        let color = texture.sample(black_box(uv), filter, wrap);
                        acc = acc.wrapping_add(color.to_rgba_u32());
                    }
                    black_box(acc);
                });
            });
        }
    }

    group.finish();
}

/// 纹理采样的便捷包装：确认它们与 `sample` 等价（应无额外开销）。
fn bench_texture_sample_helpers(c: &mut Criterion) {
    let mut group = c.benchmark_group("texture_sample_helpers");
    let side = 64usize;
    let texels = gradient_texture(side, side);
    let texture = Texture2D::new(side, side, &texels, PixelFormat::Rgba8888);
    let uv: Vec2 = [0.37, 0.62];

    group.bench_function("sample_nearest", |b| {
        b.iter(|| black_box(black_box(&texture).sample_nearest(black_box(uv))));
    });
    group.bench_function("sample_bilinear", |b| {
        b.iter(|| black_box(black_box(&texture).sample_bilinear(black_box(uv))));
    });
    group.bench_function("sample_nearest_repeat", |b| {
        b.iter(|| black_box(black_box(&texture).sample_nearest_repeat(black_box(uv))));
    });
    group.bench_function("sample_bilinear_repeat", |b| {
        b.iter(|| black_box(black_box(&texture).sample_bilinear_repeat(black_box(uv))));
    });

    group.finish();
}

/// 零尺寸与 NaN 采样：边界短路路径。
///
/// 零尺寸纹理的任何采样都必须先短路返回透明黑，否则 `limit - 1` 会下溢。
/// NaN/∞ 坐标同样先被 `is_finite` 拦下。
fn bench_texture_sample_degenerate(c: &mut Criterion) {
    let mut group = c.benchmark_group("texture_sample_degenerate");

    let empty = Texture2D::new(0, 0, &[], PixelFormat::Rgba8888);
    group.bench_function("zero_sized", |b| {
        b.iter(|| {
            black_box(black_box(&empty).sample([0.5, 0.5], FilterMode::Bilinear, WrapMode::Repeat));
        });
    });

    let texels = gradient_texture(8, 8);
    let texture = Texture2D::new(8, 8, &texels, PixelFormat::Rgba8888);
    group.bench_function("nan_uv", |b| {
        b.iter(|| {
            black_box(black_box(&texture).sample([f32::NAN, 0.5], FilterMode::Nearest, WrapMode::Clamp));
        });
    });
    group.bench_function("infinite_uv", |b| {
        b.iter(|| {
            black_box(black_box(&texture).sample(
                [f32::INFINITY, f32::NEG_INFINITY],
                FilterMode::Bilinear,
                WrapMode::Repeat,
            ));
        });
    });

    group.finish();
}

/// 带纹理的完整绘制：`TextureFs` 经由 varying UV 采样。
///
/// 这是纹理管线唯一的端到端场景——顶点传 UV、片元采样、按混合模式落盘，
/// 因此它同时覆盖插值与采样两条路径。
fn bench_texture_draw(c: &mut Criterion) {
    let mut group = c.benchmark_group("texture_draw");
    let (width, height) = SIZE_SD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, width, height);
    group.throughput(Throughput::Elements((width * height) as u64));

    for side in [8usize, 128] {
        let texels = gradient_texture(side, side);
        let texture = Texture2D::new(side, side, &texels, PixelFormat::Rgba8888);

        for (label, filter) in [("nearest", FilterMode::Nearest), ("bilinear", FilterMode::Bilinear)] {
            let pipeline = Pipeline::new(
                Passthrough2D::<Vec2>::new(),
                TextureFs::new(texture, filter, WrapMode::Clamp),
                PipelineState::alpha_blend(viewport),
            );
            // UV 覆盖整张纹理；`position` 覆盖整屏。
            let vertices = [
                Vertex2D { position: [0.0, 0.0], varying: [0.0, 0.0] },
                Vertex2D { position: [1.0, 0.0], varying: [1.0, 0.0] },
                Vertex2D { position: [1.0, 1.0], varying: [1.0, 1.0] },
                Vertex2D { position: [0.0, 0.0], varying: [0.0, 0.0] },
                Vertex2D { position: [1.0, 1.0], varying: [1.0, 1.0] },
                Vertex2D { position: [0.0, 1.0], varying: [0.0, 1.0] },
            ];

            let id = format!("tex{side}_{label}");
            group.bench_with_input(BenchmarkId::from_parameter(&id), &id, |b, _| {
                b.iter(|| {
                    fb.renderer
                        .draw_arrays(PrimitiveType::Triangles, black_box(&vertices), &pipeline)
                        .unwrap();
                });
            });
        }
    }

    group.finish();
}

/// 端到端场景：一帧完整画面的绘制成本。
///
/// 把固定功能图元与可编程管线混用在同一帧缓冲上，模拟真实渲染循环。
/// 这组数字不像单项基准那样可归因，但它是「一帧要多久」的直接答案。
fn bench_frame_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_mixed");
    let (width, height) = SIZE_HD;
    let mut fb = PixFb::new(width, height, PixelFormat::Rgba8888);
    let viewport = Viewport::new(0, 0, width, height);
    // `TextureFs` 的 Varying 是 Vec2，顶点带 UV；这里用内建纹理着色器
    // 让「一帧」里真的包含纹理采样这条链路。
    let texels = gradient_texture(64, 64);
    let texture = Texture2D::new(64, 64, &texels, PixelFormat::Rgba8888);
    let pipeline = Pipeline::new(
        Passthrough2D::<Vec2>::new(),
        TextureFs::new(texture, FilterMode::Nearest, WrapMode::Clamp),
        PipelineState::alpha_blend(viewport),
    );
    let mask = crate::shared::antialiased_mask(64, 64);

    group.throughput(Throughput::Elements((width * height) as u64));
    group.bench_function("1920x1080_mixed_frame", |b| {
        b.iter(|| {
            // 1. 清屏
            fb.renderer.clear(Color::new(18, 18, 24));
            // 2. 固定功能：网格线
            for i in 0..16usize {
                let offset = i * 120;
                fb.renderer.draw_line(
                    rasteron::Pixel::new(offset, 0),
                    rasteron::Pixel::new(offset, height - 1),
                    &Color::with_alpha(60, 60, 70, 255),
                );
                fb.renderer.draw_line(
                    rasteron::Pixel::new(0, offset),
                    rasteron::Pixel::new(width - 1, offset),
                    &Color::with_alpha(60, 60, 70, 255),
                );
            }
            // 3. 固定功能：几个填充图元
            fb.renderer.fill_circle(rasteron::Pixel::new(400, 300), 160, Color::with_alpha(200, 60, 40, 200));
            fb.renderer.fill_rect(rasteron::Pixel::new(900, 200), 400, 300, Color::with_alpha(40, 120, 200, 180));
            fb.renderer.fill_arc(
                rasteron::Pixel::new(1400, 700),
                220,
                rasteron::Angle::ZERO,
                rasteron::Angle::RIGHT_DOWN,
                Color::with_alpha(240, 200, 60, 190),
            );
            // 4. 精灵：掩码混合
            for i in 0..24usize {
                fb.renderer.blit_mask(
                    rasteron::Pixel::new((i * 79) % (width - 64), (i * 137) % (height - 64)),
                    64,
                    64,
                    &mask,
                    64,
                    Color::with_alpha(255, 255, 255, 220),
                );
            }
            // 5. 可编程管线：一批三角形
            let mut vertices = Vec::with_capacity(300);
            for i in 0..100usize {
                let base_x = ((i * 191) % 1000) as f32 / 1000.0;
                let base_y = ((i * 313) % 1000) as f32 / 1000.0;
                vertices.push(Vertex2D { position: [base_x, base_y], varying: [0.0, 0.0] });
                vertices.push(Vertex2D { position: [base_x + 0.05, base_y], varying: [1.0, 0.0] });
                vertices.push(Vertex2D { position: [base_x + 0.025, base_y + 0.05], varying: [0.5, 1.0] });
            }
            fb.renderer
                .draw_arrays(PrimitiveType::Triangles, &vertices, &pipeline)
                .unwrap();
        });
    });

    group.finish();
}

criterion_group!(
    name = programmable_pipeline;
    config = criterion_config();
    targets =
    bench_draw_arrays_points,
    bench_draw_arrays_lines,
    bench_draw_arrays_short_lines,
    bench_draw_arrays_triangles,
    bench_blend_modes,
    bench_blend_modes_formats,
    bench_varying_types,
    bench_fragment_discard,
    bench_viewport_scissor,
    bench_draw_indexed_cache,
    bench_draw_indexed_oob,
    bench_builtin_shaders,
    bench_texture_sample,
    bench_texture_sample_helpers,
    bench_texture_sample_degenerate,
    bench_texture_draw,
    bench_frame_mixed
);
fn main() {
    // 先量一次本机抖动：它决定本次数字是否值得采信。
    shared::report_jitter();
    // `criterion_config()` 给的是本项目统一的参数，`configure_from_args()`
    // 再叠加命令行覆盖（`--quick`、`--sample-size` 等），后者优先。
    programmable_pipeline();
}
