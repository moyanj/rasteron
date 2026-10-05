# rasteron 项目知识库

**生成时间:** 2026-10-05
**提交:** 7009f2a (`docs: 完善注释`)
**分支:** main（无远端，共 39 次提交）

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
| `Renderer<'a>` | struct | `src/core.rs:153` | 唯一入口类型；`fmt_*` 字段缓存格式属性 |
| `Pixel` | struct | `src/pixel.rs:5` | 二维坐标，`pixel!` 宏构造 |
| `FramebufferInfo` | struct | `src/framebuffer.rs:14` | `width`/`height`/`pitch`/`format` |
| `Color` | struct | `src/color.rs:16` | RGBA8；`color!` 宏构造 |
| `PixelFormat` | enum | `src/color.rs:271` | 8bpp × 6 种 + 565 打包 × 2 种 |
| `mod swar` | mod | `src/color.rs:682` | `pub(crate)`；4bpp Alpha 混合 SIMD 内核 |
| `Angle` | struct | `src/angle.rs:94` | 圈值（整圈 `2^32`），加减自动回绕 |
| `Interpolate` | trait | `src/programmable/mod.rs:34` | varying 重心插值 |
| `Pipeline` | struct | `src/programmable/mod.rs:416` | `VS` + `FS` + `PipelineState` |
| `mod simd` | mod | `src/simd.rs` | AVX-512/AVX2/SSE2/NEON/RVV memcpy |

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
  `fmt_bytes_swapped`，绘制路径禁止重新 `match` `PixelFormat`。
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
  `extern crate alloc;` 与 `use alloc::string::String;`（见 `src/error.rs:157`）。

## 禁止模式

- ❌ 引入 `[dependencies]` 中除 `criterion` 外的运行时依赖（破坏 `no_std` /
  零依赖定位；确需引入须先确认）
- ❌ 用 `as` 做 `usize → i64` 转换。必须用饱和转换，否则
  `usize::MAX as i64 == -1`，端点会被折到画布另一侧（`src/core.rs:978` 有完整说明）
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

# 全部 feature 组合都必须能编译（已验证 6 种全 PASS）
cargo check --no-default-features
cargo check --no-default-features --features simd
cargo check --no-default-features --features programmable
cargo check --no-default-features --features programmable,simd
cargo check --no-default-features --features alloc
cargo check --all-features

# 测试
cargo test --lib          # 124 passed
cargo test --doc          # 26 passed, 1 ignored

# 基准
cargo bench               # 约 16 组，warm_up 4s / measure 10s，耗时较长

# 示例（均输出 BMP 到仓库根目录）
cargo run --example basic
cargo run --example gallery
cargo run --example programmable
cargo run --example subviewport
```

## 注意事项

- **`cargo test` 与 `cargo build --examples` 当前失败**（HEAD 未修复）。
  `examples/gallery/main.rs:220` 在 `unsafe {}` 块内调用 `draw_line_unchecked`，
  但该方法在 `src/core.rs:977` 是**私有**的（同类中的
  `set_pixel_unchecked` 才是 `pub`）。这是唯一错误，其余目标均可编译。
  修复前不要把 `cargo test` 的失败当作新引入的回归。两种修法见
  `examples/AGENTS.md` 的「已知破损」。
- **MSRV 文档与配置矛盾**：`src/lib.rs:27` 与 `src/error.rs:20` 声称 MSRV 1.79
  （理由为 `core::error::Error` 需 1.81），但 `Cargo.toml` 用
  `edition = "2024"`，实际要求 Rust 1.85+。真实的下限由 edition 决定。
- **`lib.rs:59` 的 API 表格宣传了 `draw_line_unchecked`**，但它并未公开——
  文档与代码不一致，以代码为准。
- **`alloc` feature 是死的**。`Cargo.toml` 声明它、`lib.rs:38` 的表格宣传它
  「解锁自持有存储的便捷容器」，但 `rg 'feature = "alloc"'` 在
  `src/`、`examples/`、`benches/` 里零命中。同表称「`programmable` 是唯一
  需要 `alloc` 的特性」也不成立——`programmable` 的顶点缓存借用调用方切片。
  要么实现要么从 feature 表删除。
- **`VecVertexCache` 尚不存在**，但 `src/programmable/cache.rs:20` 的模块文档
  与 `VertexCache` 的结构体文档都引用了它，并写「两者都遵循同一份契约」，
  而对照表只列了一行。
- `release` profile 开了 `panic = "abort"`；`bench` profile 未开（基准需要
  unwind 以捕获 panic）。
- `no_std` 但库内不含 `unsafe` 的 `extern "C"` ABI，全部通过安全 API 暴露。