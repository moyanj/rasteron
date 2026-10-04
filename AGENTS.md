# rasteron 项目知识库

**生成时间:** 2026-10-04
**提交:** `1182972` + 未提交工作区（圆半径钳制修复、`draw_line_unchecked` 注释、文档同步）
**分支:** `main`

## 概述

`no_std` 软件光栅化库。在原始帧缓冲字节切片上直接绘制像素、线段、三角形、矩形、圆形、
圆弧与扇形，可选双缓冲与 Alpha 混合。开启 `programmable` 后额外提供一条显式参数式的 2D
可编程管线（顶点/片元着色器、varying 插值、viewport/scissor、纹理采样）。

Rust edition 2024，MSRV 1.79。**零运行时依赖**，唯一 dev-dependency 是 criterion。

## 目录结构

```
rasteron/
├── src/                 # 库本体，唯一实现目录（详见 src/AGENTS.md）
│   ├── lib.rs           # #![no_std] 门面 + feature 声明 + 顶部文档
│   ├── core.rs          # Renderer，固定功能管线与光栅化热路径
│   ├── angle.rs         # 定点角度 Angle / 楔形 Wedge，圆弧扇形的几何内核
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
| 改圆的迭代量上界 | `src/core.rs` `walk_circle_points` | 半径已钳到 `width + height`，见反模式第 6 条 |

## CODE MAP

| 符号 | 类型 | 位置 | 角色 |
|------|------|------|------|
| `Renderer` | struct | `core.rs:125` | 唯一门面。持有 `&mut [u8]` 帧缓冲 + 缓存的格式属性 |
| `Renderer::new` | fn | `core.rs:164` | 唯一 panic 点：切片小于 `pitch × height` |
| `Renderer::draw_line` | fn | `core.rs:804` | `clip_line` 快速拒绝 → 迭代区间收窄的 Bresenham |
| `Renderer::draw_line_unchecked` | fn | `core.rs:948` | 与 `draw_line` 同一内核，仅省掉快速拒绝与前置守卫 |
| `Renderer::fill_rect` | fn | `core.rs:1049` | span 填充主入口，不透明 / 半透明双分支 |
| `Renderer::blit` | fn | `core.rs:287` | 区域拷贝，格式不一致时静默返回 |
| `Renderer::blit_mask` | fn | `core.rs:342` | 8bpp 掩码绘制，抗锯齿路径 |
| `Renderer::draw_circle` / `fill_circle` | fn | `core.rs:1126,1154` | 半径在 `walk_circle_points`（`core.rs:1083`）内钳制 |
| `Renderer::draw_arc` / `fill_arc` | fn | `core.rs:1173,1237` | 圆弧 / 扇形。复用 `walk_circle_points`，回调内用 `Wedge` 筛角度 |
| `Renderer::swap_buffers` | fn | `core.rs:1175` | 仅 `double_buffer`。实为 back→front 单向拷贝，非指针交换 |
| `Angle` | struct | `angle.rs` | 定点角度，整圈 `2^32` 圈值，加减自动环绕 |
| `Wedge` | struct | `angle.rs` | 起止角区间，构造时预算单位向量，判定只做叉积 |
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
- **`unsafe fn` 必须写 `# Safety` 段**。`lib.rs:82` 已开 `unsafe_op_in_unsafe_fn`，块内仍需显式 `unsafe {}`。
- **`# Safety` 段不能与正文自相矛盾**。若实现已自行收窄与校验、实际无附加前置条件，就只声明与安全版相同的前提，不要一边写「不需要额外保证」一边罗列坐标要求（`draw_line_unchecked` 是范例）。
- **边界裁剪只做一次**：在上层几何函数裁好，热路径直接裸指针写。`debug_assert!` 兜底，不做运行时检查。
- **格式属性在 `new()` 缓存**：`fmt_bpp` / `fmt_rgb_order` / `fmt_opaque_alpha` / `fmt_stores_color_alpha`。热路径禁止 `match info.format`。
- **字节序分支用宏消除**：`write_rgb!` / `write_rgb_blend!` / `write_rgb_blend_ptr!` / `write_rgb_ptr!`（`core.rs:15-88`）。
- **优化路径必须与标量参考等价**，并写穷举校验单测。命名约定：`blend_word_matches_blend_channel_over_all_byte_pairs`、`blend_span_matches_write_color_reference`、`alpha_fill_matches_per_channel_reference`。
- **`usize → i64` 一律走 `sat_i64`**（`utils.rs:117`）。乘积中间值走 `i128`。
- **迭代量必须与调用方输入解耦**：任何 `for` / `while` 的次数只能取决于帧缓冲尺寸，不得取决于端点坐标或半径。本库曾是 DoS 温床——`draw_line_unchecked` 按线段全长步进（端点取 `usize::MAX` 时越界写，实测 SIGSEGV）、`walk_circle_points` 按半径迭代（半径取 `1_000_000_000` 时约 7 亿次迭代把线程钉死），二者现均改为按帧缓冲尺寸收窄（见反模式）。
- **panic 只用于构造期不变量**：`FramebufferInfo::new`、`Renderer::new`、`Texture2D::new`。运行期可恢复错误走 `Option`/`Result`（如 `Texture2D::try_new`、`blit_mask` 静默返回）。
- **文档与注释全中文**，公开项必须有 rustdoc 示例。

