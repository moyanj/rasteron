# rasteron 项目知识库

**生成时间:** 2026-10-06
**分支:** main（无远端）

## 概述

`rasteron` 是 `#![no_std]` 的 2D 软件光栅化库，面向嵌入式与内核态帧缓冲渲染。
除 `font_truetype` 门控的 `fontdue` 外零 `[dependencies]`（`criterion` 仅 dev），
核心栈只有整数运算、裸指针与内建 SIMD intrinsics。

同时产出两种目标：

- **lib**（`src/lib.rs`）— 库本体，`no_std`
- **bin**（`src/main.rs`）— 演示程序，画一条线后落盘 `rasteron-test.bmp`

## 目录结构

```
rasteron/
├── src/                    # 全部实现，见 src/AGENTS.md
│   ├── programmable/       # feature 门控的 f32 可编程管线
│   └── image/              # 图像编解码（目前只有 BMP）
├── src/font/               # 字体 trait + 位图/TrueType 实现
├── examples/               # 4 个示例 + 跨目录共享的 BMP 落盘工具
├── tests/fixtures/         # minimal_ascii.ttf（DejaVu 子集，10768 B，7 字形）
├── benches/                # 5 个 criterion 目标 + shared/ 共享脚手架
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
| 加/改纹理与采样 | `src/programmable/texture.rs` | `Texture2D` + `sample(uv, filter, wrap)` |
| 改子视口裁剪 | `src/utils.rs` | `sub_view` 系列 |
| 改 SIMD memcpy | `src/simd.rs` | 仅 `memcpy`，feature = `simd` |
| 加/改字体接口 | `src/font/mod.rs` | `Font` trait + `draw_char` / `draw_text` |
| 改 TrueType 支持 | `src/font/ttf.rs` | `TtfFont`；度量来自 fontdue，光栅化写调用方缓冲 |
| 加错误类型 | `src/error.rs` | 全库错误唯一定义处 |
| 加/改 BMP 编解码 | `src/image/bmp.rs` | 解码到调用方缓冲 / 编码到调用方切片，零分配；调色板逻辑在 `src/image/bmp/palette.rs` |
| 加基准 | `benches/<子系统>.rs` | 按被测子系统选文件；复用 `benches/shared/mod.rs` 的 `criterion_config()` / `PixFb` |
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
| `Texture2D<'a>` | struct | `src/programmable/texture.rs:113` | 借用式 2D 纹理；半 texel 采样 |
| `TextureFs<'a>` | struct | `src/programmable/shader.rs:263` | 按 UV varying 采样的内建片元着色器 |
| `mod simd` | mod | `src/simd.rs` | AVX-512/AVX2/SSE2/NEON/RVV memcpy（7 个测试） |
| `Font` | trait | `src/font/mod.rs` | 度量 / 光栅化两阶段；返回值**不借用** `&self` |
| `GlyphLayout` | struct | `src/font/mod.rs` | 单字度量：`width`/`height`/`bearing_x`/`bearing_y`/`advance` |
| `FontMetrics` | struct | `src/font/mod.rs` | 行度量：`ascent` / `descent` / `line_gap` |
| `PenX` | struct | `src/font/mod.rs` | 26.16 定点画笔；`advance()` 精确累加，长串不漂移 |
| `Char<'a>` | struct | `src/font/mod.rs` | 已光栅化字形（尺寸 + 掩码借用），供调用方缓存 |
| `TtfFont` | struct | `src/font/ttf.rs:44` | fontdue 后端；`new` 解析失败即 panic |
| `DefaultFont` | struct | `src/font/font_8x16.rs:628` | 内置 8×16 ASCII；基线第 12 行，零分配 |

## 约定

- **坐标系**：Y 轴向下，`(0,0)` 在左上角；角度沿屏幕坐标增大（`0°`→右、
  `90°`→下）。全库统一，改动任何图元都不要引入别的约定。
- **无浮点契约**：关闭 `programmable` feature 时几何计算**全整数**
  （`Angle` 圈值 + 波表，无 `f32`）。**注意 `font_truetype` 同样引入 `f32`**
  ——fontdue 的光栅化本就以浮点累加（`scale_factor(px)`、亚像素偏移），接口
  选 `f32` 字号是顺势而为而非破坏约定。内核态目标须关闭**两个** feature 并据此
  评估 FPU 状态保存；`font_bitmap` 路径不产生任何浮点运算。
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
  （`Renderer::with_back_buffer`）。**`alloc` feature 是显式放弃这条契约的唯一
  出口**——它只新增 `draw_char_owned` / `draw_text_owned` 两个便利方法，
  其余 API 在任何 feature 组合下都不分配。`font_truetype` 隐含 `alloc`
  （fontdue 本身依赖 `alloc`）。
- **文档语言**：全中文。每个公开项带 `///` 文档，模块顶部 `//!` 用
  `# 概述` / `# 示例` / `# 恐慌` / `# Safety` 分节。新增公开 API 必须带
  可运行 doctest。
