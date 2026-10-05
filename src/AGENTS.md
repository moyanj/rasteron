# src/ 实现层

**生成时间:** 2026-10-06 · 11 个 `.rs` + 1 个子目录，约 13 300 行（含约 8 000 行测试）

全部手写实现，无 `unsafe extern "C"` FFI，无代码生成（唯一例外：`angle.rs`
的 257 项正弦波表是脚本产物后手抄入库的）。

## 模块职责

| 文件 | 行数 | 职责 | 出口 |
|------|------|------|------|
| `core.rs` | 5253 | `Renderer` + 全部固定功能图元 + 4 个字节序写宏 | `pub struct Renderer<'a>` |
| `color.rs` | 2006 | `Color`、`PixelFormat`、色彩常量、**内嵌 `pub(crate) mod swar`** | `pub struct Color` / `pub enum PixelFormat` |
| `angle.rs` | 908 | 圈值 `Angle`、`Wedge`（弧形判定）、257 项正弦波表 | `pub struct Angle` |
| `utils.rs` | 1328 | `sub_view` 子视口、`clip_line`（Cohen–Sutherland）、`pattern_fill`、f32 取整助手 | 若干 `pub fn` |
| `error.rs` | 326 | 全库错误类型唯一定义处 + `Display` 契约测试 | `ColorParseError` / `DrawError` / `TextureError` |
| `simd.rs` | 211 | 5 条 ISA 分支的 `memcpy`（AVX-512/AVX2/SSE2/NEON/RVV） | `pub unsafe fn memcpy` |
| `framebuffer.rs` | 184 | 布局元数据 + pitch 断言 | `pub struct FramebufferInfo` |
| `pixel.rs` | 51 | 二维坐标 + `pixel!` 宏 | `pub struct Pixel` |
| `main.rs` | 39 | **bin 目标**：1280×720 BGR24 画一条线后落盘 | — |
| `programmable/` | 2866 | feature 门控的 f32 可编程管线 | 见 `programmable/AGENTS.md` |

`core.rs` 的 5253 行里约 3600 行是 `#[cfg(test)]`（83 个测试），
实际实现约 1650 行——读它时先跳过测试模块。

全库测试分布（`cargo test --lib` = 274 passed）：
`core` 83、`programmable`（含 cache/shader）61、`color` 46、`angle` 40、
`utils` 40、`framebuffer` 13、`error` 7、`simd` 7，另有 `pixel` 的 doctest。

## 内部依赖图

```
error  ←  color  ←──────────┐
              ↑              │
angle ←──── core ────────────┤（core 用 utils::clip_line / pattern_fill / sat_i64）
  ↑        ↑   ↓             │
framebuffer  │  utils ───────┘  ← utils 用 core::Renderer 构造子渲染器
  ↑          │    ↑
pixel ───────┘    ├─ programmable (feature)
                 └─ simd (feature)
```

**`core` ↔ `utils` 是有意的双向依赖**：`utils::sub_view` 必须造
`core::Renderer` 才能给出子视口，而 `core` 的线段裁剪与行填充要用
`utils` 的内核。改动其中一个时注意别把另一侧拖进 feature 门控的死角
（`utils` 已在用 `#[cfg(feature = …)]` 隔离 `programmable` / `simd` 分支，
`core` 里 `simd` 门控只有 4 处）。

## core.rs 解剖

**4 个写字节序的宏**（`:12`–`:85`）承担 RGB/BGR × 普通/裸指针的组合展开：
`write_rgb` / `write_rgb_blend` / `write_rgb_blend_ptr` / `write_rgb_ptr`。
新增像素格式或写入路径时先看这四个，不要另起炉灶。

**私有内核**（都不在公开 API 里，改动需同步 `core.rs` 内测试）：