## 反模式

- 不要在热路径加日志、`println!`、计时或分配。
- 不要新增 `PixelFormat` 变体却不同步 `core.rs:177-184` 的 match 与四个 `fmt_*` 字段——会静默写错字节序。
- 不要给 3bpp 强加 SWAR：跨字节不对齐，`pattern_fill` 必须退回倍增复制（`utils.rs:163`）。
- 不要在 4bpp 行首未 4 字节对齐时走 SWAR。`pitch` 允许非 4 倍数，第 2 行起会错位（`core.rs:522`）。
- **不要把 `clip_line` 的返回值当新端点用**。`intersect_axis` 用整数除法截断求交点，交点不在原直线上，斜率一改像素覆盖就变。`clip_line` 只能作「整段不可见」的快速拒绝，可见区间的裁剪由 `draw_line_internal` 的区间收窄完成。
- **不要写 `for x in a..=b` 这类由端点决定的循环**。先收窄到可见范围再迭代；`draw_line_internal` 的 `x_hi = x2.min(x_hi_bound)` 就是范本。
- **不要把 `radius` 直接喂给迭代次数**。`walk_circle_points`（`core.rs:1083`）已把半径钳到 `width + height`——任意可见像素到圆心的距离都小于 `sqrt(w² + h²) < w + h`，故该钳制不改变任何可见像素的着色结果，屏幕外的点由 `visit` 的边界检查丢弃。加法须用 `saturating_add`（`FramebufferInfo::new` 不限制 `width`/`height`），转换须用 `sat_i64`（`as i64` 会把 `usize::MAX` 回绕成负半径而一个点都不画）。
  改动此处必须让两条测试同时通过，它们各守一个方向：`circle_clamps_huge_radius` 抓**钳制缺失**（半径表把 `1_000_000_000` 排在最前，无钳制时测试直接挂死），`circle_clamp_preserves_radii_within_limit` 抓**钳制过度**（锚定半径 1/2/3 的实测填充像素数 5/15/16）。