- **提交信息**：Conventional Commits + 中文描述，正文说明**为什么**。
  参考 `fix(core): 钳制圆半径与线段迭代量,消除 DoS 与越界写`。
- **`no_std` 下的测试**：`#[cfg(test)] mod tests` 内需显式
  `extern crate alloc;` 与 `use alloc::string::String;`（见 `src/error.rs:160`）。

## 禁止模式

- ❌ 引入 `[dependencies]` 中除 `criterion` / `fontdue` 外的运行时依赖
  （`fontdue` 由 `font_truetype` 门控，是唯一的例外；其余确需引入须先确认）
- ❌ 在无 `alloc` 的构建里让库内分配。`draw_char` / `draw_text` 的 scratch
  一律由调用方传入；`*_owned` 变体必须 `#[cfg(feature = "alloc")]` 门控。
  `ttf.rs` 里的 `alloc::vec::Vec` 只用于承接 fontdue 的返回值，是适配层的必然
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
- **示例即基准图**：`examples/gallery` 把全部能力排成 28 格编号网格（图元、双缓冲、
  子视口、字体排版、可编程管线、8 种像素格式），供人工比对；标题与注释文字由库
  自己的 `draw_text` + 内置 8×16 点阵字体绘制（不维护手写字库）。
- **基准的帧缓冲只在一处 `unsafe`**：`benches/shared/mod.rs` 的 `PixFb`
  用 `from_raw_parts_mut` 造 `'static`，让 `Renderer<'static>` 能跨
  `b.iter()` 复用。该处同时断言切片 ≥ `pitch × height`（`Renderer::new`
  的前置条件），写错尺寸得到的是清晰 panic 而非越界写。
- **越权防护视为功能**：`11c3e58` 与 `33f951b` 把半径钳制、线段迭代收窄、
  精确圆盘语义当作正确性修复而非优化，勿"简化"回去。

## 命令

```bash
# 构建与检查（默认 feature：programmable + simd）
cargo build
cargo check --release

# 全部 feature 组合都必须能编译（已验证 10 种全 PASS）
# 注：`alloc` 曾是死 feature 已删除，本次重构重新引入——它现在是
# `draw_char_owned` / `draw_text_owned` 两个免 scratch 方法的门控。
cargo check --no-default-features
cargo check --no-default-features --features simd
cargo check --no-default-features --features programmable
cargo check --no-default-features --features alloc
cargo check --no-default-features --features font_bitmap
cargo check --no-default-features --features font_bitmap,alloc
cargo check --no-default-features --features font_truetype
cargo check --no-default-features --features font_truetype,font_bitmap
cargo check --no-default-features --features programmable,simd
cargo check --all-features

# 测试
cargo test                # 494 passed（lib 425 + 集成 22 + doctest 47）
cargo test --lib          # 425 passed
cargo test --lib --no-default-features   # 245 passed（验证 feature 门控）
cargo test --doc          # 47 passed, 0 ignored

# ⚠️ 不要跑 bench！仅维护基准代码，不主动执行。需要数据时由用户本人运行。
#
# 以下命令仅作「基准存在哪些目标」的记录，供理解代码结构用：
#   5 个目标，按被测子系统拆分；各自声明 required-features
cargo bench --bench pixel_and_buffer        # 像素读写/清屏/双缓冲/blit/子视口
cargo bench --bench primitives              # 线段/三角形/矩形/圆/弧/掩码混合
cargo bench --bench color_and_angle         # 颜色、格式编解码、角度三角函数
cargo bench --bench programmable_pipeline   # 可编程管线（需 programmable）
cargo bench --bench font                    # 位图与 TrueType 字体（需字体 feature）
cargo bench                                 # 全部目标

# 抖动的机器上收窄置信区间（默认 282 条 / 1s 预热 / 3s 测量 / 100 采样，约 18 分钟）
BENCH_MEASURE_MS=10000 BENCH_SAMPLES=300 cargo bench --bench primitives
BENCH_QUICK=1 cargo bench --bench color_and_angle   # 冒烟档，数据不可引用
# 每个目标启动时会打印一次抖动自检结论（可信 / 不可信）
# 基准设计与解读见 benches/README.md

# 示例（均输出 BMP 到仓库根目录）
cargo run --example basic
cargo run --example gallery
cargo run --example programmable
cargo run --example subviewport
```

## 注意事项

- **三角形扫描转换用半开扫描线规则**（`f80d896` 修复）。拆分点 `y2` **只属于
  下半段**：上半段扫 `y1..y2`、下半段扫 `y2..=y3`，故每行恰好发射一次；
  `y2 == y3`（水平底边）时下半段只剩一行、两条边重合，跨度直接取
  `min(x2, x3)..max(x2, x3)`——**不要**改回把 `x2` 当 `base_x` 去插值，
  那会让覆盖依赖顶点顺序（原缺陷：同一三角形两种绕向相差 1 个像素）。
  改这段代码时注意：像素**集合**比较对重复写完全免疫，
  只有 `reference_triangle_write_counts` 的逐像素计数能守住「每行只扫一遍」。
