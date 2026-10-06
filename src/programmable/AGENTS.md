# src/programmable — 2D 可编程管线

**生成时间:** 2026-10-06 · 4 文件 5906 行
**feature 门控:** 整个目录由 `#[cfg(feature = "programmable")]` 包裹（`lib.rs:142`），默认开启

这是全库**唯一**引入 `f32` 的地方。关闭该 feature 后几何计算回到全整数
（父级 `angle.rs` 的圈值路径），这条契约由 `cargo check --no-default-features` 守护。

## 四个文件

| 文件 | 行数 | 内容 |
|------|------|------|
| `mod.rs` | 3507 | 类型体系、`Pipeline`、3 个 `rasterize_*` 内核、2 个公开绘制入口（55 个测试） |
| `cache.rs` | 387 | `VertexCache<'a, V>`——按顶点索引复用着色结果的 post-transform cache（11 个测试） |
| `texture.rs` | 1266 | `Texture2D` + `WrapMode` / `FilterMode` + 寻址 / 采样（39 个测试） |
| `shader.rs` | 746 | `Passthrough2D<V>`（顶点）+ `SolidFs` / `GradientFs` / `TextureFs`（片元）（20 个测试） |

## 数据流

```
vertices ──VS::shade_vertex──▶ FragmentInput ──缓存按 index 复用──┐
                                        │                        │
                                   光栅化（3 种图元）              │
                                        ▼                        │
                        重心坐标 ──Interpolate::interpolate──▶ varying
                                        ▼
                            FS::shade_fragment ──▶ FragmentOutput
                                        ▼
                          write_fragment_color(blend) ──▶ 帧缓冲
```

## 类型体系

| 类型 | 位置 | 说明 |
|------|------|------|
| `Vec2` / `Vec3` / `Vec4` | `:8`–`:10` | `[f32; N]` 别名，非 struct |
| `PrimitiveType` | `:13` | `Points` / `Lines` / `Triangles`；`to_step()` 给每图元的顶点数 |
| `Interpolate` | `:34` | varying 插值 trait。**契约：`interpolate(a,a,a,..) == a`**，否则同一片元在不同顶点顺序下结果不同 |
| `Flat<T>` | `:121` | 平坦插值包装器（不做插值） |
| `FragmentInput<V>` | `:155` | `position` + `varying`，同时充当顶点着色器输入与片元着色器输入 |
| `Vertex2D<V>` | `:160` | `FragmentInput<V>` 的别名——顶点就是「只有位置与 varying 的片元输入」 |
| `FragmentOutput` | `:167` | 当前只有 `Color` 变体；`draw_*` 处用 `if let` 匹配，非 `Color` 即丢弃 |
| `VertexShader` / `FragmentShader` | `:179` / `:188` | `shade_vertex(&self, &Self::Input)` / `shade_fragment(&self, FragmentInput) -> FragmentOutput` |
| `BlendMode` | `:198` | `None` / `Alpha` / `Add` / `Multiply` / `PremultipliedAlpha` |
| `Rect` → `Viewport` / `ScissorRect` | `:254`–`:283` | 三个类型别名同一结构，`PipelineState` 里三选二 |
| `Bounds` | `:286` | 半开区间可见矩形，含 `contains_normalized`（**必须在浮点域判定**）与 `clip_segment_t`（参数域裁剪） |
| `PipelineState` | `:391` | `{ blend, scissor, viewport }`；`alpha_blend(viewport)` 是便捷构造 |
| `Pipeline<VS, FS>` | `:416` | 三字段打包，**无全局状态**，可被多个 `Renderer` 复用 |

## 私有内核

| 内核 | 位置 | 职责 |
|------|------|------|
| `visible_bounds` | `:504` | 帧缓冲 ∩ viewport ∩ scissor，三者交集为空则整批图元直接成功返回（连顶点着色都不执行） |
| `write_fragment_color` | `:528` | 按 `BlendMode` 落盘。**取整不在这里做**——各图元落位规则不同（点向零取整、线四舍五入） |
| `rasterize_point` | `:563` | 二值可见判定，无参数域裁剪 |
| `rasterize_line` | `:612` | 参数域裁剪 + 插值 + **按像素去重片元**（见下） |
| `rasterize_triangle` | `:703` | 重心坐标插值，`w0/w1/w2` 除以 `area` |

