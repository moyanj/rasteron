# src/ 库本体

`no_std` 实现，唯一有业务逻辑的目录。`programmable/` 是唯一子目录（管线阶段与顶点缓存），其余文件平铺在 `src/` 下。

## 文件职责

| 文件 | 行数 | 职责 | 扩展点 |
|------|------|------|--------|
| `lib.rs` | 129 | `#![no_std]`、feature 门控、文档首页 | 无 |
| `core.rs` | 3846 | `Renderer` 与固定功能图元 | 新增图元 |
| `angle.rs` | 630 | 定点 `Angle` + 楔形 `Wedge` | 改角度精度 / 三角实现 |
| `programmable/mod.rs` | 892 | 图元装配与光栅化、`draw_arrays` / `draw_indexed` | 新增图元类型 |
| `programmable/shader.rs` | 34 | 内建透传顶点着色器 `Passthrough2D` | 新增内建着色器 |
| `programmable/cache.rs` | 252 | 顶点缓存：借用调用方槽位的 `VertexCache` | 新增缓存策略 |
| `color.rs` | 1517 | `Color`、`PixelFormat`、混合、SWAR 内核 | 新增像素格式 |
| `error.rs` | 213 | 全库错误类型的唯一定义处 | 新增错误类别 |
| `simd.rs` | 211 | 分派 memcpy | 扩指令阶梯 |
| `utils.rs` | 458 | 裁剪、饱和转换、模式填充、视口变换 | 无（多为私有） |
| `framebuffer.rs` | 80 | 布局描述与不变量断言 | 无 |
| `pixel.rs` | 49 | 坐标类型与宏 | 无 |
| `main.rs` | 39 | 演示二进制，非库的一部分 | 无 |

## core.rs 分层

自上而下四段，调用方向单向向下，不允许反向回跳：

```
宏层      write_rgb{,_blend,_blend_ptr,_ptr}!        core.rs:15-88
   ↓ 消除 RGB/BGR 字节序分支
门面      struct Renderer + new/buffer/info          core.rs:125-285
   ↓ 缓存 fmt_* 五个格式属性
跨度层    fill_span_opaque / fill_span_alpha         core.rs:459-600
   ↓ 不透明 → 4 字节块直写 / pattern_fill；半透明 → swar 或逐像素
图元层    clear/line/triangle/rect/circle/arc/blit  core.rs:679-1320
   ↑ 各自负责裁剪，再下沉到跨度层；热路径无边界检查
```

关键内部函数（非 `pub`，改动需连带 review 调用方）：

| 函数 | 位置 | 作用 |
|------|------|------|
| `draw_line_internal` | `core.rs:831` | **唯一的 Bresenham 实现**。迭代区间收窄到 `[x_lo, x_hi]`（主轴可见范围），次轴在循环内校验上界。`draw_line` 与 `draw_line_unchecked` 都委托到这里 |
| `interpolate_scan_x` | `core.rs:594` | Bresenham 递推的纯函数形式，便于单测独立验证 |
| `disk_params` | `core.rs:1087` | 圆与圆弧共用的参数归一。**在此钳制 `radius`**（见下） |
| `disk_half_at` | `core.rs:1113` | 精确圆盘第 `k` 行的半宽 `floor(sqrt(r² − k²))`，增量收缩求取，无浮点 |
| `walk_disk_rows` | `core.rs:1132` | 逐行回调 `(cx, cy, k, half)`，**每行恰好一次**。`fill_circle`/`fill_arc` 的共享入口 |
| `walk_disk_boundary` | `core.rs:1161` | 精确圆盘边界逐像素回调 `(px, py, dx, dy)`，**每像素恰好一次**。`draw_circle`/`draw_arc` 的共享入口 |
| `fill_span_opaque` | `core.rs:459` | 不透明跨度。用 `get_unchecked_mut` 取整行切片后按 4 字节块（4bpp）或 2 字节块（565）写，安全性完全依赖调用方已裁剪 |
| `fill_span_alpha` | `core.rs:503` | 半透明跨度。`bpp == 4 && row.align_offset(4) == 0` 才进 SWAR（判定在 `522`，调用在 `530`），否则逐像素。SWAR 路径独立成函数是为避免挤占不透明路径的代码布局 |
| `fill_visible_horizontal_span` | `core.rs:563` | 自动裁剪到可见区域，裁剪完才下沉到 `fill_span_*` |
| `build_pattern` | `core.rs:750` | 生成 16 字节填充模式；3bpp 无法整除 16，退回 3 字节单像素模式 |
| `clear_unchecked` | `core.rs:702` | `clear` 的无检查内核。`pitch == row_len` 时一次 `pattern_fill` 铺满整个 buffer；否则铺满首行后逐行 `memcpy`。`init_len = pattern_len.min(total_bytes)` 这个钳制不可去掉——1×1 的 4bpp 只有 4 字节 |

