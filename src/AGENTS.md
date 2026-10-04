# src/ 库本体

`no_std` 实现，唯一有业务逻辑的目录。无子目录，全部平铺在 `src/` 下。

## 文件职责

| 文件 | 行数 | 职责 | 扩展点 |
|------|------|------|--------|
| `lib.rs` | 94 | `#![no_std]`、feature 门控、文档首页 | 无 |
| `core.rs` | 2077 | `Renderer` 与固定功能图元 | 新增图元 |
| `programmable.rs` | 2145 | 2D 可编程管线 | 实现 `VertexShader` / `FragmentShader` / `Interpolate` |
| `color.rs` | 977 | `Color`、`PixelFormat`、混合、SWAR 内核 | 新增像素格式 |
| `simd.rs` | 211 | 分派 memcpy | 扩指令阶梯 |
| `utils.rs` | 175 | 裁剪、饱和转换、模式填充 | 无（私有） |
| `framebuffer.rs` | 80 | 布局描述与不变量断言 | 无 |
| `pixel.rs` | 49 | 坐标类型与宏 | 无 |
| `main.rs` | 39 | 演示二进制，非库的一部分 | 无 |

## core.rs 分层

自上而下四段，调用方向单向向下，不允许反向回跳：

```
宏层      write_rgb{,_blend,_blend_ptr,_ptr}!        core.rs:12-88
   ↓ 消除 RGB/BGR 字节序分支
门面      struct Renderer + new/buffer/info          core.rs:113-273
   ↓ 缓存 fmt_* 四个格式属性
跨度层    fill_span_opaque / fill_span_alpha         core.rs:447-580
   ↓ 不透明 → 4 字节块直写 / pattern_fill；半透明 → swar 或逐像素
图元层    clear/line/triangle/rect/circle/blit       core.rs:667-1155
   ↑ 各自负责裁剪，再下沉到跨度层；热路径无边界检查
```

关键内部函数（非 `pub`，改动需连带 review 调用方）：

| 函数 | 位置 | 作用 |
|------|------|------|
| `draw_line_internal` | `core.rs:811` | 迭代区间收窄的 Bresenham。开头的注释承诺"收窄不改变像素覆盖"，该承诺正由两个失败的测试守护 |
| `interpolate_scan_x` | `core.rs:582` | Bresenham 递推的纯函数形式，便于单测独立验证 |
| `fill_span_opaque` | `core.rs:447` | 不透明跨度。用 `get_unchecked_mut` 取整行切片后按 4 字节块写，安全性完全依赖调用方已裁剪 |
| `fill_span_alpha` | `core.rs:491` | 半透明跨度。`bpp == 4 && row.align_offset(4) == 0` 才进 SWAR，否则逐像素。SWAR 路径独立成函数是为避免挤占不透明路径的代码布局 |
| `fill_visible_horizontal_span` | `core.rs:551` | 自动裁剪到可见区域，裁剪完才下沉到 `fill_span_*` |
| `build_pattern` | `core.rs:738` | 生成 16 字节填充模式；3bpp 无法整除 16，退回 3 字节单像素模式 |
| `clear_unchecked` | `core.rs:690` | `clear` 的无检查内核。`pitch == row_len` 时一次 `pattern_fill` 铺满整个 buffer；否则铺满首行后逐行 `memcpy`。`init_len = pattern_len.min(total_bytes)` 这个钳制不可去掉——1×1 的 4bpp 只有 4 字节 |

`swap_buffers`（`core.rs:1145`）实为 `front_buffer.copy_from_slice(&back_buffer)`——单向 present，不是指针交换，且不重置 `back_buffer`。依赖"交换后后台缓冲为空"的调用方会读到上一帧内容。

`draw_line_internal` 开头就说明了为何必须用 `sat_i64` 而非 `as i64`：`usize::MAX as i64 == -1` 会把端点折到画布另一侧，覆盖结果完全错误。

### 五条写入路径

写像素有五条并存的路径，各有适用前提。**选错会静默写错字节或错位**：