公开入口只有两个：`draw_arrays`（`:829`）与 `draw_indexed`（`:919`），
都返回 `Result<(), DrawError>`——索引越界是**调用方程序错误**，
fail-fast 上抛而非静默跳过整条图元。

## 陷阱

- **线段 DDA 的采样点≠像素，必须去重**（`9e56098` 修复）。`rasterize_line`
  按 `steps = ceil(max|dx|, |dy|)` 采样，短于一个像素的线段会有多个 `t`
  映射到同一整数像素。`Alpha` / `Add` / `Multiply` / `PremultipliedAlpha`
  都是读-改-写，重复着色就得到两倍效果——半透明短线段比同位置的点更亮，
  且亮度取决于线段方向。
  修法是循环里记住 `previous: Option<(usize, usize)>`，取整并通过
  `bound.contains` 后与之比较、相同则跳过，**只有真正写入后才更新**。
  DDA 主采样轴单调递增，重复坐标必然连续出现，故单个变量足够、无需分配。
  保留的是**第一次**映射到该像素的 `t`。
  改这段时注意：`previous` 必须在 `continue`（`bound` 判定之外）之后才更新，
  漏更新会让整段线段只画一个像素。
  守护测试：`short_line_shades_each_pixel_once`、
  `short_line_alpha_matches_a_point_at_the_same_pixel`、
  `short_lines_of_different_directions_have_equal_brightness`、
  `every_blend_mode_writes_a_short_line_exactly_once`、
  `line_coverage_matches_independent_reference_model`。
- **坐标系不是 OpenGL**：顶点着色器输出的 `position` 是 normalized
  `[0, 1]` viewport 坐标，`[0, 0]` 为左上角，**不是** `[-1, 1]` clip space。
- **视口变换是「边缘空间」，跨度乘全宽**：`normalized_to_screen` 算的是
  `x + position * width`（**不是** `width - 1`），与 OpenGL `glViewport` /
  Vulkan `VkViewport` / D3D11 `D3D11_VIEWPORT` 同构——clip space 的两端
  对应 `left` 与 `left + width` 这条**边界线**。因此 `position == [1.0, 1.0]`
  落在 `max_x` / `max_y` 上、位于半开区间 `[x, x + width)` **之外**：
  点图元会被 `contains_normalized` 判为不可见，线段与三角形则由
  `clip_segment_t` 与 bbox 收敛，末像素仍在绘制范围内。
  「像素中心 + 半像素偏移」（`gl_FragCoord`）属于栅格化后的采样阶段，
  不在视口变换里——别把它和 `width - 1` 口径混为一谈。
- **点判定必须先在浮点域做**。`rasterize_point` 的注释说明了原因：
  先 `as usize` 会让负坐标饱和成 `0`，从而误通过下界检查。
- **scissor 裁在着色器之前**。语义是「超出部分既不写着色器也不写帧缓冲」，
  不能交由写入层兜底。
- **三角形的 `area` 是三倍缩放**。`8586ef3` 修过这个 bug
  （三角形面积三倍缩放导致 varying 插值失真）——改插值权重时注意别退回
  归一化面积混用。
- **`Interpolate::interpolate` 的恒等性契约**（`interpolate(a,a,a,..) == a`）
  是缓存正确性的前提，`cache.rs` 的命中值必须与重新着色逐位相同。

## 缓存契约（`cache.rs`）

`VertexCache<'a, V>` 借用调用方的 `&'a mut [Option<FragmentInput<V>>]` 槽位，
**自身零分配**。降级语义是刻意设计，不是缺陷：

- `lookup` 越界返回 `None`，`store` 越界静默丢弃 → 缓存短于顶点缓冲是合法的
  （低索引命中、高索引每次重新着色）
- 顶点数据变化后调用方必须 `clear()`，库不会自动失效
- 契约依赖 `VertexShader::shade_vertex` 是纯函数

## 便捷着色器（`shader.rs`）

| 类型 | 种类 | 参数载体 | 说明 |
|------|------|---------|------|
| `Passthrough2D<V>` | 顶点 | `PhantomData<V>` | 原样透传顶点属性，不做任何坐标变换 |
| `SolidFs` | 片元 | `&self`：`color` | 每个片元输出同一 `Color`，alpha 原样透传给 `BlendMode` |
| `GradientFs` | 片元 | `&self`：`from` / `to` / `dir` | 沿 `dir` 在 **normalized 空间**做线性渐变 |
| `TextureFs<'a>` | 片元 | `&self`：`texture` / `filter` / `wrap` | 按 UV varying 采样纹理（`type Varying = Vec2`） |

