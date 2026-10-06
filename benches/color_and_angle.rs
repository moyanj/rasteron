//! 颜色、像素格式编解码与角度三角函数的基准。
//!
//! 覆盖的公开面：
//!
//! * `color`：`Color::{new, with_alpha, from_hex, to_rgba_u32, from_rgba_u32,
//!   to_bytes, from_bytes, mix_alpha, blend, invert}`
//! * `color::PixelFormat::{write_color, read_color, blend_channel, *_shift,
//!   bytes_per_pixel, bits_per_pixel}`
//! * `angle`：`Angle::{from_degrees, from_millidegrees, degrees, sin, cos,
//!   sin_cos, raw, from_raw}` 与 `Wedge::{new, contains, is_full}`
//!
//! 这一层是内核态与嵌入式场景的关键路径：角度用圈值 + 257 项波表而非浮点，
//! 颜色混合用整数 SWAR 而非浮点，所以它们的成本直接决定「无浮点契约」下
//! 图元还能有多快。

mod shared;

use core::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use rasteron::color::{Color, PixelFormat};
use rasteron::angle::{Angle, Wedge};

use shared::criterion_config;

/// 颜色构造与通道访问。
fn bench_color_construct(c: &mut Criterion) {
    let mut group = c.benchmark_group("color_construct");

    group.bench_function("new", |b| {
        b.iter(|| {
            let color = Color::new(black_box(12), black_box(200), black_box(77));
            black_box(color);
        });
    });

    group.bench_function("with_alpha", |b| {
        b.iter(|| {
            let color = Color::with_alpha(black_box(12), black_box(200), black_box(77), black_box(128));
            black_box(color);
        });
    });

    group.bench_function("from_rgba_u32", |b| {
        b.iter(|| {
            let color = Color::from_rgba_u32(black_box(0x78_C8_4D_80));
            black_box(color);
        });
    });

    group.bench_function("to_rgba_u32", |b| {
        let color = Color::with_alpha(12, 200, 77, 128);
        b.iter(|| {
            black_box(black_box(color).to_rgba_u32());
        });
    });

    group.finish();
}

/// 十六进制解析：命中路径与两类错误路径。
///
/// 错误路径（长度非法 / 含非法字符）在真实代码里是配置与资源解析的入口，
/// 每次都只应做一次线性扫描。
fn bench_color_from_hex(c: &mut Criterion) {
    let mut group = c.benchmark_group("color_from_hex");

    group.bench_function("rgb6_valid", |b| {
        b.iter(|| {
            black_box(Color::from_hex(black_box("#78C84D")).unwrap());
        });
    });

    group.bench_function("rgba8_valid", |b| {
        b.iter(|| {
            black_box(Color::from_hex(black_box("#78C84D80")).unwrap());
        });
    });

    group.bench_function("without_hash", |b| {
        b.iter(|| {
            black_box(Color::from_hex(black_box("78C84D")).unwrap());
        });
    });

    group.bench_function("err_invalid_length", |b| {
        b.iter(|| {
            black_box(Color::from_hex(black_box("#78C8")).unwrap_err());
        });
    });

    group.bench_function("err_invalid_digit", |b| {
        b.iter(|| {
            black_box(Color::from_hex(black_box("#ZZZZZZ")).unwrap_err());
        });
    });

    group.finish();
}

/// 颜色混合与逐通道运算。
///
/// `blend` 是 `BlendMode::Alpha` 的整数权威实现（每通道 `(v + 255) >> 8`），
/// `mix_alpha` 只改 alpha，`invert` 做通道取反。
fn bench_color_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("color_ops");
    let src = Color::with_alpha(200, 100, 50, 128);
    let dst = Color::with_alpha(20, 220, 140, 255);

    group.bench_function("blend", |b| {
        b.iter(|| {
            black_box(black_box(&src).blend(black_box(&dst), black_box(128)));
        });
    });

    group.bench_function("mix_alpha", |b| {
        b.iter(|| {
            black_box(black_box(&src).mix_alpha(black_box(64)));
        });
    });

    group.bench_function("invert", |b| {
        b.iter(|| {
            black_box(black_box(&src).invert());
        });
    });

    // `blend_channel` 是最内层的单通道混合，直接暴露整数混合的分子运算。
    group.bench_function("blend_channel", |b| {
        b.iter(|| {
            black_box(PixelFormat::blend_channel(
                black_box(200),
                black_box(20),
                black_box(128),
                black_box(127),
            ));
        });
    });

    // `lerp` 是 varying 插值与 `GradientFs` 共用的权威插值口径。
    // 它定义在 `programmable` 模块旁边（`impl Interpolate for Color` 的
    // inherent 包装），故随该 feature 门控。
    #[cfg(feature = "programmable")]
    group.bench_function("lerp", |b| {
        b.iter(|| {
            black_box(Color::lerp(black_box(src), black_box(dst), black_box(0.37)));
        });
    });

    group.finish();
}