`swap_buffers`（`core.rs:1175`）实为 `front_buffer.copy_from_slice(&back_buffer)`——单向 present，不是指针交换，且不重置 `back_buffer`。依赖"交换后后台缓冲为空"的调用方会读到上一帧内容。

`draw_line_internal` 开头就说明了为何必须用 `sat_i64` 而非 `as i64`：`usize::MAX as i64 == -1` 会把端点折到画布另一侧，覆盖结果完全错误。

### 圆半径的钳制

`disk_params`（`core.rs:1087`）把 `radius` 钳到 `width + height`。上界的正确性来自：任意可见像素到圆心的距离不超过画布对角线，而 `sqrt(w² + h²) < w + h`。因此半径超过该上界时，屏幕外不画（由各图元自己的边界检查丢弃），可见像素的着色结果不变，而迭代量从 `O(radius)` 降到 `O(height)`——`walk_disk_rows` 的 `k` 上限是 `min(r, height)`，比旧中点圆递推的 `O(width + height)` 还紧一档。

`FramebufferInfo::new` 只校验 `pitch`，不限制 `width`/`height`，故加法用 `saturating_add`；另有局部常量 `MAX_RADIUS = i64::MAX / 2` 兜底，它保证 `r²` 在 `u128` 内可表示。半径转 `i64` 必须走 `sat_i64`——`as i64` 会把 `usize::MAX` 回绕成 −1，一行都不画。

**改此处必须让两条测试同时通过，它们各守一个方向：**

| 测试 | 守护 |
|------|------|
| `circle_clamps_huge_radius` | 钳制**缺失**。半径表把 `1_000_000_000` 排在最前：无钳制时迭代量随半径爆炸，测试直接挂死 |
| `circle_clamp_preserves_radii_within_limit` | 钳制**过度**。锚定半径 1/2/3 的精确圆盘像素数 5/11/16（4×4 屏、圆心 `(1,2)`），并比对全屏期望字节 |

两条都经过变异验证：拆掉 `.min(limit)` 会挂起，改成 `.min(limit).min(2)` 会让第二条报「半径 3 的填充像素数被钳制改变」。

注意 4×4 屏上半径 ≥ 3 即覆盖整屏，所以"钳到 3 以上"的过度收紧在该场景下不可观察——锚点只需覆盖半径 1 与 2。

### 六条写入路径

写像素有六条并存的路径，各有适用前提。**选错会静默写错字节或错位**：

| 策略 | 用在哪 | 前提 |
|------|--------|------|
| `chunks_exact_mut(4)` / `pattern_fill` | `fill_span_opaque`（定义 `core.rs:459`）、`clear_unchecked`（定义 `core.rs:702`） | 不透明整段覆盖。4bpp 按 4 字节块写；3bpp 退回倍增复制 |
| `chunks_exact_mut(2)` | `fill_span_opaque` 的 2bpp 分支；`clear_unchecked` 经 `build_pattern` 的 16 字节模式 | 565 不透明整段覆盖（量化打包后 2 字节 pattern） |
| `color::swar::blend_span_alpha_4bpp` | `fill_span_alpha`（定义 `core.rs:503`，SWAR 调用 `530`） | **仅 4bpp 且行首 4 字节对齐** |
| `color::swar::blend_masked_row_4bpp` | `blit_mask`（定义 `core.rs:342`，SWAR 调用 `407`） | 同上，掩码 alpha 逐字节驱动 |
| 逐像素 `write_rgb_blend_ptr!` | 上述两条的回退分支 | 3bpp 或未对齐 |
| 逐像素 `color::blend_565` + `core::{load_565, store_565}` | `write_color_at_offset` / `blit_mask` / `fill_span_alpha` / `draw_line_internal` 的 `bpp == 2` 分支 | **仅 565**：逐字节读写，不假设 `u16` 对齐；565 不走 SWAR |
| `PixelFormat::write_color` | 不作为热路径 | 标量参考实现，等价性基准 |