`SolidFs` / `GradientFs` 声明 `type Varying = ()`（不携带任何 varying），
顶点数据因此退化为零属性——`impl Interpolate for ()` 就是为它们准备的。
换来的是顶点缓冲更小、无插值开销，且着色结果与视口分辨率无关。

`TextureFs` 是唯一**携带 varying** 的内建片元着色器：UV 由顶点传入，
纹理本身在 `&self`。**无需新顶点着色器**——`Vertex2D` 的 `position` 与
`varying` 是独立字段，`Passthrough2D<Vec2>` 直接透传 UV 即可。

### 参数为什么只能放 `&self`

`FragmentShader::shade_fragment(&self, input)` **没有 uniform 参数**，且
`draw_arrays` 持 `&mut Renderer` 的同时只能持 `&Pipeline`，故 `&self`
无法放宽为 `&mut self`。加上「库内无全局状态」，参数只剩 `&self` 与 varying
两条通道。静态参数选 `&self` 字段（泛型单态化后通常不留开销）；要每帧变的
动画参数就重建 `Pipeline`——它是三个字段的按值结构体，重建零分配。

曾评估过「给 `FragmentInput` 加 uniform」：语义错位（`FragmentInput` 是 VS 的
**输出**，VS 拿不到 uniform，只能原样透传，等于复制 N 份却一次都不用），
且要动 ~45 处并给 `VertexCache` 加生命周期参数。已否决，勿重提。

### 陷阱：`GradientFs` 反向渐变要交换两端色

`t = dot(position, dir)` 钳制到 `[0, 1]`，而 `position` 恒在 `[0, 1]`，
故 `dir` 含负分量时 `t` 会整体落到区间外被钳到端点色。典型后果：
`dir = [-1.0, 0.0]` **不是**反向渐变，而是整片塌成 `from` 色（`t ∈ [-1, 0]`）。
要反向应交换 `from` / `to` 并保持 `dir` 为正。
由 `gradient_fs_negative_direction_collapses_to_from_color` 守护。

### 插值口径只有一处

`GradientFs` 调用 `Color::lerp`（`mod.rs` 中的 inherent 方法），
后者转发 `Interpolate::lerp` → `Interpolate for Color`。
**不要改用 `Color::blend`**：它的权重语义本身是标准的（`self` = src 乘 alpha，
与 GL 的 `SRC_ALPHA / ONE_MINUS_SRC_ALPHA` 同构），但返回值由 `Color::new`
构造、**alpha 恒置 255 且不插值 alpha 通道**，会把半透明端点色
（如 `from.a == 128`）错误地变成不透明。

## 纹理（`texture.rs`）

| 符号 | 位置 | 说明 |
|------|------|------|
| `WrapMode` | `:59` | `Clamp` / `Repeat`，单轴寻址；**无 `Mirror`** |
| `FilterMode` | `:73` | `Nearest` / `Bilinear` |
| `Texture2D<'a>` | `:113` | 借用 `&'a [u8]`，4 字段 + 格式，`Copy` |
| `try_new` | `:147` | 返回 `Result`，尾部 padding 容忍 |
| `new` | `:193` | **panic**（构造期不变量），`#[track_caller]` |
| `from_rows` | `:244` | 带 pitch 的源切片 → 紧凑纹理（静态方法） |
| `sample` | `:377` | 唯一真入口：`sample(uv, filter, wrap)` |
| `sample_nearest` / `_bilinear` / `_nearest_repeat` / `_bilinear_repeat` | `:462`–`:498` | 4 个便捷薄包装 |
| `wrap` / `texel` | `:307` / `:325` | 私有：单轴寻址 / 读解码单 texel |
| `floor_to_i32` | `:519` | 私有：**`no_std` 下的手写 floor** |

### 坐标约定：半 texel

```text
x = u × width − 0.5      y = v × height − 0.5
```

