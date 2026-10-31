# src/ 实现层

**生成时间:** 2026-10-06 · 11 个 `.rs` + 1 个子目录，约 13 300 行（含约 8 000 行测试）

全部手写实现，无 `unsafe extern "C"` FFI，无代码生成（唯一例外：`angle.rs`
的 257 项正弦波表是脚本产物后手抄入库的）。

## 模块职责

| 文件 | 行数 | 职责 | 出口 |
|------|------|------|------|
| `core.rs` | 5891 | `Renderer` + 全部固定功能图元 + 4 个字节序写宏 | `pub struct Renderer<'a>` |
| `color.rs` | 2275 | `Color`、`PixelFormat`、色彩常量、**内嵌 `pub(crate) mod swar`** | `pub struct Color` / `pub enum PixelFormat` |
| `angle.rs` | 908 | 圈值 `Angle`、`Wedge`（弧形判定）、257 项正弦波表 | `pub struct Angle` |
| `utils.rs` | 1383 | `sub_view` 子视口、`clip_line`（Cohen–Sutherland）、`pattern_fill`、f32 取整助手 | 若干 `pub fn` |
| `error.rs` | 324 | 全库错误类型唯一定义处 + `Display` 契约测试 | `ColorParseError` / `DrawError` / `TextureError` |
| `simd.rs` | 389 | 5 条 ISA 分支的 `memcpy`（AVX-512/AVX2/SSE2/NEON/RVV） | `pub unsafe fn memcpy` |
| `framebuffer.rs` | 283 | 布局元数据 + 尺寸溢出断言 | `pub struct FramebufferInfo` |
| `pixel.rs` | 49 | 二维坐标 + `pixel!` 宏 | `pub struct Pixel` |
| `core/ssaa.rs` | 388 | 超采样抗锯齿：`Renderer::ssaa` / `ssaa_owned` + 面积平均降采样 | `pub enum SsaaFactor` |
| `font/` | — | 字体 trait + 位图/TrueType 实现 | `pub trait Font` / `pub mod ttf` |
| `main.rs` | 39 | **bin 目标**：1280×720 BGR24 画一条线后落盘 | — |
| `programmable/` | 3507 | feature 门控的 f32 可编程管线 | 见 `programmable/AGENTS.md` |

`core.rs` 的 5891 行里约 4061 行是 `#[cfg(test)]`（96 个测试，从 `:1831` 起），
实际实现约 1830 行——读它时先跳过测试模块。

全库测试分布（`cargo test --lib` = 431 passed）：
`core` 96、`core::ssaa` 6、`programmable`（含 cache/shader）125、`utils` 40、
`color` 39、`angle` 27、`framebuffer` 16、`simd` 7、`error` 7、`font` 36，
另有 `pixel` 的 doctest。

`core/ssaa.rs` 的 `ssaa_owned_matches_scratch_version` 必须带
`#[cfg(feature = "alloc")]`——`ssaa_owned` 本身被该 feature 门控，
漏了它 `--no-default-features` 的测试构建会直接编译失败。

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
| `write_color_at_offset` | `:361` | `pub(crate)`；按 `fmt_*` 缓存把 `Color` 落盘 |
| `fill_span_opaque` / `fill_span_alpha` | `:641` / `:701` | 行内跨度填充，4bpp 走 SWAR；进入 unsafe 前 checked 算偏移与跨度 |
| `fill_visible_horizontal_span` | `:796` | 带可见区裁剪的跨度入口 |
| `interpolate_scan_x` | `:839` | 扫描线插值，**关联函数**（`fn` 无 `self`） |
| `build_pattern` | `:1024` | 为 `pattern_fill` 构造 16 字节重复模式 |
| `draw_line_unchecked` | `:1114` | **私有** Bresenham 受限内核，`draw_line` 的唯一实现 |
| `disk_params` / `disk_half_at` | `:1414` / `:1440` | 圆盘精确语义：半径钳制 + 每半行开方边界 |
| `walk_disk_rows` / `walk_disk_boundary` / `emit_boundary_row` | `:~1459`–`:~1523` | 圆/弧的行遍历三层分解 |

公开入口里需要留意的几条：`pixel_offset`（`:344`，溢出即 panic）、
`blit` / `blit_mask`（`:424` / `:497`，尺寸乘积溢出即静默返回）、
`clear`（`:929`）、`fill_triangle`（`:1288`）、`swap_buffers`（`:1774`）。

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
cargo test --lib                       # 431 passed
cargo test --lib --no-default-features # 250 passed（验证 feature 门控）
cargo build --bins                     # 验证 src/main.rs 这个 bin 目标
```

## `font/` 的契约（`a53a335` 之后的接口变更）

字体接口把**度量**与**光栅化**分开，因为 fontdue 的 `rasterize` 返回
`(Metrics, Vec<u8>)`——**拥有所有权**的临时缓冲，借用式返回类型
（`Option<Char<'_>>` 借 `&self`）对它是不可实现的。

| 类型 | 存储来源 | 需要 `alloc` | 容量不足时 |
|------|----------|--------------|-----------|
| `GlyphLayout` / `FontMetrics` / `PenX` | 纯值 | 否 | — |
| `Font::rasterize` | 调用方借出的 `&mut [u8]` | 否 | **panic** |

`rasterize` 选 panic 而非静默返回，与 `blit_mask` 的策略**相反且刻意**：
缓冲尺寸是调用方拿上一步 `layout()` 的返回值算出来的，算错属调用方 bug。
库内**不做任何分配**——`draw_char` / `draw_text` 的 scratch 由调用方传入。

改动 `font/` 时必须守住的三条：