- 不要在 `draw_line` 的迭代收窄中改动 Bresenham 相位或丢弃原始端点，否则像素覆盖会变（`draw_line_iteration_narrowing_preserves_coverage` 守护）。
- **不要把大跨度楔形写成「补集」**。`span > 180°` 时直接取小楔形的补，会把恰好落在起点射线上的像素误排除（该点位于缺口楔形的边界）。正确写法是从中点拆成两个 `< 180°` 的小楔形再取**并集**——`Wedge::contains` 就是这么做的，`angle.rs::wedge_major_arc_includes_start_ray` 守护。
- **不要在 `fill_arc` 里照抄 `fill_circle` 的四条对称跨度**。中点圆的八对称在 `x == y` 的对角位置会让多条跨度覆盖同一像素；对不透明色重复写无害，**半透明色重复混合会逐次加深**（实测 alpha=128 叠两次得到 191 而非 128）。`fill_arc` 因此先把 `(行, 半宽)` 归并去重再写，`fill_arc_alpha_blends_into_background` 守护。
- 不要去掉 `Wedge` 叉积判定里的容差项。定点三角在"像素恰好落在边界射线上"（如轴向像素 `(1,0)`）会有舍入抖动，无容差会把边界像素随机判到界外；容差按操作数同阶缩放，`wedge_tolerance_does_not_widen_arc` 守护它没有取得过宽。
- 不要引入 OpenGL 风格 clip space `[-1,1]`。`programmable` 全程用 normalized `[0,1]`。
- 不要把 SWAR 内核从 `color::swar` 挪走——它与 `PixelFormat` 的字节序/alpha 语义强耦合。
- 不要用 `alloc::vec![None; n]` 之外的可变状态缓存顶点着色结果，`draw_indexed` 依赖它免去重复着色。

## 命令

```bash
cargo build                          # 默认三特性全开
cargo test --lib                     # 106 个单测（angle 17 / core 55 / color 16 / programmable 16 / framebuffer 2），1 个 ignored
cargo test                           # lib 106 + doctest 15（1 个 ignored）
cargo bench                          # criterion，harness = false
cargo run --example basic            # → basic-example.bmp
cargo run --example programmable --features programmable   # → programmable-example.bmp
cargo run                            # src/main.rs → rasteron-test.bmp

# 差分探针（人工比对两次实现的像素覆盖，不作回归断言）
PROBE_OUT=/tmp/a cargo test --lib diff_probe -- --ignored

# no_std 与分配器验证
cargo build --no-default-features
cargo build --no-default-features --features programmable
```

## 已知缺口

`cargo test` 全绿（lib 106 + doctest 15）。2 个 `ignored`：一在 `lib.rs` 的 ` ```ignore ` 示例，一在 `core.rs` 的 `diff_probe` 差分探针。
以下为已知但未修的项：

- `simd::memcpy`（`simd.rs:145`）是全库唯一缺 `/// # Safety` 段的 `pub unsafe fn`（`cargo clippy` 会报 `missing_safety_doc`）；`utils.rs:168` 与 `simd.rs` 内的裸指针块也没有 `// SAFETY:` 注释。
- `cargo clippy` 另有 5 条既存警告：`core.rs:220` 的 `as usize` 冗余转换、`programmable.rs:1111` 循环变量只用于索引、`examples/shared/bmp.rs:113` 无效位运算与 `:157` 手写 `is_multiple_of`。

## 其他易踩点

- `src/main.rs:11` 用 `#[path = "../examples/shared/bmp.rs"]` 跨目录复用 example 的代码。改 `examples/shared/bmp.rs` 会连带影响演示二进制。
- `Renderer` 字段除 `info` 外全部私有；`fmt_*` 为 `pub(crate)`。新增公开 API 前先确认是否真需要暴露。
- `draw_line_unchecked` 保留 `unsafe` 仅为不破坏 API，实现已委托给受限内核 `draw_line_internal`，实际不再需要调用方提供任何坐标保证。
- `pattern_fill` 对 16 字节模式走直接块写而非倍增，原因见 `utils.rs:131-138` 注释（倍增链无法向量化）。
- `floor_to_usize`（`utils.rs:127`）只在 `programmable` 下编译，缺失该 `cfg` 会让 `--no-default-features` 构建报 `dead_code`。