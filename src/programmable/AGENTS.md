# src/programmable — 2D 可编程管线

**生成时间:** 2026-10-06 · 3 文件 2866 行
**feature 门控:** 整个目录由 `#[cfg(feature = "programmable")]` 包裹（`lib.rs:122`），默认开启

这是全库**唯一**引入 `f32` 的地方。关闭该 feature 后几何计算回到全整数
（父级 `angle.rs` 的圈值路径），这条契约由 `cargo check --no-default-features` 守护。

## 三个文件

| 文件 | 行数 | 内容 |
|------|------|------|
| `mod.rs` | 2376 | 类型体系、`Pipeline`、3 个 `rasterize_*` 内核、2 个公开绘制入口 |
| `cache.rs` | 386 | `VertexCache<'a, V>`——按顶点索引复用着色结果的 post-transform cache |
| `shader.rs` | 104 | `Passthrough2D<V>`——唯一内建顶点着色器（原样透传，`PhantomData` 绑类型） |

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
| `visible_bounds` | `:453` | 帧缓冲 ∩ viewport ∩ scissor，三者交集为空则整批图元直接成功返回（连顶点着色都不执行） |
| `write_fragment_color` | `:477` | 按 `BlendMode` 落盘。**取整不在这里做**——各图元落位规则不同（点向零取整、线四舍五入） |
| `rasterize_point` | `:512` | 二值可见判定，无参数域裁剪 |
| `rasterize_line` | `:541` | 参数域裁剪 + 插值 |
| `rasterize_triangle` | `:605` | 重心坐标插值，`w0/w1/w2` 除以 `area` |

公开入口只有两个：`draw_arrays`（`:688`）与 `draw_indexed`（`:778`），
都返回 `Result<(), DrawError>`——索引越界是**调用方程序错误**，
fail-fast 上抛而非静默跳过整条图元。

## 陷阱

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
cargo test --lib                                            # 本目录 61 个测试
cargo run --example programmable                            # 最小可用示例
```