注意不透明填充**不走** SWAR——`fill_span_opaque` 只用 `chunks_exact_mut`，SWAR 只服务于半透明与掩码。

任何优化档的改动都必须跑 `color.rs` 里的穷举单测（`blend_*_matches_*_reference` 系列）；
565 另有一组（`rgb565_*`）：位域布局、量化往返、逐通道混合参考、字节交换，
以及 `core.rs` 的 `rgb565_*` 图元等价性测试（含奇数 `pitch`）。

## angle.rs 定点角度

`Angle` 是圈值（整圈 `2^32` 圈值）的 `u32` 新类型，加减用 `wrapping_*` 自动环绕，
不做任何取模分支——`>> 30` 直接得象限号，三角函数的四象限折叠由此而来。

选「圈值」而非「度 × 系数」是为了让加减天然环绕；选 `u32` 而非 `u16` 是因为一圈
`2^32` 个刻度（约 8.4e-8 度）远细于像素级判定所需精度。

`SIN_TABLE`（257 项 `i16`）是脚本生成的字面量，**不是 `const fn` 算出来的**——
MSRV 1.79 的 const-eval 不支持浮点。改精度前先确认表仍由脚本生成，不要手改数值。
实测最大误差 1.54 LSB（`SIN_SCALE = 32767`），折算成半径 `r` 处约 `0.0001 × r` 像素；
`sin(0°/90°/180°/270°)` 为精确值。

`from_degrees` 必须**先对 360 取模再乘**。不取模时 `deg = 360` 会算出 `2^32`，
`as u32` 回绕成 0 而 `deg = 361` 变成 104——正是 doctest 断言 `from_degrees(720) == ZERO`
要防的情形。同理 `from_millidegrees` 对 360_000 取模。

`Wedge` 在构造时预算 start/end/mid 三条射线，使 `contains` 每次判定只做 2 次叉积，
无除法、无三角运算。判定分两种路径：

| 跨度 | 写法 | 守护 |
|------|------|------|
| `<= 180°` | `cross(start,p) >= 0 && cross(p,end) >= 0` | — |
| `> 180°` | 拆成 `start..mid` 与 `mid..end` 两个小楔形**取并集** | `wedge_major_arc_includes_start_ray` |

大跨度**不能取补集**：恰好落在起点射线上的点位于缺口楔形边界，取补会被误排除。
这正是该测试存在的理由。

叉积带与操作数同阶的容差（`|a|_1 × |p|_inf / 2^15`），用于吸收定点三角的舍入抖动——
无容差时轴向像素 `(1,0)` 会被随机判到界外。`wedge_tolerance_does_not_widen_arc`
守护容差没有取得过宽（边界外 1° 的方向不得被误收）。

圆心（零向量）在 `Wedge::contains` 中恒判为**在区间内**——它是任何扇形的一部分。
`fill_arc` 与 `draw_arc` 都靠这一条保证圆心像素不被漏掉。

## programmable/ 管线

四阶段顺序固定，坐标空间每阶段都不同：

| 阶段 | 输入 → 输出 | 坐标空间 |
|------|-------------|----------|
| 顶点着色 | `VS::Input` → `FragmentInput`（即 `Vertex2D`） | normalized `[0,1]²` |
| 视口变换 | [`utils::normalized_to_screen`] | viewport 内浮点像素 |
| 图元裁剪 | 点二值判定 / 线段参数域裁剪 / 三角形 bbox 求交 | 屏幕像素（`Bounds`） |
| 光栅化 + 片元着色 | 重心权重插值 → `shade_fragment` → `BlendMode` 写入 | 光栅化按屏幕像素定位；回传给 `shade_fragment` 的 `FragmentInput.position` 仍是 normalized `[0,1]²` |

**裁剪统一收敛到 `Bounds`**（`programmable/mod.rs:286`）：帧缓冲 ∩ viewport ∩ scissor 由 `visible_bounds`（`mod.rs:453`）在装配前算一次，为空则整次绘制调用直接成功返回，连顶点着色都不执行。三种图元各用各的裁剪手段，**没有多边形半平面裁剪**：