| 策略 | 用在哪 | 前提 |
|------|--------|------|
| `chunks_exact_mut(4)` / `pattern_fill` | `fill_span_opaque`（定义 `core.rs:447`）、`clear_unchecked`（`core.rs:690`） | 不透明整段覆盖。4bpp 按 4 字节块写；3bpp 退回倍增复制 |
| `color::swar::blend_span_alpha_4bpp` | `fill_span_alpha`（定义 `core.rs:491`，SWAR 调用 `518`） | **仅 4bpp 且行首 4 字节对齐** |
| `color::swar::blend_masked_row_4bpp` | `blit_mask`（定义 `core.rs:330`，SWAR 调用 `395`） | 同上，掩码 alpha 逐字节驱动 |
| 逐像素 `write_rgb_blend_ptr!` | 上述两条的回退分支 | 3bpp 或未对齐 |
| `PixelFormat::write_color` | 不作为热路径 | 标量参考实现，等价性基准 |

注意不透明填充**不走** SWAR——`fill_span_opaque` 只用 `chunks_exact_mut`，SWAR 只服务于半透明与掩码。

任何优化档的改动都必须跑 `color.rs` 里的穷举单测（`blend_*_matches_*_reference` 系列）。

## programmable.rs 管线

四阶段顺序固定，坐标空间每阶段都不同：

| 阶段 | 输入 → 输出 | 坐标空间 |
|------|-------------|----------|
| 顶点着色 | `VS::Input` → `ProgramVertex` | normalized `[0,1]²` |
| 图元裁剪 | 半平面裁剪 | normalized `[0,1]²`，保证顶点全部落在内部 |
| 视口变换 | `ndc_to_screen` | viewport 内浮点像素 |
| 光栅化 + 片元着色 | 重心权重插值 → `shade_fragment` → `BlendMode` 写入 | 屏幕像素 |

裁剪上界：`MAX_CLIP_VERTICES = 8`（`programmable.rs:45`），四个半平面各切一刀，3 顶点凸多边形最多长到 7。改裁剪逻辑时不得让中间顶点数组溢出这个常量。

插值契约（`programmable.rs:127`）：`Interpolate` 实现必须满足 `interpolate(a, a, a, ..) == a`，否则同一片元在不同顶点顺序下结果不同。`lerp` 由 `interpolate` 默认提供，自定义类型只实现一个方法即可。

`FragmentInput.position` 是**相对 viewport 原点**的 `[x+0.5, y+0.5]`，viewport 不在 `(0,0)` 时它不等于帧缓冲绝对坐标。着色器里要写绝对像素必须自己加 viewport 原点。

`draw_indexed` 用 `alloc::vec![None; n]` 做惰性着色缓存，索引越界返回 `None` 并静默跳过该图元。这是本模块链接 `alloc` 的唯一原因。

## color.rs

`PixelFormat` 六变体，每个都要同时维护三处语义：`red/green/blue/alpha_shift`（`const fn`）、`bytes_per_pixel`、以及 `core.rs:165-172` 的 `fmt_*` 缓存值。`write_color` / `read_color` 是字节序的唯一权威实现，任何"更快"的写入都要与它们对齐。

`pub(crate) mod swar`（`color.rs:432`）只服务 4bpp；放在这里是因为它要复用 `PixelFormat` 的 shift 与 alpha 语义。

`blend_channel`（`color.rs:207`）用 `(src*a + dst*ia + 255) >> 8` 近似 `/255`，误差 ≤ 1。SWAR 内核必须复现这个近似而非改用精确除法。

`mod swar` 内部还有逐项优化：`mask == 255` 时直接整像素写 `u32`，跳过读-改-写（`color.rs:613`）；否则读 `u32`、混合、再把第 4 字节按 `opaque_alpha` 拼回（`color.rs:615-619`）。改动这里要保住"完全不透明时不读原像素"这个短路。

## simd.rs

阶梯按 `#[cfg(target_feature)]` 静态分派，**不是**运行时检测：`avx512f` → `avx/avx2` → `sse2` → `neon` → `riscv64+v`。新增一级要同时加函数与 `memcpy` 里的 `cfg` 分支，并保留末尾 `#[cfg(not(any(...)))]` 的 `remainder = len` 兜底。`len <= 128` 恒走 `copy_nonoverlapping`（`simd.rs:150`），小拷贝不进向量路径。

## utils.rs

