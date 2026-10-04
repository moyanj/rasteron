# rasteron 项目知识库

**生成时间:** 2026-10-04
**提交:** `faf837a` perf(core): SWAR 掩码混合与对齐回退,补齐文档与基准
**分支:** `main`

## 概述

`no_std` 软件光栅化库。在原始帧缓冲字节切片上直接绘制像素、线段、三角形、矩形、圆形，
可选双缓冲与 Alpha 混合。开启 `programmable` 后额外提供一条显式参数式的 2D 可编程管线
（顶点/片元着色器、varying 插值、viewport/scissor、纹理采样）。

Rust edition 2024，MSRV 1.79。**零运行时依赖**，唯一 dev-dependency 是 criterion。

## 目录结构

```
rasteron/
├── src/                 # 库本体，唯一实现目录（详见 src/AGENTS.md）
│   ├── lib.rs           # #![no_std] 门面 + feature 声明 + 顶部文档
│   ├── core.rs          # Renderer，固定功能管线与光栅化热路径
│   ├── programmable.rs  # 可编程管线（着色器/插值/裁剪/纹理）
│   ├── color.rs         # Color / PixelFormat / SWAR 混合内核
│   ├── framebuffer.rs   # FramebufferInfo 布局描述
│   ├── pixel.rs         # Pixel 坐标 + pixel! 宏
│   ├── simd.rs          # 编译期 `#[cfg(target_feature)]` 分派的 memcpy
│   ├── utils.rs         # 无分配几何/内存工具
│   └── main.rs          # 演示二进制，产出 rasteron-test.bmp
├── benches/renderer.rs  # criterion 基准，17 个 bench fn
├── examples/
│   ├── basic/           # 全部固定功能图元
│   ├── programmable/    # 着色器/插值/纹理/裁剪
│   └── shared/bmp.rs    # BMP 写出，被 examples 与 src/main.rs 复用
└── *.bmp                # 运行产物，已 gitignore
```

## 去哪里改

| 任务 | 位置 | 要点 |
|------|------|------|
| 加一个固定功能图元 | `src/core.rs` | 参照 `fill_rect`/`fill_circle` 的裁剪 + 跨度填充两段式 |
| 加一个像素格式 | `src/color.rs` + `src/core.rs` | **两处都要改**，见下方 ANTI-PATTERNS |
| 优化写像素热路径 | `src/core.rs` 跨度函数 + `src/color.rs::swar` | 必须同步补等价性单测 |
| 改可编程管线语义 | `src/programmable.rs` | 坐标是 normalized `[0,1]`，不是 clip space |
| 换 SIMD 阶梯 | `src/simd.rs` | 阶梯顺序 AVX512 → AVX → SSE2 → NEON → RVV |
| 加基准 | `benches/renderer.rs` | 追加 bench fn 并注册进 `criterion_group!` |
| 改帧缓冲布局约束 | `src/framebuffer.rs` | `pitch` 允许不是 4 的倍数，会击穿 SWAR 路径 |

## CODE MAP

| 符号 | 类型 | 位置 | 角色 |
|------|------|------|------|
| `Renderer` | struct | `core.rs:113` | 唯一门面。持有 `&mut [u8]` 帧缓冲 + 缓存的格式属性 |
| `Renderer::new` | fn | `core.rs:152` | 唯一 panic 点：切片小于 `pitch × height` |
| `Renderer::draw_line` | fn | `core.rs:792` | Cohen–Sutherland 拒绝 → 迭代区间收窄的 Bresenham |
| `Renderer::fill_rect` | fn | `core.rs:1047` | span 填充主入口，SWAR / 逐像素双分支 |
| `Renderer::blit` | fn | `core.rs:275` | 区域拷贝，格式不一致时静默返回 |
| `Renderer::blit_mask` | fn | `core.rs:330` | 8bpp 掩码绘制，抗锯齿路径 |
| `Renderer::swap_buffers` | fn | `core.rs:1145` | 仅 `double_buffer`。实为 back→front 单向拷贝，非指针交换 |
| `PixelFormat` | enum | `color.rs:186` | 6 种格式，shift/bpp 均 `const fn` |
| `PixelFormat::write_color` | fn | `color.rs:288` | **标量参考实现**，所有优化路径的等价性基准 |
| `color::swar` | mod | `color.rs:432` | SWAR 批量混合内核（`pub(crate)`） |
| `FramebufferInfo` | struct | `framebuffer.rs:14` | `width/height/pitch/format`，`pitch` 可含填充 |
| `Pixel` + `pixel!` | struct/macro | `pixel.rs:5,45` | `(0,0)` 在左上角 |
| `FragmentShader` | trait | `programmable.rs:248` | 唯一着色器扩展点 |
| `Interpolate` | trait | `programmable.rs:129` | varying 插值契约 |
| `Pipeline` | struct | `programmable.rs:620` | VS + FS + `PipelineState` 三元组 |
| `Texture2D` | struct | `programmable.rs:389` | nearest / bilinear 采样 + wrap 模式 |
| `clip_line` | fn | `utils.rs:14` | Cohen–Sutherland，i128 中间运算防溢出 |
| `simd::memcpy` | fn | `simd.rs:145` | `len <= 128` 直接走 `copy_nonoverlapping` |

## FEATURES

| 标志 | 默认 | 影响 |
|------|------|------|
| `double_buffer` | 是 | 链接 `alloc`，分配后台缓冲；`programmable` 关闭时唯一需要分配器的特性 |
| `programmable` | 是 | 引入 `f32`；关闭后几何计算全整数，无浮点依赖 |
| `simd` | 是 | 经 `blit`/`clear`/填充路径使用向量寄存器 |

组合约束（改动时勿破坏）：

- 关掉 `programmable` + `double_buffer` ⇒ 不链接全局分配器。
- 内核态使用 `simd` 需自行配对 `kernel_fpu_begin/end`（或 NEON 版本），因为它会动 XMM/YMM/ZMM、V0-V31。
- `simd` 在无任何向量扩展的平台上仍有 `#[cfg]` 回退分支，不要删。

