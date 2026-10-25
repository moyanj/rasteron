# examples — 4 个示例 + 共享 BMP 工具

**生成时间:** 2026-10-10 · 5 文件

这些是**独立 bin 目标**（`std` 可用，不是 `no_std`），cargo 按
「每个子目录一个 `main.rs`」自动发现，无需在 `Cargo.toml` 注册。

## 组成

| 路径 | 行数 | 演示内容 | 产出 |
|------|------|----------|------|
| `basic/main.rs` | 223 | 全部 2D 绘图原语；双缓冲渐变天空背景 | `basic-example.bmp`（640×480 BGR24） |
| `gallery/main.rs` | 1229 | 全能力展厅：28 格编号网格，覆盖图元、双缓冲、子视口、字体、可编程管线、像素格式 | `gallery-example.bmp`（1280×1086 BGR24） |
| `programmable/main.rs` | 106 | `Passthrough2D` + 自定义 FS，`Points` 画点阵 | `programmable-example.bmp` |
| `subviewport/main.rs` | 194 | `sub_view_with_back_buffer` 实现无状态裁剪的 UI 面板 | `subviewport-example.bmp` |
| `shared/bmp.rs` | 164 | BMP 落盘（24-bit BGR / 32-bit BGRA）+ 棋盘格源数据生成 | 库 |

`gallery` 占全目录 64%，它不是「演示」而是**人工比对的基准图**：
每格有边框、按行取色的强调色块，标题由库自己的 `draw_text` + 内置 8×16 点阵
字体绘制（不再维护手写字库），出错时能一眼定位到具体图元。改动任何图元后
重新跑它，与上一版输出对比。

## 两条硬约定

**1. 复用 `shared/bmp.rs` 用 `#[path]`，不是模块树**

```rust
#[path = "../shared/bmp.rs"]
mod bmp;
```

`shared/` 没有 `main.rs`，所以它不是 cargo target；每个 example 用上面的
`#[path]` 把它拉进来。`src/main.rs`（bin 目标）也用同一手法反向引用本目录。
`shared/bmp.rs` 顶部有 `#![allow(dead_code)]`——每个 example 只用到其中
一部分子函数，缺了这个 allow 就会满屏警告。**新增共享工具时沿用此结构，
不要为它新建 crate 或挪进 `src/`。**

**2. 输出全部是 `Bgr24`**

`write_bmp` 只对 `Bgr24` 与 `Rgba8888` 原生落盘，其余格式先经
`convert_to_bgr24` 转一遍（该函数假设 `pitch == width × bpp`，即无行填充）。
示例统一选 `Bgr24` 以避开转换开销。产物在仓库根目录，`.gitignore` 已排除
`*.bmp`——**不要 `-f` 强推**。

## gallery 的内部结构

```rust
const WIDTH: usize = 1280;            // 画布宽，4 列均分
const CELL_W: usize = WIDTH / COLS;   // 320
const CELL_H: usize = 128;            // 每格内容区高
const TITLE_H: usize = 26;            // 8x16 点阵行高 16，上下各留 5 px
const GAP: usize = 8;
const ROWS: usize = 7;                // 28 格 = 7 行 × 4 列
HEIGHT = ROWS * (CELL_H + TITLE_H) + GAP   // 1086
const NOTE_DY: usize = CELL_H - GLYPH_H;   // 格底注释行的 y，恰好贴住内容区底边
```

`cell_origin(col, row)` 算每格左上角；`draw_frame(r, origin, index, name)`
画边框 + 强调色条 + 标题并返回内容区左上角。格子登记在 `main` 的 `cells`
表（`[(&str, CellFn); 28]`）里：写一个 `fn cell_xx(r, c)` 按 `c`（内容区
左上角）绘制，再在表中登记——序号即格号，由表的位置算出，不必硬编码；
`index` 决定强调色（`ACCENTS[index / COLS % ACCENTS.len()]`）。

### 三处需要记住的坑

**1. 标题字体不缩放。** 内置 8×16 点阵字体只有一份位图，`px` 参数只做
`> 0` 与有限性检查，所以标题一律 1 倍绘制（`TITLE_PX = 16.0`），**别指望
靠调大 `px` 得到 2 倍字**。`GLYPH_H = 16` 同时决定 scratch 的下限容量。

**2. `Renderer::blit` 要求源格式与目标格式完全一致**（`src/core.rs:434`
的第一个短路条件），格式不同直接静默返回。而「同一图案在 8 种格式下长什么样」
恰恰需要跨格式贴图——所以格内用私有辅助 `blit_converted()`：逐像素走一趟
`PixelFormat::read_color`，再交给 `set_pixel` 按主画布（恒为 `Bgr24`）的通道
顺序写回。两者互为逆运算，于是显示的是**格式差异本身**而非转换误差。

**3. `main` 必须分三阶段。** `sub_view` 需要父缓冲的裸切片，而 `Renderer`
在存活期间独占它，所以子渲染器那一格（11）不能和主渲染器同处一个作用域：
① 主渲染器画 27 格 → `swap_buffers()` → ② 作用域结束后 `fb` 归还，用
`sub_view_with_back_buffer` 画 11 → ③ 写文件。第一阶段只留占位函数
`cell_placeholder_subview`。

### Feature 前置

本示例展示 `programmable` 与两个字体 feature，需**默认 feature 全开**。
示例目录整体按默认 feature 编译——`cargo build --no-default-features
--examples` 本来就会因 `examples/programmable` 而失败，故格子里不做
`#[cfg(feature = ...)]` 拆分（数组长度是编译期常量，拆了反而编不过）。

## 已修复：`gallery` 曾调用私有方法

原先 `gallery/main.rs` 在 `unsafe {}` 块内调用 `draw_line_unchecked`，而该方法
在 `src/core.rs` 是**私有**的（同类中的 `set_pixel_unchecked` 才是 `pub`），
导致 `cargo build --examples` 与 `cargo test` 失败。

现按推荐的方案改成了公共 API：受安全与非安全版本对照的那两条水平线都改用
`r.draw_line(...)`（`gallery/main.rs` 的 `cell_line`，仍保留这一对照）。
`draw_line` 内部已经
通过 `clip_line` + 区间收窄完成裁剪，语义等价，**无需库侧放开 API**。

`lib.rs` 的固定功能图元表格里原先宣传的 `draw_line_unchecked` 一并删除——
它始终不是公开方法，以代码为准。

## 验证

```bash
cargo run --example basic
cargo run --example gallery      # 改动图元后的比对基准
cargo run --example programmable
cargo run --example subviewport

cargo build --examples           # 当前 PASS
cargo build --benches            # benches 也用 programmable，验证共享链路
```

所有示例的帧缓冲都是 `vec![]` 分配后交给渲染器，**库自身不分配**这一点
在每个示例里都能直接观察到。
