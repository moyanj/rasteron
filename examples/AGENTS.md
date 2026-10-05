# examples — 4 个示例 + 共享 BMP 工具

**生成时间:** 2026-10-06 · 5 文件

这些是**独立 bin 目标**（`std` 可用，不是 `no_std`），cargo 按
「每个子目录一个 `main.rs`」自动发现，无需在 `Cargo.toml` 注册。

## 组成

| 路径 | 行数 | 演示内容 | 产出 |
|------|------|----------|------|
| `basic/main.rs` | 223 | 全部 2D 绘图原语；双缓冲渐变天空背景 | `basic-example.bmp`（640×480 BGR24） |
| `gallery/main.rs` | 1000 | 全部固定功能图元排成 4 列 × 3 行带编号网格 | `gallery-example.bmp`（960×760 BGR24） |
| `programmable/main.rs` | 106 | `Passthrough2D` + 自定义 FS，`Points` 画点阵 | `programmable-example.bmp` |
| `subviewport/main.rs` | 194 | `sub_view_with_back_buffer` 实现无状态裁剪的 UI 面板 | `subviewport-example.bmp` |
| `shared/bmp.rs` | 164 | BMP 落盘（24-bit BGR / 32-bit BGRA）+ 棋盘格源数据生成 | 库 |

`gallery` 占全目录 59%，它不是「演示」而是**人工比对的基准图**：
每格有边框、强调色块、5×7 点阵字模标题（2 倍放大），出错时能一眼定位到
具体图元。改动任何图元后重新跑它，与上一版输出对比。

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
const WIDTH: usize = 960;   // 画布宽，4 列均分
const CELL_W: usize = WIDTH / 4;
const CELL_H: usize = 170;  // 每格内容区高
const TITLE_H: usize = 20;  // 5x7 字模 2 倍放大后 14px + 上下各 4px
const GAP: usize = 8;
const ROWS: usize = 3;      // 12 格 = 3 行 × 4 列
HEIGHT = ROWS * (CELL_H + TITLE_H) + GAP
```

`cell_origin(col, row)` 算每格左上角；`draw_frame(r, origin, index, name)`
画边框 + 标题条并返回内容区左上角。加新图元测试时：写一个 `fn case_NN(r, c)`
按 `c` 计算原点，调 `draw_frame` 拿内容区，再画图，最后在 `main` 的网格里
登记——`index` 决定强调色（`ACCENTS[index % ACCENTS.len()]`）。

## 已修复：`gallery` 曾调用私有方法

原先 `gallery/main.rs` 在 `unsafe {}` 块内调用 `draw_line_unchecked`，而该方法
在 `src/core.rs` 是**私有**的（同类中的 `set_pixel_unchecked` 才是 `pub`），
导致 `cargo build --examples` 与 `cargo test` 失败。

现按推荐的方案改成了公共 API：受安全与非安全版本对照的那两条水平线都改用
`r.draw_line(...)`（`gallery/main.rs` 的 `cell_line`）。`draw_line` 内部已经
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