- **`sub_view` 在 `x > 0` 时恒定少一行**：这不是保守取舍，而是 `Renderer::new`
  要求切片至少 `pitch × height` 的硬约束（窗口切片取自
  `&mut fb[off..off + pitch × height]`，末行也必须凑满一整行）。按可见像素算本可
  多一行，但那样切片会超出缓冲区、构造时 panic。详见 `sub_view` 文档
  「为什么 `x > 0` 时恒定少一行」小节。
- **`clip_line` 的左/上边界分支是死代码**：`compute_code` 里的 `LEFT`/`BOTTOM`
  判断永不成立——`Pixel` 字段是 `usize`，负坐标无法表达。
- **`fill_triangle` 的顶点裁剪是「钳位」不是几何裁剪**：屏幕外顶点被
  `min(max_x/max_y)` 拉到边界，斜率和形状随之改变，屏幕内结果**不等于**
  原始三角形与视口的交集。这是既有的公开语义，由
  `fill_triangle_clamps_out_of_screen_vertices_instead_of_clipping` 与
  `fill_triangle_clamps_huge_vertices_without_narrowing_bug` 钉住。
  改成固定容量 Sutherland–Hodgman 裁剪属行为级变更，须单独评审。
- **`FramebufferInfo` 的字段是 `pub`，构造期校验管不到构造之后**：所有按布局
  做的算术（`buffer_size`、`pixel_offset`、`clear`、`swap_buffers`、
  `fill_span_*`、`blit*`、`sub_view_geometry`）都必须独立 `checked_*`，
  溢出即 panic 或静默返回，**不能裸乘**——回绕出的偏移落在缓冲区内，
  会让 `get_unchecked_mut` 写到错误的像素上。
- `release` profile 开了 `panic = "abort"`；`bench` profile 未开（基准需要
  unwind 以捕获 panic）。
- `no_std` 但库内不含 `unsafe` 的 `extern "C"` ABI，全部通过安全 API 暴露。
- 测试规模与分布见 `target/TEST_PLAN.md` §6.1（该文件在 `.gitignore` 内）。
- **不要跑 bench**：仅维护基准代码，不主动执行采集。需要数据时由用户本人运行。

## 已修复的历史缺陷（均有回归护栏）

| 缺陷 | 影响 |
|------|------|
| `color!` 宏无法匹配字符串字面量 | 文档宣传的 `color!("#RRGGBB")` 从未能编译 |
| `clear` 无条件 memcpy 16 字节模式 | `pitch > row_len && row_len < 16` 时污染行填充字节 |
| `fill_visible_horizontal_span` 越界写 | **内存破坏**（release 静默写坏内存） |
| `fill_circle`/`fill_arc` 的 `cx ± half` i64 溢出 | 大坐标 panic |
| `ceil_to_usize(f32::NAN)` 返回 1 | 三角形 bbox 凭空多扩一像素 |
| `floor_to_i32` 饱和后 `truncated - 1` 下溢 | 纹理采样遇 `±∞` 坐标时 panic（已改 `saturating_sub`） |
| `FramebufferInfo::new` 用 `saturating_mul` | `width = usize::MAX` 时饱和值让不存在的布局「通过」校验 |
| `buffer_size()` / `pixel_offset` 等裸乘 | 回绕出的偏移落在缓冲区内 → `get_unchecked_mut` 写错像素（静默渲染错误） |
| `fill_triangle` 上下段都是闭区间 | `y2` 行被扫两次，半透明色重复混合 |
| `fill_triangle` 水平底边取 `x2` 当端点 | 三顶点 Y 排序不唯一 → 覆盖依赖绕向（相差 1 像素） |
| 可编程管线线段 DDA 重复片元 | 短线段多次着色：半透明线段比同位置点更亮，亮度还取决于方向 |

## 浮点取整：`no_std` 下没有 `f32::floor`

无 std 无 libm 时 `floor` / `round` / `ceil` / `trunc` / `sqrt` **全部不可用**
（已实测确认）。库内因此有四份手写取整：`utils::floor_to_usize`、
`utils::ceil_to_usize`、`texture::floor_to_i32`、`font::PenX::quantize`。
改任一份都要保持三条共同契约：

1. **`NaN` 显式归零**，不能靠 `as` 的饱和转换蒙对（`ceil_to_usize` 曾返回 1）。
2. **饱和后不再做加减**。`as` 在 `|value| ≥ i32::MAX` 饱和到 `i32::MAX`，
   此时负数分支的 `- 1` 会下溢 panic——必须 `saturating_sub(1)`。
3. **`ceil` / `floor` 只对非负值有意义**，`utils` 侧靠调用方保证。

`PenX::quantize` 多一条：**进位条件必须比对小数部分**
（`scaled - truncated >= 0.5`）。写成 `scaled >= truncated as f32` 时，整数刻度
（如 `0.5px` → `32.0`）会让条件恒成立，每个刻度都被多进一位，1000 次步进漂移
15.6 px。