| 图元 | 手段 | 位置 |
|------|------|------|
| 点 | `Bounds::contains_normalized` 二值判定。必须在浮点域做——先 `as usize` 会让负坐标饱和成 `0` 而误通过下界检查 | `rasterize_point`（`mod.rs:512`） |
| 线段 | `Bounds::clip_segment_t` 参数域裁剪（Liang–Barsky），只对可见子线段步进 | `rasterize_line`（`mod.rs:541`） |
| 三角形 | bbox 与 `bound` 求交后逐像素边函数判定，不做几何裁剪 | `rasterize_triangle`（`mod.rs:605`） |

迭代量因此只取决于 `bound` 尺寸，与端点/顶点坐标解耦（线段 `steps` 的上界另有 `debug_assert!` 兜底）。三角形的有符号面积取 `edge(v1, v2, v0)` **单项**——三项相加会把面积放大 3 倍，使权重和退化成 `1/3`，插值结果被整体缩小。

插值契约（`programmable/mod.rs:34`）：`Interpolate` 实现必须满足 `interpolate(a, a, a, ..) == a`，否则同一片元在不同顶点顺序下结果不同。`lerp` 由 `interpolate` 默认提供，自定义类型只实现一个方法即可。

`FragmentInput.position` 是 **normalized `[0,1]` viewport 坐标**，与顶点着色器输出的 `position` 同处一个坐标系（`[0,0]` 为 viewport 左上角，Y 轴向下），**不是**帧缓冲绝对像素坐标。光栅化内部按屏幕像素定位，但交给 `shade_fragment` 的 `position` 仍是归一化值。着色器里要写绝对像素必须自己调 `utils::normalized_to_screen`。

注意归一化距离只在**正方形 viewport** 下各向同性；viewport 非正方形时 x/y 缩放比不同，着色器里做径向判定会退化成椭圆。

**混合落点**：`Renderer::write_fragment_color`（`programmable/mod.rs:477`）是唯一的混合分派点，公式下沉在 `color.rs`。`BlendMode::None` 与 `Alpha` **直接委托** `Renderer::set_pixel_unchecked`（字节层权威，`core.rs`）——`None` 先调 `mix_alpha(255)` 绕开 `a == 0` 短路，否则 `None` 会退化成空操作；`Add` / `Multiply` / `PremultipliedAlpha` 先读目标像素，再走 `color.rs` 的纯函数 `BlendMode::blend` 落盘。管线因此与固定功能共用同一条写入路径，**565 在管线上自动可用**（`core.rs` 的 `set_pixel_unchecked` 已含 `bpp == 2` 分支）。

**取整由各图元自负**：`write_fragment_color` 只收整数像素坐标。点用向零取整（`floor_to_usize`），线段用四舍五入（`floor_to_usize(v + 0.5)`），三角形在 bbox 内整数扫描——三者不可互换，故取整不能收进写入层，否则线段覆盖会被静默改变。

`draw_indexed` 的第 5 参是 `Option<&mut VertexCache<'_, VS::Varying>>`。`None` = 每次重新着色；`Some(cache)` = 按顶点索引惰性复用，`cache.len()` 短于顶点数时退化为部分缓存（高索引每次重新着色），越界不 panic。缓存自身不分配：`VertexCache`（`programmable/cache.rs`）借用调用方槽位，`no_std` 下可用。`VertexShader::shade_vertex` 必须是纯函数，否则命中与未命中的结果会不一致。

**索引校验先于任何绘制**：越界返回 `DrawError::IndexOutOfBounds`，且整次调用不产生任何片元——半途报错会在帧缓冲里留下「画了一半」的残留。尾部不足一个完整图元的索引（`Lines` 余 1、`Triangles` 余 1/2）静默丢弃。

`draw_arrays` 与 `draw_indexed` 共用私有的 `assemble_primitives`：闭包 `resolve(slot) -> FragmentInput` 把**装配位置**映射到顶点着色结果（前者传顶点下标，后者传索引缓冲查表），装配与光栅化逻辑因此只有一份，两条 API 的像素覆盖逐位一致。**该等价性当前没有测试守护**——旧 `programmable.rs` 的测试在重写时被整体删除，待补（见「测试分布」）。

**不要用「线段端点反序应当翻转渐变」验证索引是否生效**。`Interpolate::lerp(a, b, t)` 逐通道恒等于 `lerp(b, a, 1 - t)`，而端点反序后落在同一像素的步进参数恰为 `1 - t`，两者相乘抵消——**任何两顶点线段反转索引后逐像素完全相同**，这是数学恒等式而非实现缺陷，该断言恒不成立。验证索引生效要用「换顶点子集」的顶点缓冲，或用三角形换索引（三角形反转顺序同样不可观测：重心权重跟随各自顶点）。