1. **`draw_glyph` 裁左边界时必须同时裁源掩码**：`bearing_x` 为负（悬挂部 /
   斜体）时，`Pixel::x` 是 `usize` 表达不了负坐标，所以要跳过源的前若干列
   **并把传给 `blit_mask` 的 `mask_pitch` 从 `w` 改成 `draw_w`**。只裁目标
   会让字形整体右移；`mask_pitch` 忘改则被 `blit_mask` 的长度校验静默拒绝，
   表现是带悬挂部的字（`f`/`j`/斜体）**凭空消失**。回归护栏是
   `draw_glyph_clips_source_left_edge_for_negative_bearing`。
2. **顶部裁剪必须改写 `dst_y` 本身**，不能只用它算 `src_row_skip`：
   负值 `as usize` 会得到巨大正数，位图被推到画面外，裁剪等于没做。
   护栏：`draw_glyph_clips_rows_above_framebuffer_top`。
3. **`PenX` 的小数位是 16 不是 6**：量化误差随字距线性累积，1/64 px 的网格
   表示不了常见的比例 advance（`1+1/128`），1600 字一行漂移 12.5 px。
   1/65536 px 下常见 advance 全部精确。代价是 `i32` 画笔范围降到 ±32768 px。
   护栏：`pen_x_keeps_subpixel_advance_exact`。

**字号是 `f32`，不是无浮点**：`font_truetype` 内部的 fontdue 本就以浮点累加
（`scale_factor(px)`、亚像素偏移），接口不再假装能定点。`font_bitmap` 路径
不产生任何浮点运算。内核态目标关闭 `font_truetype`——这是「无浮点契约」
在新配置下的准确表述，改 `Cargo.toml` 的 feature 时同步改本段。

`no_std` 下 `f32::round` / `f32::trunc` **均不可用**（与
`utils::floor_to_usize` 同源的限制），`PenX::quantize` 用 `as i32` + 小数部分
比较手写四舍五入。进位条件必须比对**小数部分**（`scaled - truncated >= 0.5`），
写成 `scaled >= truncated as f32` 会让整数刻度每次都被多进一位。

`tests/fixtures/minimal_ascii.ttf` 是 **DejaVu Sans** 经 fontTools 子集化的
10768 字节 / 7 字形资产（2048 unitsPerEm）。选它是因为 6 个字形覆盖三类难点：
`W`=2025（最宽）、`A`=1401、`g`=1300（下伸部）、`\|`=690、`i`/`j`=569
（最窄，且 `j` **有悬挂部**，`bearing_x` 为负）。advance 跨度 3.6 倍，等宽
fixture 测不出步进逻辑错误；`j` 的悬挂部是 `draw_glyph` 左边界裁剪的真实触发
条件。生成时须 `drop_tables` 掉 hinting 程序（`fpgm`/`prep`/`cvt `）与
`GSUB`/`GPOS`——fontdue 不做 hinting，留下它们会让体积从 10 KB 涨到 16 KB。
许可证（Bitstream Vera）允许改名分发，name 表已只保留版权与许可证两项。

**分配契约**：`alloc` feature 是「库内不做分配」这条默认约定的唯一出口，
只新增 `draw_char_owned` / `draw_text_owned` 两个方法。它们与带 scratch 的
版本共用 `layout_text` 内核，并由 `owned_variants_match_scratch_variants_pixel_for_pixel`
钉住「两条路径逐字节相同」。签名**不按 feature 分裂**——`draw_char` /
`draw_text` 在任何构建下都收 scratch 参数，调用点不会因 feature 变化而改代码。

改动 `core.rs` 后务必跑 `cargo test --lib`——它的 96 个测试里有大量
像素级覆盖等价性断言（线段收窄、圆盘钳制、SWAR 对齐回退、跨度求交顺序），
那些是防止性能优化改坏语义的护栏，不是可选的覆盖率。

特别注意 `fill_visible_horizontal_span` 的**顺序契约**：必须
「排序 → 求交 → 隐式夹取」。改成「各自 clamp」会在屏幕边缘凭空画出
一列（完全越界的跨度被压成边缘单像素）；去掉 `swap` 排序则会丢掉
`fill_triangle` 的反序跨度（实测多数绕向下 9/12 行是反序的）。

## 布局尺寸算术必须 checked

`FramebufferInfo` 的 `width` / `height` / `pitch` 都是 `pub`，构造之后
调用方仍可直接改写，**构造期的长度校验对改写后的布局没有约束力**。
因此凡是按布局做算术、以及凡是进入 `get_unchecked_mut` / 裸指针 `add` /
`from_raw_parts_mut` 的路径，都必须独立 `checked_*`，且必须在进入 unsafe
**之前**完成：

| 路径 | 溢出时的行为 | 理由 |
|------|-------------|------|
| `pixel_offset` / `fill_span_*` / `clear` / `swap_buffers` | `panic` | 构造期不变量被破坏 → fail-fast |
| `blit` / `blit_mask` 的尺寸乘积 | 静默返回 | 与它们既有的「参数不合法即不写」契约一致 |

不能裸乘的理由是静默错误比 panic 难发现：回绕出的偏移通常**落在缓冲区内**，
于是 `get_unchecked_mut` 写到错误的像素上，既不越界也不报错。

回归护栏：`core.rs` 的 `pixel_offset_panics_when_*`、
`clear_panics_on_overflowed_layout_instead_of_wrapping`、
`swap_buffers_panics_on_overflowed_layout_instead_of_wrapping`、
`blit_returns_silently_when_size_products_overflow`、
`blit_mask_returns_silently_when_mask_pitch_times_height_overflows`，
以及 `framebuffer.rs` 的四条构造期溢出测试。它们全部用 4×4 帧缓冲做底座，
只验证算术被拒绝，不真实分配大内存。