| 内核 | 位置 | 作用 |
|------|------|------|
| `write_color_at_offset` | `:347` | `pub(crate)`；按 `fmt_*` 缓存把 `Color` 落盘 |
| `fill_span_opaque` / `fill_span_alpha` | `:591` / `:643` | 行内跨度填充，4bpp 走 SWAR |
| `fill_visible_horizontal_span` | `:709` | 带可见区裁剪的跨度入口 |
| `interpolate_scan_x` | `:740` | 扫描线插值，**关联函数**（`fn` 无 `self`） |
| `build_pattern` | `:890` | 为 `pattern_fill` 构造 16 字节重复模式 |
| `draw_line_unchecked` | `:977` | **私有** Bresenham 受限内核，`draw_line` 的唯一实现 |
| `disk_params` / `disk_half_at` | `:1223` / `:1249` | 圆盘精确语义：半径钳制 + 每半行开方边界 |
| `walk_disk_rows` / `walk_disk_boundary` / `emit_boundary_row` | `:1268`–`:1332` | 圆/弧的行遍历三层分解 |

`CircleKind`（`:123`）是 `#[cfg(test)]` 专用的描边/填充枚举，
让测试调用点自解释而非传 `true`/`false`——新增图元测试时沿用此风格。

## 本目录特有约定

- **f32 隔离**：`utils` 里的 `floor_to_usize` / `ceil_to_usize` /
  `normalized_to_screen` 与 `color` 里的 `blend_add` / `blend_multiply` /
  `blend_premultiplied` 全部带 `#[cfg(feature = "programmable")]`。
  新增任何用浮点的东西必须同样门控，否则 `--no-default-features` 会炸。
  **注意**：`utils.rs` 的测试模块里用到 `Viewport` / `Bounds` /
  `normalized_to_screen` 的**测试函数本身**也要逐个加
  `#[cfg(feature = "programmable")]`，否则
  `cargo test --no-default-features --lib` 编译失败。
- **feature 门控的扩展方法**：`programmable/mod.rs` 里写
  `impl<'a> Renderer<'a>` 给 `core::Renderer` 追加 `draw_arrays` /
  `draw_indexed`。同 crate 内合法，**不要**为此把 `core.rs` 引入
  `programmable` 的依赖。
- **饱和转换**：`utils::sat_i64`（`:~300`）是 `usize → i64` 的唯一入口，
  `core.rs:978` 的注释解释了为什么不能用 `as`。
- **`as` 只用于 `f32 as usize` 之外的自解释窄化**；`usize → i64`、
  坐标相减一律走饱和/检查路径。
- **`no_std` 测试**：任何用到 `String` / `Vec` / `format!` 的
  `#[cfg(test)]` 模块都要显式 `extern crate alloc;`；用到 `vec!` 还要
  `use alloc::vec;`，用到 `Vec` 还要 `use alloc::vec::Vec;`。范例见
  `error.rs` 与 `color.rs` 的 `mod tests`。
  `criterion` 风格的 `black_box` 走 `core::hint`。
- **测试风格**：几乎全部用 `should_panic` + 消息子串断言（如
  `framebuffer.rs:76`）或精确像素断言；`CircleKind` 式的自解释枚举在
  测试里代替布尔参数。

## 快速验证

```bash
cargo check --no-default-features      # 唯一能验证「无浮点契约」的门径
cargo test --lib                       # 274 passed
cargo test --lib --no-default-features # 198 passed（验证 feature 门控）
cargo build --bins                     # 验证 src/main.rs 这个 bin 目标
```

改动 `core.rs` 后务必跑 `cargo test --lib`——它的 83 个测试里有大量
像素级覆盖等价性断言（线段收窄、圆盘钳制、SWAR 对齐回退、跨度求交顺序），
那些是防止性能优化改坏语义的护栏，不是可选的覆盖率。

特别注意 `fill_visible_horizontal_span` 的**顺序契约**：必须
「排序 → 求交 → 隐式夹取」。改成「各自 clamp」会在屏幕边缘凭空画出
一列（完全越界的跨度被压成边缘单像素）；去掉 `swap` 排序则会丢掉
`fill_triangle` 的反序跨度（实测多数绕向下 9/12 行是反序的）。