## color.rs

`PixelFormat` 八变体，每个都要同时维护三处语义：`red/green/blue/alpha_shift`（`const fn`）、
`bytes_per_pixel`、以及 `core.rs` `build` 里的 `fmt_*` 缓存值（含 `fmt_bytes_swapped`）。
`write_color` / `read_color` 是字节序的唯一权威实现，任何"更快"的写入都要与它们对齐。

shift 的语义是「**位**位移」：对 6 种 8 位通道格式恰好等于字节位移 × 8（数值与旧版一致），
对 565 则是 11/5/0。因此 `to_bytes` / `from_bytes` / `write_color` / `read_color` 都用
`bytes_per_pixel() == 2` 早返回进入 565 分支，8 位路径一字未动。

### 16 位打包格式（`Rgb565` / `Bgr565`）

位域固定为 `R = 15..11`、`G = 10..5`、`B = 4..0`；两变体位域相同，仅存储字节序不同
（`Rgb565` 小端，`Bgr565` 大端，即面板厂商常称的 RGB565_BE——**与 DRM 的
`DRM_FORMAT_BGR565`（B 在高位）不是一回事**，rustdoc 已显式标注）。

五个 `pub(crate)` 原语是全部 565 逻辑的单一来源：

| 原语 | 作用 |
|------|------|
| `quantize_channel(v, bits)` | 8 位 → 5/6 位，四舍五入 `(v × max + 127) / 255`；`bits == 8` 时恒等 |
| `expand_channel(v, bits)` | 位复制扩展回 8 位：`(v << k) \| (v >> (bits − k))`，`k = 8 − bits` |
| `pack_565` / `unpack_565` | 颜色 ↔ 565 字（不含字节序） |
| `blend_565(word, color)` | 唯一的 565 混合语义：解码 → `blend_channel` → 量化；`a == 255` 直接 `pack_565`（不读目标），`a == 0` 原样返回 |
| `decode_565_bytes` / `encode_565_bytes` | 字节序的唯一权威（`from_le_bytes` / `from_be_bytes` 分支） |

量化与扩展互逆：`quantize_channel(expand_channel(v, bits), bits) == v` 对全部 5/6 位取值成立
（`rgb565_channel_quantization_is_roundtrip_stable` 守护），因此反复读写同一像素不会漂移。
量化是四舍五入而非截断，`quantize5(255) = 31`、`quantize6(255) = 63` 无溢出。

`core.rs` 侧另有 `load_565` / `store_565` 两个 `unsafe fn`：**逐字节**读写，不假设 `u16` 对齐
（`pitch` 允许为奇数，行首可能落在奇地址）。565 没有 SWAR 路径。

`pub(crate) mod swar`（`color.rs:682`）只服务 4bpp；放在这里是因为它要复用 `PixelFormat` 的 shift 与 alpha 语义。

`blend_channel`（`color.rs:304`）用 `(src*a + dst*ia + 255) >> 8` 近似 `/255`，误差 ≤ 1。SWAR 内核必须复现这个近似而非改用精确除法；565 的 `blend_565` 同样复用它。

`Color::blend` 逐通道委托 `blend_channel`，是**全库 Alpha 混合的唯一权威**——SWAR 内核、固定功能跨度填充与可编程管线都收敛到它，因此同一次混合在任意路径上逐位一致。它曾用精确 `/255`，与 `blend_channel` 在 48.67% 的 `(src, dst, alpha)` 组合上差 1；改回精确除法即会让可编程管线与字节层不再逐位一致。**该一致性当前没有测试守护**（旧 `programmable.rs` 的测试在重写时被整体删除，待补）。

`Color::blend_add` / `blend_multiply` / `blend_premultiplied` 是 `pub(crate)`，只服务可编程管线的对应 `BlendMode`。三者都带 `#[cfg(feature = "programmable")]`——漏掉该 `cfg` 会让 `--no-default-features` 报 `dead_code`。

