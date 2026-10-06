# rasteron 项目知识库

**生成时间:** 2026-10-06
**分支:** main（无远端）

## 概述

`rasteron` 是 `#![no_std]` 的 2D 软件光栅化库，面向嵌入式与内核态帧缓冲渲染。
零 `[dependencies]`（`criterion` 仅 dev），核心栈只有整数运算、裸指针与内建
SIMD intrinsics。

同时产出两种目标：

- **lib**（`src/lib.rs`）— 库本体，`no_std`
- **bin**（`src/main.rs`）— 演示程序，画一条线后落盘 `rasteron-test.bmp`

## 目录结构

```
rasteron/
├── src/                    # 全部实现，见 src/AGENTS.md
│   └── programmable/       # feature 门控的 f32 可编程管线
├── examples/               # 4 个示例 + 跨目录共享的 BMP 落盘工具
├── benches/renderer.rs     # criterion 基准（单文件，16 个 bench 组）
├── Cargo.toml              # 无 [dependencies]；feature 与 profile 全在此
└── *.bmp                   # 示例产物，已被 .gitignore 排除
```

**缺失的基础设施**（不是遗漏，是尚未建立）：无 CI、无 `rustfmt.toml`、
无 `clippy.toml`、无 README、无 `.github/`。

## 去哪找

| 任务 | 位置 | 说明 |
|------|------|------|
| 加/改固定功能图元 | `src/core.rs` | `Renderer` 全部绘制方法集中于此 |
| 加/改像素格式 | `src/color.rs` | `PixelFormat` 变体 + SWAR 内核同文件 |
| 改角度/三角函数 | `src/angle.rs` | 圈值 `Angle` + 257 项四分之一波表 |
| 改可编程管线 | `src/programmable/mod.rs` | `draw_arrays` / `draw_indexed` 在此 |
| 改子视口裁剪 | `src/utils.rs` | `sub_view` 系列 |
| 改 SIMD memcpy | `src/simd.rs` | 仅 `memcpy`，feature = `simd` |
| 加错误类型 | `src/error.rs` | 全库错误唯一定义处 |
| 加基准 | `benches/renderer.rs` | 已有 `criterion_config()` 统一配置 |
| 加示例 | `examples/<name>/main.rs` | 复用 `examples/shared/bmp.rs` |

## 代码地图

| 符号 | 类型 | 位置 | 角色 |
|------|------|------|------|
| `Renderer<'a>` | struct | `src/core.rs:141` | 唯一入口类型；`fmt_*` 字段缓存格式属性 |
| `Pixel` | struct | `src/pixel.rs:5` | 二维坐标，`pixel!` 宏构造 |
| `FramebufferInfo` | struct | `src/framebuffer.rs:14` | `width`/`height`/`pitch`/`format` |
| `Color` | struct | `src/color.rs:16` | RGBA8；`color!` 宏构造 |
| `PixelFormat` | enum | `src/color.rs:270` | 8bpp × 6 种 + 565 打包 × 2 种 |
| `mod swar` | mod | `src/color.rs:694` | `pub(crate)`；4bpp Alpha 混合 SIMD 内核 |
| `Angle` | struct | `src/angle.rs:94` | 圈值（整圈 `2^32`），加减自动回绕 |
| `Interpolate` | trait | `src/programmable/mod.rs:34` | varying 重心插值 |
| `Pipeline` | struct | `src/programmable/mod.rs:432` | `VS` + `FS` + `PipelineState` |
| `mod simd` | mod | `src/simd.rs` | AVX-512/AVX2/SSE2/NEON/RVV memcpy（7 个测试） |

## 约定

- **坐标系**：Y 轴向下，`(0,0)` 在左上角；角度沿屏幕坐标增大（`0°`→右、
  `90°`→下）。全库统一，改动任何图元都不要引入别的约定。
- **无浮点契约**：关闭 `programmable` feature 时几何计算**全整数**
  （`Angle` 圈值 + 波表，无 `f32`）。`programmable` 是唯一引入 `f32` 的 feature，
  内核态目标须据此评估 FPU 状态保存。
- **错误分层**（`src/error.rs` 模块文档有完整表格）：
  - 构造期不变量被破坏 → `panic`（如 `FramebufferInfo::new` 的 pitch 断言）
  - 运行期可恢复失败 → `Result` / `Option`
  - **像素级越界不算错误**：几何超出屏幕由绘制 API 静默裁剪
- **热路径零格式匹配**：`Renderer` 在构造时把格式属性缓存进 `fmt_bpp` /
  `fmt_rgb_order` / `fmt_opaque_alpha` / `fmt_stores_color_alpha` /
  `fmt_layout_565`，绘制路径禁止重新 `match` `PixelFormat`。565 的字节序是
  小端常量（`FMT_565_LITTLE_ENDIAN`），无需缓存——红蓝差异由 `fmt_layout_565` 表达。
- **安全边界分层**：安全 API 用 `width`/`height` 做边界判断，底层写入用
  `get_unchecked_mut`。`Renderer::new` 断言切片 ≥ `pitch × height` 是这套
  方案不越界的前提，不可移除。
- **不做分配**：所有缓冲区由调用方提供；双缓冲的后台缓冲区同样由调用方传入
  （`Renderer::with_back_buffer`）。
- **文档语言**：全中文。每个公开项带 `///` 文档，模块顶部 `//!` 用
  `# 概述` / `# 示例` / `# 恐慌` / `# Safety` 分节。新增公开 API 必须带
  可运行 doctest。
- **提交信息**：Conventional Commits + 中文描述，正文说明**为什么**。
  参考 `fix(core): 钳制圆半径与线段迭代量,消除 DoS 与越界写`。
