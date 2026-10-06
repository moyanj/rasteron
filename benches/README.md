# 基准说明

本目录的 criterion 基准按**被测子系统**拆成 5 个独立目标。每个目标都是
自己的 `criterion_main`，可以单独跑，也可以 `cargo bench` 一次跑完。

| 目标 | 覆盖内容 | `required-features` |
|------|----------|---------------------|
| `pixel_and_buffer` | 像素读写、清屏、双缓冲呈现、`blit`、子视口 | 无 |
| `primitives` | 线段、三角形、矩形、圆、圆弧、`blit_mask`、混合矩阵 | 无 |
| `color_and_angle` | 颜色运算、8 种格式编解码、`Angle` 三角函数、`Wedge` | 无 |
| `programmable_pipeline` | 顶点/片元着色、图元装配、混合模式、纹理采样、顶点缓存 | `programmable` |
| `font` | 位图（8×16）与 TrueType 两条后端的度量/光栅化/绘制 | `font_bitmap` |

`benches/shared/mod.rs` 是共享脚手架（各文件以 `mod shared;` 引入）。

## 快速开始

```bash
# 全部目标（默认参数下约 18 分钟，282 条）
cargo bench

# 单个目标
cargo bench --bench primitives

# 只跑某个分组（criterion 的位置参数是过滤器，支持子串匹配）
cargo bench --bench primitives -- "fill_triangle"
cargo bench --bench pixel_and_buffer -- "clear/"

# 冒烟档：只验证能否跑通且不 panic，数据不可引用
BENCH_QUICK=1 cargo bench --bench color_and_angle
```

### 为什么默认参数是 1s / 3s / 100

criterion 的时间预算是**每条基准固定**的，与本条的快慢无关。所以
**单条时长 × 条数 = 总耗时**：282 条约 18 分钟，把单条从 4 秒加到 11 秒
就是 50 分钟。

默认取 1 秒预热 + 3 秒测量 + 100 采样，实测中位数置信区间已在 ±0.3% 内、
离群点 0~1%，足以支撑「比较两个实现的相对快慢」这一主要用途。刻意不用
更长的时长：把置信区间从 ±0.3% 收到 ±0.1% 要付三倍时长，而那点差异
通常小于你关心的实现区别。需要更紧的区间时用环境变量加严。

### 条数是刻意控制的

基准覆盖的是**有区分度的维度**，不是 API 的笛卡尔积。例如：

- `PixelFormat` 有 8 种，但编解码只取 4 档代表（3bpp / 存 alpha 4bpp /
  不存 alpha 4bpp / 565）——同类内两个成员只在移位常量上不同；
- TrueType 字形只取 `W`（最宽）与 `j`（唯一负 bearing）；
- 字号只取两档，因为光栅化成本基本是 `px²` 的函数。

新增覆盖时请守住这条：**加一个有分支差异的维度，而不是把已有的再乘一遍**。

## 关于测量波动

基准跨四个数量级（2 ns 的颜色构造到 1 ms 的 1080p 填屏），波动来源不同：

- **短基准（ns 级）** 主要受 CPU 调频影响。`schedutil` 之类的 governor
  会在任务间隙降频，预热不足时前后采样的频率不同。
- **长基准（µs–ms 级）** 受内存带宽与后台进程抢占影响，表现为**离群点**
  而非噪声。

每个目标启动时会先打印一行抖动自检，它用一块 256 KiB 缓冲区（跨 L2，
对带宽与抢占都敏感）采样 64 次，给出中位数、最慢/中位比与离群比例：

```
抖动自检: 中位 77977 ns, 最慢/中位 1.30x, 离群 0.0%, 样本 64 → 可信
```

判据是「最慢样本 < 中位数 3 倍」且「离群占比 < 5%」。判为**不可信**时，
先排除干扰再采信数字。

### 抖动大时怎么办

```bash
# 1. 加大测量时间与采样数（最直接）
BENCH_MEASURE_MS=10000 BENCH_SAMPLES=300 cargo bench --bench primitives

# 2. 把 CPU governor 固定为 performance（需要 root，且会增加功耗/发热）
sudo cpupower frequency-set -g performance

# 3. 确认没有后台重负载
cat /proc/loadavg
```

可用的环境变量：