texel `i` 的中心落在 `(i + 0.5) / size`。**`u = 0` 落在 texel 0 的左边缘，
不是中心**；`u = 1.0` 落在最后一列右边缘，钳制后仍是最后一列——两端各得
完整 1 texel 覆盖。省略 `- 0.5` 会让边界缩进半像素并产生接缝。
由 `texel_centers_land_at_half_texel_offsets` 与
`clamp_covers_edge_texels_without_seam` 守护。

这与「视口变换是边缘空间」是同族决策，但**不可混用**：视口映射吃 normalized
屏幕坐标，纹理映射吃 varying UV，两者独立计算。

### 陷阱：`no_std` 没有 `f32::floor`

`#![no_std]` 环境无 std 无 libm，`floor` / `round` / `ceil` / `sqrt` **全部
不可用**（已实测确认）。这正是 `utils::floor_to_usize` / `ceil_to_usize`
手写取整的原因，`texture.rs` 同样必须手写 `floor_to_i32`。

它用「`as i32` 向零截断 + 符号判断」，**不是**「`as` 强转后修正」——
后者在 `|value| ≥ i32::MAX` 饱和到 `i32::MAX` 时，负数分支的 `truncated - 1`
会**下溢 panic**（该 bug 已实际触发过一次，见 `floor_to_i32_saturates_on_infinities`
的断言）。必须用 `saturating_sub(1)`。

### 陷阱：NaN 必须显式归零

`uv` 含 `NaN` 时，`NaN as i32` 饱和到 `0` → **静默采到左上角 texel**，
一个「看起来能跑」的错答案。`sample` 用 `is_finite` 同时拦 `NaN` 与 `±∞`
（`∞ × size` 仍是 `∞`，而 `f32` 溢出产生 `NaN`），返回透明黑。
这与 `utils::ceil_to_usize` 的「NaN → 1」是同一类历史教训。

### 陷阱：双线性不预乘 alpha

alpha **直通插值**（non-premultiplied），半透明纹理放大后有边缘光晕。
标准解法（先预乘、插值、再反预乘）会引入**第二条**取整口径，与本库
「插值口径只有一处」冲突，v1 刻意不做。
由 `bilinear_interpolates_alpha_straight_through` 记录该行为。

bilerp 的合成**必须 f32 域一次算完再取整**，口径 `(v + 0.5).clamp(..) as u8`。
不要折成两次 `Color::lerp`——累积 2~3 LSB 误差。注意 2×2 纹理的中点是
**四色平均**（四角权重各 0.25），不是两点折线。

### 边界契约

- **零尺寸纹理**允许构造，**任何采样恒返回透明黑**（不 panic）。`sample` 在
  `width == 0` 时必须**先短路**，否则 `wrap` 里 `limit as i32 - 1` 下溢。
- `try_new` 用**两次** `checked_mul`（`w*h` 与 `(w*h)*bpp`），任一溢出报
  `DimensionOverflow`，不能绕回小值蒙混过长度检查。
- `Repeat` 用 `rem_euclid` 而非 `%`（后者负数返回负余数，`as usize` 饱和越界）。
- `texel` 用**安全下标**，不用 `unsafe`：`try_new` 保证
  `texels.len() ≥ w*h*bpp` 且 `x < w, y < h` ⇒ 切片恒合法。（`core.rs` 用
  `get_unchecked_mut` 是帧缓冲每像素调用，纹理量级不同，先等基准数据说话。）

### 明确不做

mipmap（需自持有存储，破坏零分配）、`Mirror` 寻址（边界歧义需另定契约）、
`pitch` 字段（每 texel 多一次分支；改用静态的 `from_rows` 先转换）。

## 已解决的历史不一致

原先本节列的两条均已处理：

- **`VecVertexCache` 引用**：该类型从未存在，已删除 `cache.rs` 模块文档与
  `VertexCache` 结构体文档中的两处引用，并改为说明「库内没有自持有存储的变体」。
- **`alloc` feature 是死的**：`Cargo.toml` 已删除该 feature，`lib.rs` 的
  特征表同步改为如实描述 `programmable` / `simd` 两个。库内零分配，槽位由
  调用方借出，因此本就不需要它。

## 验证

```bash
cargo check --no-default-features                          # 确认本目录被完全排除
cargo check --no-default-features --features programmable   # 单独开启
cargo test --lib                                            # 本目录 125 个测试
cargo run --example programmable                            # 最小可用示例
```