- **`no_std` 下的测试**：`#[cfg(test)] mod tests` 内需显式
  `extern crate alloc;` 与 `use alloc::string::String;`（见 `src/error.rs:160`）。

## 禁止模式

- ❌ 引入 `[dependencies]` 中除 `criterion` 外的运行时依赖（破坏 `no_std` /
  零依赖定位；确需引入须先确认）
- ❌ 用 `as` 做 `usize → i64` 转换。必须用饱和转换，否则
  `usize::MAX as i64 == -1`，端点会被折到画布另一侧（`src/core.rs` 的 `draw_line_unchecked` 有完整说明）
- ❌ 给错误类型实现 `std::error::Error`。MSRV 早于其稳定，仅实现
  `Debug` + `Display`；`Display` 文案是对外契约，有测试守护
- ❌ 在 `angle.rs` 引入浮点三角函数，或改用「度 × 系数」表示角度
- ❌ 用 `unimplemented!` / `todo!` 占位。当前代码零此类标记
- ❌ 提交 `*.bmp` 产物（`.gitignore` 已排除，但勿用 `-f` 强推）

## 项目独有风格

- **宏承担重复**：字节序写入在 `core.rs` 有 4 个成组的
  `write_rgb*` 宏；`pixel!` / `color!` 在各自模块。
- **SWAR 与回退成对**：4bpp 且行首 4 字节对齐走 `color::swar` 批量内核，
  `Bgr24` 因跨字节错位**始终**逐通道回退（基准中作为对照组）；565 无 SWAR 路径。
  新增格式时须明确它落在哪一侧。
- **`Angle` 用圈值而非毫度**：为了 `>> 30` 即得象限号、加减自动环绕。
- **示例即基准图**：`examples/gallery` 排布全部图元成带编号网格，供人工比对。
- **基准绕过借用检查**：`benches/renderer.rs:71` 用
  `from_raw_parts_mut` 造 `'static`，让 `Renderer<'static>` 逃出函数作用域。
- **越权防护视为功能**：`11c3e58` 与 `33f951b` 把半径钳制、线段迭代收窄、
  精确圆盘语义当作正确性修复而非优化，勿"简化"回去。

## 命令

```bash
# 构建与检查（默认 feature：programmable + simd）
cargo build
cargo check --release

# 全部 feature 组合都必须能编译（已验证 5 种全 PASS）
# 注：原计划的 `--features alloc` 一项已作废——alloc 是死 feature，已删除。
cargo check --no-default-features
cargo check --no-default-features --features simd
cargo check --no-default-features --features programmable
cargo check --no-default-features --features programmable,simd
cargo check --all-features

# 测试
cargo test                # 323 passed（lib 274 + 集成 22 + doctest 27）
cargo test --lib          # 274 passed
cargo test --lib --no-default-features   # 198 passed（验证 feature 门控）
cargo test --doc          # 27 passed, 0 ignored

# 基准
cargo bench               # 约 16 组，warm_up 4s / measure 10s，耗时较长

# 示例（均输出 BMP 到仓库根目录）
cargo run --example basic
cargo run --example gallery
cargo run --example programmable
cargo run --example subviewport
```

## 注意事项

- **已知缺陷：`fill_triangle` 水平底边的绕向不对称**。`y2 == y3`（水平底边）时
  三顶点按 Y 排序不唯一，`x2` 取决于顶点顺序，而 `x2` 是 `lower` 段的 `base_x`：
  正序触发 `interpolate_scan_x` 的 `delta_y == 0` 短路返回 `x2`，反序走插值，
  同一三角形两种绕向**相差 1 个像素**。已用实验确认与其它修复无关。
  修它要改几何语义（`lower` 段在 `y2 == y3` 时应取 `x3`），风险高于收益。
  现状由 `fill_triangle_horizontal_bottom_edge_is_winding_asymmetric` 记录——
  **该测试断言的是 `assert_ne!`，缺陷修好的那一刻它会主动失败并提示你改写。**
- **`sub_view` 在 `x > 0` 时恒定少一行**：这不是保守取舍，而是 `Renderer::new`
  要求切片至少 `pitch × height` 的硬约束（窗口切片取自
  `&mut fb[off..off + pitch × height]`，末行也必须凑满一整行）。按可见像素算本可
  多一行，但那样切片会超出缓冲区、构造时 panic。详见 `sub_view` 文档
  「为什么 `x > 0` 时恒定少一行」小节。
- **`clip_line` 的左/上边界分支是死代码**：`compute_code` 里的 `LEFT`/`BOTTOM`
  判断永不成立——`Pixel` 字段是 `usize`，负坐标无法表达。
- `release` profile 开了 `panic = "abort"`；`bench` profile 未开（基准需要
  unwind 以捕获 panic）。
- `no_std` 但库内不含 `unsafe` 的 `extern "C"` ABI，全部通过安全 API 暴露。
- 测试规模与分布见 `target/TEST_PLAN.md` §6.1（该文件在 `.gitignore` 内）。

## 已修复的历史缺陷（均有回归护栏）

| 缺陷 | 影响 |
|------|------|
| `color!` 宏无法匹配字符串字面量 | 文档宣传的 `color!("#RRGGBB")` 从未能编译 |
| `clear` 无条件 memcpy 16 字节模式 | `pitch > row_len && row_len < 16` 时污染行填充字节 |
| `fill_visible_horizontal_span` 越界写 | **内存破坏**（release 静默写坏内存） |
| `fill_circle`/`fill_arc` 的 `cx ± half` i64 溢出 | 大坐标 panic |
| `ceil_to_usize(f32::NAN)` 返回 1 | 三角形 bbox 凭空多扩一像素 |