| 变量 | 默认 | 说明 |
|------|------|------|
| `BENCH_WARMUP_MS` | 1000 | 预热时长（毫秒） |
| `BENCH_MEASURE_MS` | 3000 | 测量时长（毫秒） |
| `BENCH_SAMPLES` | 100 | 采样数 |
| `BENCH_QUICK` | — | 设为真值时启用冒烟档 |
| `BENCH_SKIP_JITTER` | — | 设为真值时跳过抖动自检打印 |

## 怎么读结果

criterion 的输出形如：

```
clear/rgba8888/1920x1080
                        time:   [446.08 µs 446.69 µs 447.32 µs]
                        thrpt:  [4.6356 Gelem/s 4.6422 Gelem/s 4.6485 Gelem/s]
```

- `time` 是 `[下界 中位数 上界]`，**引用中位数**。
- `thrpt` 只在标注了 `Throughput::Elements` 的分组出现，是归一化吞吐
  （元素/秒），跨尺寸比较时比绝对耗时更有意义。
- `change:` 行只在与上次运行有基线可比时出现。criterion 把基线存在
  `target/criterion/`，所以**同一台机器上的前后两次运行**才可比。

### 比较两次运行

```bash
# 跑一次，记录基线
cargo bench --bench primitives -- --save-baseline before

# 改代码后，与基线对比
cargo bench --bench primitives -- --baseline before
```

## 基准的设计约定

写新基准或用结果下结论前，需要知道这几条：

- **`PixFb` 是帧缓冲的唯一持有者**。`Renderer<'a>` 借用帧缓冲切片，
  而基准要在同一个 `Renderer` 上反复迭代，所以 `PixFb` 用一处
  `from_raw_parts_mut` 把借用延长为 `'static`，并断言切片满足
  `Renderer::new` 的 `pitch × height` 前置条件。双缓冲用 `DoubleFb`
  （同时持有前台与后台），**不要**把前台作为元组元素返回——那会让同一块
  内存出现两个可变别名。
- **输入一律 `black_box` 化**，且数据集尺寸由参数决定，防止整段绘制被
  常量折叠掉。
- **同一函数内的 `fb` 会被多个分组复用**，因此后跑的分组可能读到已被写过的
  目标像素。这是**刻意的**：它对应真实渲染循环里「同一帧缓冲连续绘制」的
  情形。跨函数不共享 `fb`，所以不同 bench 函数之间无污染。
- **静默返回是陷阱**。`blit` / `blit_mask` / `draw_glyph` 在参数不合法时
  **不 panic、不绘制、直接返回**，一个不合格的基准会得到 7 ns 的
  「空操作」数字而毫无提示。写这类基准时必须核对：
  - `blit`：`src_pitch >= width × bpp` 且 `src.len() >= src_pitch × height`；
  - `blit_mask`：`mask_pitch >= width` 且 `mask.len() >= mask_pitch × height`；
  - `draw_glyph`：`mask.len() >= width × height`。
  本目录的基准在构造这些参数后都做了断言，`bench_swap_buffers` 还回读前台
  校验呈现确实发生。
- **不对覆盖范围作乐观假设**。例如点阵字体 `DefaultFont` 位于私有模块，
  crate 外无法引用，`font` 基准用一个等价的 `BitmapFont` 驱动
  `draw_text` 的排版路径。

## 分组命名

分组名是路径式的，便于用过滤器选中：

```
<被测对象>/<变体>/<规模>
```

例如 `fill_triangle/side64_alpha`、`clear/rgba8888/1920x1080`、
`blit_mask/Rgba8888_side32_antialiased`。多数分组还会用
`BenchmarkId::from_parameter` 让 criterion 的图表按规模自动连线。

## 已知的诚实边界

- 基准只测**本机**（当前为 Xeon E5-2673 v3 @2.40GHz），绝对值不外推。
  `simd::memcpy` 的分支选择是**编译期** `#[cfg(target_feature)]`：
  默认 `x86_64` 目标只启用 SSE2，`-C target-cpu=native` 会走 AVX2，
  因此 `blit` 类基准的绝对值依赖编译方式，只有同编译方式下的相对比较有效。
- 每秒迭代次数受 `Throughput` 标注影响，标错会误导读数；本目录的标注
  都按「每次迭代实际处理的像素/元素数」计算。
- 基准不替代正确性测试。`cargo test` 才是语义的守卫（如半开扫描线规则、
  短线段去重、`PenX` 不漂移）；基准只回答「多快」。