`clip_line` 的溢出防护是硬性要求：端点来自 `usize`，经 `sat_i64` 可能取到 `i64::MAX`，与 `t`（最大 `height-1`）相乘会溢出，所以中间运算走 `i128`（`intersect_axis`，`utils.rs:107`）。改动此函数必须保留 `i128`。

`floor_to_usize` 用 `as usize`（向零取整），对非负值等价于 floor；`programmable.rs` 里坐标转像素索引依赖此语义。

## 测试分布

单测写在文件内的 `#[cfg(test)] mod tests` 中，共 5 处：

| 位置 | 覆盖对象 |
|------|----------|
| `core.rs:1157` | `Renderer` 全部公开方法、`*_unchecked` 等价性、图元裁剪 |
| `color.rs:875` | `Color` 构造/解析/混合、`PixelFormat::write_color`/`read_color` |
| `color.rs:625` | **内嵌在 `mod swar` 内**，直接测 `blend_word` / `source_word` / `blend_span_alpha_4bpp` / `blend_masked_row_4bpp` |
| `programmable.rs:1535` | 裁剪、插值、viewport/scissor、纹理采样、着色器 |
| `framebuffer.rs:61` | `buffer_size` 派生与 `pitch` 下限断言 |

`utils.rs` / `simd.rs` / `pixel.rs` 无单测——它们的行为通过上述集成路径间接覆盖。`simd.rs` 尤其无测试，因为验证它需要真实向量指令集，只能靠 `cargo bench` 观察。

命名语义：

| 后缀 | 断言内容 |
|------|----------|
| `_matches_*_reference` | 优化路径 == 标量参考实现 |
| `_unchecked_matches_safe_path` | unchecked 版 == 安全版 |
| `_preserves_*` | 优化未改变原有像素覆盖/行为 |
| `_rejects_*` / `_does_nothing` | 非法输入被静默拒绝 |
| `#[should_panic(expected = "...")]` | 构造期不变量，断言 panic 文案 |

穷举型单测的写法值得照抄：`color.rs:625` 的 `SAMPLES: [u8; 11]` 覆盖 0/1/2/63/64/127/128/129/192/254/255——刻意包含**每个位宽的分界点**，因为混合公式的错误几乎全出现在进位边界上。4bpp 测试还把 4 个通道填成互不相同的字节，一次调用同时校验 4 条通道并能暴露"某槽被相邻槽污染"。

新增优化路径时，前两类单测是准入条件，不是可选项。

## unsafe 与 SAFETY 注释

`unsafe` 共 71 处，分布在 5 个文件：`core.rs` 36、`simd.rs` 19、`color.rs` 14、`utils.rs` 1、`lib.rs` 1（后者是 `#![allow(unsafe_op_in_unsafe_fn)]` 属性）。

两级注释是强约定：

- `pub unsafe fn` 上方写 `/// # Safety` + 「调用方必须保证 ……」，说明**前置条件**。全库共 8 个：`core.rs` 5 个（`set_pixel_unchecked`、`set_pixel_at_unchecked`、`get_pixel_unchecked`、`get_pixel_at_unchecked`、`draw_line_unchecked`）、`color::swar` 2 个（`pub(crate)`）、`simd::memcpy` 1 个。前 7 个均已写全。
- 块内写 `// SAFETY: <局部推理>`，说明**此处为何安全**。现有 11 处（`core.rs` 3、`color.rs` 8），都紧邻需要论证的裸指针访问，例：`color.rs:534`「调用方保证 row 指向 pixel_count * 4 字节且 4 字节对齐，故 idx * 4 + 4 <= pixel_count * 4，且每次 u32 访问都对齐」；`core.rs:460`「调用方 fill_visible_horizontal_span 保证 x_start..=x_end 已裁剪到 [0, width)，y 已裁剪到 [0, height)，故 start + row_len <= buffer.len()」。

`lib.rs:82` 已开 `unsafe_op_in_unsafe_fn`，因此 `unsafe fn` 体内每处裸指针操作仍需显式 `unsafe {}` 块。新增裸指针代码时，块内 `// SAFETY:` 是硬性要求。

**现存缺口**：`simd::memcpy`（`simd.rs:145`）是唯一的 `pub unsafe fn` 缺 `# Safety` 段；`utils.rs:167` 与 `simd.rs` 内的裸指针块也没有 `// SAFETY:`。补 unsafe 时顺手把这三处补齐。