`mod swar` 内部的关键优化在 `blend_lane_pairs`（`color.rs:727`）：把 `c0`/`c2` 留在原位、`c1`/`c3` 右移 8 位，使一个 32 位字同时容纳两个通道的 16 位中间值；`d*255` 展开成 `(d << 8) - d` 省掉一次乘法。掩码路径 `blend_masked_segment`（`color.rs:941`）用无分支构造的 `keep_high`（`color.rs:959`）保留原第 4 字节，`m == 0` 时结果退回原字。改动这里必须与 `blend_channel` 逐字节等价（`blend_word_matches_blend_channel_over_all_*` 两条穷举测试守护）。

## simd.rs

阶梯按 `#[cfg(target_feature)]` 静态分派，**不是**运行时检测：`avx512f` → `avx/avx2` → `sse2` → `neon` → `riscv64+v`。新增一级要同时加函数与 `memcpy` 里的 `cfg` 分支，并保留末尾 `#[cfg(not(any(...)))]` 的 `remainder = len` 兜底。`len <= 128` 恒走 `copy_nonoverlapping`（`simd.rs:150`），小拷贝不进向量路径。

## utils.rs

`clip_line` 的溢出防护是硬性要求：端点来自 `usize`，经 `sat_i64` 可能取到 `i64::MAX`，与 `t`（最大 `height-1`）相乘会溢出，所以中间运算走 `i128`（`intersect_axis`，`utils.rs:295`）。改动此函数必须保留 `i128`。

`floor_to_usize` 用 `as usize`（向零取整），对非负值等价于 floor；`programmable/mod.rs` 里坐标转像素索引依赖此语义。它带 `#[cfg(feature = "programmable")]`——漏掉该 `cfg` 会让 `--no-default-features` 构建报 `dead_code`。

## 测试分布

单测写在文件内的 `#[cfg(test)]` 模块中，共 124 个 `#[test]`，分布在 7 个文件：

| 位置 | 数量 | 覆盖对象 |
|------|------|----------|
| `core.rs:1637` | 72 | `Renderer` 全部公开方法、`*_unchecked` 等价性、图元裁剪、迭代量上界、圆半径钳制双向、精确圆盘/边界参考等价性、圆弧/扇形 |
| `angle.rs:402` | 17 | `Angle` 定点三角（基准角精确、单位恒等式、往返）、`Wedge` 楔形判定（含大跨度与容差） |
| `color.rs:1215` | 17 | `Color` 构造/解析/混合、`PixelFormat::write_color`/`read_color`、565 位域与量化往返 |
| `color.rs:966` | 5 | **内嵌在 `mod swar` 内**，直接测 `blend_word` / `source_word` / `blend_span_alpha_4bpp` / `blend_masked_row_4bpp` |
| `programmable/cache.rs:123` | 7 | 缓存命中/回填/清空、越界索引不 panic、短缓存降级为部分命中 |
| `error.rs:157` | 2 | 错误展示文本稳定、`DrawError` 字段 |
| `framebuffer.rs:61` | 2 | `buffer_size` 派生与 `pitch` 下限断言 |
| `utils.rs:413` | 2 | `sub_view` / `sub_view_with_back_buffer` 的收缩与后台缓冲区校验 |

`core.rs:121` 另有一个 `#[cfg(test)]` 的 `CircleKind`，供参考实现对照使用，不是独立测试模块。

**可编程管线（`programmable/mod.rs`）当前没有单测**：旧 `programmable.rs` 的 18 个测试（图元装配、索引校验、缓存跨调用复用、纹理采样、线段/三角形裁剪、565 落盘、`BlendMode::None` 覆盖语义等）在重写时被整体删除，尚未补回。改动管线前必须手工验证这几条语义。

该模块需要 `extern crate alloc;` 与 `extern crate std;`：`#![no_std]` 下两者都不在 extern prelude，即使只在 `#[cfg(test)]` 里也必须显式引入。

`simd.rs` 与 `pixel.rs` 无单测——它们的行为通过上述集成路径间接覆盖。`simd.rs` 尤其无测试，因为验证它需要真实向量指令集，只能靠 `cargo bench` 观察。

`programmable/shader.rs` 没有单测；它只有一个 `Passthrough2D` 透传实现，目前也没有集成测试覆盖它。

命名语义：

| 后缀 | 断言内容 |
|------|----------|
| `_matches_*_reference` | 优化路径 == 标量参考实现 |
| `_unchecked_matches_safe_path` | unchecked 版 == 安全版 |
| `_preserves_*` | 优化未改变原有像素覆盖/行为 |
| `_rejects_*` / `_does_nothing` | 非法输入被静默拒绝 |
| `#[should_panic(expected = "...")]` | 构造期不变量，断言 panic 文案 |