/// 编码与解码的代表性格式覆盖。
///
/// 编码路径含 3bpp / 存 alpha 的 4bpp / 不存 alpha 的 4bpp / 565 四类分支，
/// 解码同理。只取每类一个代表而不穷举 8 种：同类内两个成员的代码路径
/// 只在移位常量上不同，重复测量得不到新信息。
fn bench_color_codec(c: &mut Criterion) {
    let mut group = c.benchmark_group("color_codec");
    let color = Color::with_alpha(200, 100, 50, 128);

    for format in [
        PixelFormat::Bgr24,
        PixelFormat::Rgba8888,
        PixelFormat::Xrgb8888,
        PixelFormat::Rgb565,
    ] {
        group.bench_with_input(BenchmarkId::new("to_bytes", format!("{format:?}")), &format, |b, _| {
            b.iter(|| {
                black_box(black_box(&color).to_bytes(black_box(format)));
            });
        });

        let encoded = color.to_bytes(format);
        group.bench_with_input(BenchmarkId::new("from_bytes", format!("{format:?}")), &format, |b, _| {
            b.iter(|| {
                black_box(Color::from_bytes(black_box(format), black_box(&encoded)));
            });
        });

        // 直接经格式的权威入口编解码（`to_bytes` / `from_bytes` 内部即转发到它们）。
        let mut dst = [0u8; 4];
        group.bench_with_input(
            BenchmarkId::new("write_color", format!("{format:?}")),
            &format,
            |b, _| {
                b.iter(|| {
                    black_box(format).write_color(black_box(&mut dst), black_box(&color));
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("read_color", format!("{format:?}")),
            &format,
            |b, _| {
                b.iter(|| {
                    black_box(black_box(format).read_color(black_box(&encoded)));
                });
            },
        );
    }

    group.finish();
}

/// 格式元数据查询：移位量与位宽。
///
/// 这些是 `const fn`，在真实绘制路径中已被 `Renderer` 的 `fmt_*` 缓存吸收，
/// 单独测量它们是为了给出「若未缓存会付出多少」的参照值。
fn bench_format_metadata(c: &mut Criterion) {
    let mut group = c.benchmark_group("format_metadata");

    // 这类查询全是 `const fn`，真实绘制路径已被 `Renderer` 的 `fmt_*` 缓存
    // 吸收，故本组只作为「若不缓存会付出多少」的参照值——取两个格式即可，
    // 穷举 8 种只是把同一个常量折叠结果测 8 遍。
    for format in [PixelFormat::Rgba8888, PixelFormat::Rgb565] {
        group.bench_with_input(BenchmarkId::new("shifts", format!("{format:?}")), &format, |b, _| {
            b.iter(|| {
                let format = black_box(format);
                black_box((
                    format.red_shift(),
                    format.green_shift(),
                    format.blue_shift(),
                    format.alpha_shift(),
                ));
            });
        });

        group.bench_with_input(BenchmarkId::new("sizes", format!("{format:?}")), &format, |b, _| {
            b.iter(|| {
                let format = black_box(format);
                black_box((format.bytes_per_pixel(), format.bits_per_pixel()));
            });
        });
    }

    group.finish();
}

/// 角度构造与转换。
///
/// 圈值表示（整圈 = 2^32）使回绕成为免费操作；`from_degrees` 与
/// `from_millidegrees` 做的是整数乘加，`degrees` 做四舍五入回绕。
fn bench_angle_construct(c: &mut Criterion) {
    let mut group = c.benchmark_group("angle_construct");

    group.bench_function("from_degrees", |b| {
        b.iter(|| {
            black_box(Angle::from_degrees(black_box(217)));
        });
    });

    group.bench_function("from_millidegrees", |b| {
        b.iter(|| {
            black_box(Angle::from_millidegrees(black_box(217_500)));
        });
    });

    // 超过一整圈：验证回绕是纯整型运算而非取模。
    group.bench_function("from_degrees_wrapping", |b| {
        b.iter(|| {
            black_box(Angle::from_degrees(black_box(3_600_217)));
        });
    });

    group.bench_function("degrees", |b| {
        let angle = Angle::from_millidegrees(217_500);
        b.iter(|| {
            black_box(black_box(angle).degrees());
        });
    });

    group.bench_function("from_raw_roundtrip", |b| {
        b.iter(|| {
            black_box(Angle::from_raw(black_box(0x1234_5678)).raw());
        });
    });

    group.finish();
}

/// 三角函数：257 项四分之一波表 + 线性插值。
///
/// 这是「无浮点契约」下角度运算的核心。扫描一整圈能覆盖四个象限与插值边界，
/// 而不是只测单个角度的分支预测友好情况。
fn bench_angle_trig(c: &mut Criterion) {
    let mut group = c.benchmark_group("angle_trig");
    // 一整圈的 4096 个采样角度，步进覆盖所有象限。
    let angles: Vec<Angle> = (0..4096u32).map(|i| Angle::from_raw(i * (1 << 20))).collect();

    group.throughput(Throughput::Elements(angles.len() as u64));

    group.bench_function("sin_sweep", |b| {
        b.iter(|| {
            let mut acc = 0i64;
            for &angle in &angles {
                acc = acc.wrapping_add(angle.sin());
            }
            black_box(acc);
        });
    });

    group.bench_function("cos_sweep", |b| {
        b.iter(|| {
            let mut acc = 0i64;
            for &angle in &angles {
                acc = acc.wrapping_add(angle.cos());
            }
            black_box(acc);
        });
    });

    // `sin_cos` 一次算两个，应显著优于分别调用两次。
    group.bench_function("sin_cos_sweep", |b| {
        b.iter(|| {
            let mut acc = 0i64;
            for &angle in &angles {
                let (sin, cos) = angle.sin_cos();
                acc = acc.wrapping_add(sin).wrapping_add(cos);
            }
            black_box(acc);
        });
    });

    group.finish();
}

/// 楔形判定：`Wedge::contains` 的跨度与判定次数扫描。
///
/// `contains` 每次只做 1~2 次叉积。跨度 > 180° 时内部拆成两个子楔形取并集，
/// 因此「小跨度」「大跨度」「整圈」三档的成本不同；整圈由 `is_full()` 短路。
fn bench_wedge(c: &mut Criterion) {
    let mut group = c.benchmark_group("wedge");

    for (label, start, end, is_full) in [
        ("span_30deg", Angle::ZERO, Angle::from_degrees(30), false),
        ("span_90deg", Angle::ZERO, Angle::RIGHT_DOWN, false),
        ("span_200deg", Angle::ZERO, Angle::from_degrees(200), false),
        ("span_300deg", Angle::ZERO, Angle::from_degrees(300), false),
    ] {
        let wedge = Wedge::new(start, end);
        assert_eq!(wedge.is_full(), is_full);

        // 待判定的偏移量：绕圆一周的 256 个方向，半径 100。
        let probes: Vec<(i64, i64)> = (0..256)
            .map(|i| {
                let angle = Angle::from_degrees((i * 360) / 256);
                let (sin, cos) = angle.sin_cos();
                // sin/cos 放大 SIN_SCALE 倍，故右移回像素尺度。
                ((cos * 100) >> 15, (sin * 100) >> 15)
            })
            .collect();

        group.throughput(Throughput::Elements(probes.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &label, |b, _| {
            b.iter(|| {
                let mut hits = 0u32;
                for &(dx, dy) in &probes {
                    if wedge.contains(black_box(dx), black_box(dy)) {
                        hits += 1;
                    }
                }
                black_box(hits);
            });
        });
    }

    group.finish();
}

/// 整圈短路：`is_full` 与 `Wedge::new` 本身的成本。
///
/// `draw_arc` / `fill_arc` 在每次调用开头都会构造楔形并判整圈，
/// 这是所有弧形绘制的固定前置开销。
fn bench_wedge_construct(c: &mut Criterion) {
    let mut group = c.benchmark_group("wedge_construct");

    group.bench_function("new_and_is_full", |b| {
        b.iter(|| {
            let wedge = Wedge::new(black_box(Angle::from_degrees(30)), black_box(Angle::from_degrees(120)));
            black_box(wedge.is_full());
        });
    });

    group.bench_function("new_full_circle", |b| {
        b.iter(|| {
            let wedge = Wedge::new(black_box(Angle::ZERO), black_box(Angle::ZERO));
            black_box(wedge.is_full());
        });
    });

    group.finish();
}

criterion_group!(
    name = color_and_angle;
    config = criterion_config();
    targets =
    bench_color_construct,
    bench_color_from_hex,
    bench_color_ops,
    bench_color_codec,
    bench_format_metadata,
    bench_angle_construct,
    bench_angle_trig,
    bench_wedge,
    bench_wedge_construct
);
fn main() {
    // 先量一次本机抖动：它决定本次数字是否值得采信。
    shared::report_jitter();
    // `criterion_config()` 给的是本项目统一的参数，`configure_from_args()`
    // 再叠加命令行覆盖（`--quick`、`--sample-size` 等），后者优先。
    color_and_angle();
}
