# rasteron

**`no_std`、零依赖、零分配的 2D 软件光栅化库**——直接画进你已有的帧缓冲，不碰 FPU（可选），不碰分配器。

`rasteron` 把像素、线段、矩形、三角形、圆、圆弧、扇形、位图拷贝和文本直接写进一块线性内存。固定功能路径全程整数运算，不依赖 `std`、不需要全局分配器、不引入任何运行时依赖。

![rasteron gallery](docs/images/gallery.png)

> 上图由 `cargo run --example gallery` 生成：12 格编号网格，每格一个图元，右下角是全部 8 种像素格式的色条。

## 30 秒上手

```rust
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::{Pixel, Renderer};

let format = PixelFormat::Bgr24;
let info = FramebufferInfo::new(320, 240, 320 * format.bytes_per_pixel(), format);

let mut framebuffer = vec![0u8; info.buffer_size()];   // 缓冲区由你提供
let mut renderer = Renderer::new(info, &mut framebuffer);

renderer.clear(Color::new(20, 24, 32));
renderer.fill_circle(Pixel::new(160, 120), 48, Color::new(40, 160, 240));
renderer.draw_line(
    Pixel::new(0, 0),
    Pixel::new(319, 239),
    &Color::with_alpha(255, 255, 255, 180),
);
```

三步：描述帧缓冲 → 交出切片 → 画。`Renderer` 借用你的缓冲区，绘制就地发生，不做任何分配。

库不分配帧缓冲，但**示例程序需要 `std` 才能把结果写成 BMP**——`vec![]` 只是示例的便利写法。裸机或内核里换成静态数组或从固定地址切出的切片即可。

### 安装

```toml
[dependencies]
rasteron = { path = "../rasteron" }   # 或 git = "<仓库地址>"
```

项目尚未发布到 crates.io，仓库也未配置远端；`git` 方式需替换为实际地址。

## 适合谁

- **写内核 GUI / bootloader 的**：能在 `no_std` 下用，没有依赖包袱，不要求 FPU。
- **自建图形栈的**：需要精确控制帧缓冲布局，且不想被某个框架的抽象绑住。
- **做底层绘图的 Rust 学习者**：全部实现可读，398 个测试护栏，几何与内存契约都写进了文档。

## 不适合谁

- **要 3D、抗锯齿、变换矩阵**——这是纯 2D 光栅化器，不含投影、深度、光照。
- **纹理密集的场景**——纹理采样只有 nearest / bilinear，无 mipmap，缩小会走样。
- **要 Unicode 文本排版**——内置字体只覆盖 ASCII，且无换行、无抗锯齿缩放。
- **图元数量极多且需抗锯齿**——固定功能图元是硬边整数光栅化，没有 AA。

## 特性

- **图元**：像素、线段、矩形、三角形、圆、圆弧、扇形，描边与填充成对；每种都有整数精确语义。
- **像素格式**：`Rgb24` / `Bgr24` / `Xrgb8888` / `Argb8888` / `Rgba8888` / `Bgra8888` / `Rgb565` / `Bgr565`。
- **混合与拷贝**：Alpha 混合、十六进制颜色解析、`blit` 原始拷贝、`blit_mask` 掩码拷贝。
- **定点角度**：`Angle` 用整圈 `2^32` 圈值 + 波表实现三角函数，加减自动回绕，**不碰浮点**。
- **双缓冲与子视口**：前后台缓冲区都由你提供；子视口让每个 UI 面板拥有独立坐标系，无需全局 scissor。
- **可选可编程管线**：顶点着色器 + varying 重心插值 + 片元着色器 + viewport/scissor + 索引绘制 + 2D 纹理采样。
- **可选 SIMD**：AVX-512 / AVX2 / SSE2 / NEON / RISC-V Vector 上的内存操作优化，未对齐自动回退标量。
- **可选内置字体**：8×16 点阵 ASCII，`draw_char` 直接用。

## Feature 标志

| Feature | 默认 | 作用 |
|---------|------|------|
| `programmable` | ✅ | 可编程 2D 管线。**唯一引入 `f32` 的标志**，无 FPU 目标请关闭。 |
| `simd` | ✅ | SIMD 内存操作优化。不影响正确性，只影响速度。 |
| `font_bitmap` | ✅ | 内置 8×16 点阵字体（ASCII）。 |
| `font_truetype` | ❌ | 预留，当前无实现。 |

全部组合都不引入运行时依赖。纯整数渲染：

```bash
cargo build --no-default-features
```

## 几条必须知道的约定

坐标系 **Y 轴向下**，`(0, 0)` 在左上角；角度 `0°` 向右、`90°` 向下。

除此之外，三条硬契约：

| 约定 | 违反后果 |
|------|---------|
| `pitch ≥ width × bytes_per_pixel` | `FramebufferInfo::new` panic |
| 传入切片 `≥ pitch × height` 字节 | `Renderer::new` panic |
| 图元超出可见区 | **不算错误**，静默裁剪 |

缓冲区大小用 `info.buffer_size()` 取准确值（它会计入行填充）。

## 示例

四个示例都会在仓库根目录生成 BMP（`*.bmp` 已被 `.gitignore` 排除）：

```bash
cargo run --example gallery       # 全部图元的人工比对基准图
cargo run --example basic         # 双缓冲场景：渐变天空、房子、blit、掩码烟雾
cargo run --example subviewport   # 三个面板，各自坐标系，越界内容不泄漏
cargo run --example programmable  # 可编程管线画点阵
```

`gallery` 是改动图元后的回归基准——每格有编号，出错时能一眼定位到具体图元。


`subviewport` 演示的核心思路：**裁剪不是渲染器上的状态，而是「用缓冲区的一个切片构造一个受限 `Renderer`」**。面板外泄漏像素数会打印在 stdout（正常为 0）。

## 进一步阅读

- **`docs/design.md`** — 架构决策、内存契约、错误分层、feature 门控规则等实现细节
- **源码文档** — 每个公开 API 都有带可运行示例的 `///` 文档，`cargo doc --open` 即可
- **`examples/`** — 四个可运行的完整程序

## 开发

```bash
cargo test                                          # 398 项：lib 335 + 集成 22 + doctest 41
cargo test --lib --no-default-features              # 198 项，验证 feature 门控与无浮点契约
cargo check --all-features
cargo bench                                         # 16 组 criterion 基准，耗时较长
```

`--no-default-features` 是验证「关闭 `programmable` 后不碰浮点」的唯一门径，任何改动后都应跑一遍。

MSRV 为 Rust 1.85+（下限由 `edition = "2024"` 决定）。

## 许可证

**尚未声明许可证**，仓库也未配置远端。发布或作为依赖引入前，需先补 `LICENSE` 文件与 `Cargo.toml` 的 `license` 字段。