## 约定

偏离 Rust 社区惯例之处，改码前必读：

- **Y 轴向下**，`(0,0)` 在帧缓冲左上角（`pixel.rs:3`、`utils.rs:12`）。全库统一，无例外。
- **双 API 模式**：每个 `xxx` 都有配对的 `xxx_unchecked`。安全版做边界检查并**静默忽略**越界，unchecked 版把责任交给调用方。二者必须行为一致，且有 `*_unchecked_matches_safe_path` 一类单测守护。
- **`unsafe fn` 必须写 `# Safety` 段**，格式为「调用方必须保证 ……」。`lib.rs:82` 已开 `unsafe_op_in_unsafe_fn`，块内仍需显式 `unsafe {}`。
- **边界裁剪只做一次**：在上层几何函数裁好，热路径直接裸指针写。`debug_assert!` 兜底，不做运行时检查。
- **格式属性在 `new()` 缓存**：`fmt_bpp` / `fmt_rgb_order` / `fmt_opaque_alpha` / `fmt_stores_color_alpha`。热路径禁止 `match info.format`。
- **字节序分支用宏消除**：`write_rgb!` / `write_rgb_blend!` / `write_rgb_blend_ptr!` / `write_rgb_ptr!`（`core.rs:12-88`）。
- **优化路径必须与标量参考等价**，并写穷举校验单测。命名约定：`blend_word_matches_blend_channel_over_all_byte_pairs`、`blend_span_matches_write_color_reference`、`alpha_fill_matches_per_channel_reference`。
- **`usize → i64` 一律走 `sat_i64`**（`utils.rs:117`）。乘积中间值走 `i128`。
- **panic 只用于构造期不变量**：`FramebufferInfo::new`、`Renderer::new`、`Texture2D::new`。运行期可恢复错误走 `Option`/`Result`（如 `Texture2D::try_new`、`blit_mask` 静默返回）。
- **文档与注释全中文**，公开项必须有 rustdoc 示例。

## 反模式

- 不要在热路径加日志、`println!`、计时或分配。
- 不要新增 `PixelFormat` 变体却不同步 `core.rs:165-172` 的 match 与四个 `fmt_*` 字段——会静默写错字节序。
- 不要给 3bpp 强加 SWAR：跨字节不对齐，`pattern_fill` 必须退回倍增复制（`utils.rs:162`）。
- 不要在 4bpp 行首未 4 字节对齐时走 SWAR。`pitch` 允许非 4 倍数，第 2 行起会错位（`core.rs:511`）。
- 不要在 `draw_line` 的迭代收窄中改动 Bresenham 相位或丢弃原始端点，否则像素覆盖会变（已有两个测试因此失败，见 NOTES）。
- 不要引入 OpenGL 风格 clip space `[-1,1]`。`programmable` 全程用 normalized `[0,1]`。
- 不要把 SWAR 内核从 `color::swar` 挪走——它与 `PixelFormat` 的字节序/alpha 语义强耦合。
- 不要用 `alloc::vec![None; n]` 之外的可变状态缓存顶点着色结果，`draw_indexed` 依赖它免去重复着色。

## 命令

```bash
cargo build                          # 默认三特性全开
cargo test --lib                     # 75 个单测（core 41 / color 16 / programmable 16 / framebuffer 2）
cargo test                           # 含 examples 编译，当前失败，见 NOTES
cargo bench                          # criterion，harness = false
cargo run --example basic            # → basic-example.bmp
cargo run --example programmable --features programmable   # → programmable-example.bmp
cargo run                            # src/main.rs → rasteron-test.bmp

# no_std 与分配器验证
cargo build --no-default-features
cargo build --no-default-features --features programmable
```

## 已知破损状态

修改前先跑一遍基线，以下为既存问题，非新引入：

1. **`cargo test` 编译失败**：`examples/basic/main.rs:101` 调用 `Renderer::fill_rounded_rect`，该方法在 `src/` 中不存在（`git log -S` 显示它在 `8a86c0f` 引入过，后续提交中丢失）。影响面：`cargo test`、`cargo build --examples`。`cargo test --lib` 不受影响。
2. **2 个单测失败**（`cargo test --lib` → 73 passed / 2 failed）：
   - `core::tests::draw_line_iteration_narrowing_preserves_coverage`（`core.rs:2070`）
   - `core::tests::draw_line_preserves_bresenham_coverage_when_end_is_below_screen`（`core.rs:1335`）

   两者都是线段覆盖性与"迭代区间收窄"优化的等价性断言，症状为收窄后覆盖的像素集合右移一位。

## 其他易踩点

- `src/main.rs:11` 用 `#[path = "../examples/shared/bmp.rs"]` 跨目录复用 example 的代码。改 `examples/shared/bmp.rs` 会连带影响演示二进制。
- `Renderer` 字段除 `info` 外全部私有；`fmt_*` 为 `pub(crate)`。新增公开 API 前先确认是否真需要暴露。
- `pattern_fill` 对 16 字节模式走直接块写而非倍增，原因见 `utils.rs:130-137` 注释（倍增链无法向量化）。该段注释里 `stready-state` 是 `steady-state` 的笔误，别照抄。