穷举型单测的写法值得照抄：`color.rs:985` 的 `SAMPLES: [u8; 11]` 覆盖 0/1/2/63/64/127/128/129/192/254/255——刻意包含**每个位宽的分界点**，因为混合公式的错误几乎全出现在进位边界上。4bpp 测试还把 4 个通道填成互不相同的字节，一次调用同时校验 4 条通道并能暴露"某槽被相邻槽污染"。

新增优化路径时，前两类单测是准入条件，不是可选项。

**耗时类断言不写时间数**。`no_std` 测试里没有 `Instant`，而旧实现（按 `usize::MAX` 迭代）在 `timeout` 下也跑不完——让测试挂起本身就是失败信号。断言输出等价即可，量级由 `cargo bench` 把关。

## unsafe 与 SAFETY 注释

`unsafe` 共 90 处，分布在 6 个文件：`core.rs` 47、`simd.rs` 19、`color.rs` 18、`programmable/mod.rs` 4、`utils.rs` 1、`lib.rs` 1（后者是 `#![allow(unsafe_op_in_unsafe_fn)]` 属性）。

两级注释是强约定：

- `unsafe fn` 上方写 `/// # Safety`，说明**前置条件**。现有 12 处：`core.rs` 9 个（5 个 `pub`——`set_pixel_unchecked`、`set_pixel_at_unchecked`、`get_pixel_unchecked`、`get_pixel_at_unchecked`、`draw_line_unchecked`，加 `pub(crate)` 的 `fill_span_alpha`、私有的 `clear_unchecked`，再加 565 的 `load_565` / `store_565`；其中 `fill_span_alpha` 并非 `unsafe fn`，但同样声明前置条件）、`color::swar` 3 个 `pub(crate)`（`blend_span_alpha_4bpp` / `blend_masked_row_4bpp` / `blend_masked_segment`）。**唯一缺失是 `simd::memcpy`（`simd.rs:145`）**。
- 块内写 `// SAFETY: <局部推理>`，说明**此处为何安全**。现有 25 处：`core.rs` 8 处、`color.rs` 13 处、`programmable/mod.rs` 4 处（`write_fragment_color` 的 `BlendMode` 四条分支），都紧邻需要论证的裸指针访问（565 新增的 5 处在 `load_565` / `store_565` / `blit_mask` / `fill_span_alpha` / `draw_line_internal`）。例：`color.rs` 的「调用方保证 row 指向 pixel_count * 4 字节且 4 字节对齐，故 idx * 4 + 4 <= pixel_count * 4，且每次 u32 访问都对齐」；`core.rs` 的「调用方 fill_visible_horizontal_span 保证 x_start..=x_end 已裁剪到 [0, width)，y 已裁剪到 [0, height)，故 start + row_len <= buffer.len()」。

`lib.rs:82` 已开 `unsafe_op_in_unsafe_fn`，因此 `unsafe fn` 体内每处裸指针操作仍需显式 `unsafe {}` 块。新增裸指针代码时，块内 `// SAFETY:` 是硬性要求。

`draw_line_unchecked`（`core.rs:948`）是**无附加前置条件**的特例：实现已收窄迭代区间并逐轴校验坐标。它的 `# Safety` 段只声明"帧缓冲切片仍有效"这一与 `draw_line` 相同的前提，不再声称需要调用方提供坐标保证。写这类"保留 `unsafe` 但实际无要求"的函数时，`# Safety` 段必须与正文描述一致，不要一边说"不需要保证"一边列一堆前置条件。

**现存缺口**（`cargo clippy --all-targets` 的警告全部属于既存问题，合并未新增任何一条）。lib 3 条：`simd.rs:145` 缺 `# Safety` 段；`core.rs:338` 的 `usize as usize` 冗余转换；`programmable/shader.rs:32` 的 `(*input).into()` 是自转换（`Vertex2D<V>` 就是 `FragmentInput<V>`）。lib test 另有 3 条 `u8 as u8` 冗余转换（`core.rs:2101`/`2203`/`2722`）。examples 只有 2 个问题位置——`shared/bmp.rs:113` 无效位运算、`:157` 手写 `is_multiple_of`——但因该文件被 bin 与 4 个 example 共用，各报多次。
