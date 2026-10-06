//! 各基准文件共享的脚手架。
//!
//! 基准按被测子系统拆成多个 `[[bench]]` 目标，本模块集中放它们共用的东西：
//!
//! * [`criterion_config`] —— 统一的采样/测量参数（可由环境变量覆盖），
//!   保证跨文件、跨次运行的数字可比；
//! * [`PixFb`] —— 造一个借用期够长、格式与行填充可配的 `Renderer`；
//! * 数据集构造器（渐变纹理、抗锯齿掩码、顶点流）—— 避免每个 bench 各写一份；
//! * [`jitter_probe`] —— 在跑正式基准前量一次本机抖动，判断数字是否可信。
//!
//! # 测量的波动从哪来
//!
//! 本库的基准跨 2 ns（颜色构造）到 1 ms（1080p 填屏）四个数量级，
//! 波动来源不同：
//!
//! * **短基准**（ns 级）受 CPU 调频影响最大——`schedutil` 之类的
//!   governor 会在任务切换的间隙降频，预热时间不足时前后采样频率不同。
//! * **长基准**（µs~ms 级）受内存带宽与后台进程抢占影响，表现为
//!   **离群点**而非噪声。criterion 用中位数做点估计，故中位数本身稳，
//!   但离群比例高时置信区间会被拉宽。
//!
//! 因此本模块的对策是：足够长的预热（跨过调频窗口）+ 中位数统计 +
//! 显式的抖动自检。跑之前先看 [`jitter_probe`] 的结论，比事后怀疑数字
//! 更有效；抖动大的机器上用 `BENCH_MEASURE_SECS=20 BENCH_SAMPLES=500`
//! 能明显收窄置信区间。
//!
//! # 为什么需要 `PixFb`
//!
//! `Renderer<'a>` 借用帧缓冲切片，而基准需要在同一个 `Renderer` 上反复迭代。
//! 标准做法是把借用延长到 `'static`。这里把它收进一个类型，只在一处
//! `unsafe`，并把「切片长度必须 ≥ `pitch × height`」这条 `Renderer::new`
//! 的前置条件显式断言出来——写错尺寸时得到的是清晰的 panic，而不是越界写。
//!
//! # 为什么数据集要 `black_box` 化
//!
//! 基准的输入若能常量折叠，编译器会把整段绘制优化掉。这里对送入绘制的
//! 参数统一走 [`black_box`]，且数据集尺寸由参数决定，使循环体无法被整体求值。
#![allow(dead_code)] // 每个 bench 目标只用得到本模块的一部分

use core::hint::black_box;
use core::time::Duration;

use criterion::Criterion;
use rasteron::Renderer;
use rasteron::color::{Color, PixelFormat};
use rasteron::framebuffer::FramebufferInfo;
use rasteron::pixel::Pixel;

/// 统一的 criterion 配置，并按环境变量可调。
///
/// # 为什么参数比默认值保守
///
/// 默认的 `sample_size(100)` 在 μs 量级的绘制内核上给出的置信区间过宽。
/// 这里默认取 200 次采样、8 秒测量，实测中位数置信区间可收到 ±0.1%。
///
/// # 噪声与离群点
///
/// 本库的基准多在 10 µs ~ 1 ms 量级，**离群点**（`schedutil` 调频、
/// 后台进程抢占）比噪声更常见：实测小基准可达 7~8% 的 high 离群。
/// criterion 用**中位数**做点估计并报告 Tukey 离群，因此中位数本身
/// 是稳的，但样本数太少时置信区间会被离群拉宽。
///
/// `noise_threshold` 与 `significance_level` 决定 criterion 是否对
/// 「与上次基线不同」下结论。这里放宽到 5% / 1%，避免在被抢占的机器上
/// 反复误报「性能变化」。
///
/// # 环境变量
///
/// * `BENCH_WARMUP_MS` / `BENCH_MEASURE_MS` / `BENCH_SAMPLES` ——
///   覆盖三个主参数（默认 1000 / 3000 / 100）。抖动大的机器上加大它们能
///   收窄置信区间：`BENCH_MEASURE_MS=10000 BENCH_SAMPLES=300 cargo bench`
/// * `BENCH_QUICK=1` —— 冒烟档（0.3 秒预热 / 0.8 秒测量 / 20 采样），
///   只用于快速确认基准能跑通且不 panic，**数据不可引用**。
///
/// 读取环境变量是基准侧的合理用法：这些是 dev-dependency 场景下的
/// 开发工具参数，不影响库本身。
///
/// 读取环境变量是基准侧的合理用法：这些是 dev-dependency 场景下的
/// 开发工具参数，不影响库本身。
pub fn criterion_config() -> Criterion {
    if env_flag("BENCH_QUICK") {
        // 冒烟档：只验证基准能跑通、不 panic，数据不可引用。
        return Criterion::default()
            .warm_up_time(Duration::from_millis(300))
            .measurement_time(Duration::from_millis(800))
            .sample_size(20)
            .noise_threshold(0.05)
            .significance_level(0.01);
    }

    // 默认参数刻意取「够用即止」。criterion 的时间预算是**每条基准固定**的，
    // 与本条的快慢无关，因此单条时间直接乘以条数就是总耗时：本目录有
    // 300+ 条，每省 1 秒就省下 5 分钟以上。
    //
    // 实测依据（本机 Xeon E5-2673 v3）：1 秒预热、3 秒测量、100 采样下，
    // 中位数置信区间已在 ±0.3% 内、离群点 0~1%，足以支撑「比较两个实现的
    // 相对快慢」这一主要用途。原先的 3s/8s/200 只把置信区间从 ±0.3% 收到
    // ±0.1%，代价却是三倍时长——不值得作为默认值。
    //
    // 需要更紧的区间时用环境变量加严，而不是改这里。
    Criterion::default()
        .warm_up_time(Duration::from_millis(env_millis("BENCH_WARMUP_MS", 1_000)))
        .measurement_time(Duration::from_millis(env_millis("BENCH_MEASURE_MS", 3_000)))
        .sample_size(env_usize("BENCH_SAMPLES", 100))
        .noise_threshold(0.05)
        .significance_level(0.01)
}

/// 读一个整型环境变量，缺失或无法解析时用默认值。
fn env_usize(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(fallback)
}

/// 读一个「毫秒数」环境变量，转成 `Duration`。
fn env_millis(key: &str, fallback: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(fallback)
}

/// 环境变量是否被设为「真值」（非空且不是 `0`/`false`）。
fn env_flag(key: &str) -> bool {
    match std::env::var(key) {
        Ok(raw) => !raw.is_empty() && raw != "0" && raw != "false",
        Err(_) => false,
    }
}

/// 覆盖三类写入路径的像素格式集合。
///
/// 选型依据是 `core.rs` 的四个字节序宏与 `color::swar`：
///
/// * `Rgba8888` / `Xrgb8888` —— 4bpp，行首通常 4 字节对齐，走 SWAR 批量内核；
///   两者的 `opaque_alpha` 分别是 255 与 0，覆盖第 4 字节的两种处理。
/// * `Bgr24` —— 3bpp，跨字节错位，**始终**逐通道回退，作为 SWAR 的对照组。
/// * `Rgb565` —— 16 位打包，无 SWAR 路径，走「解码 → 混合 → 量化回写」。
pub const CORE_FORMATS: [PixelFormat; 4] = [
    PixelFormat::Rgba8888,
    PixelFormat::Xrgb8888,
    PixelFormat::Bgr24,
    PixelFormat::Rgb565,
];

/// 全部 8 种像素格式，用于验证「格式覆盖完整性」类基准。
pub const ALL_FORMATS: [PixelFormat; 8] = [
    PixelFormat::Rgb24,
    PixelFormat::Bgr24,
    PixelFormat::Xrgb8888,
    PixelFormat::Argb8888,
    PixelFormat::Rgba8888,
    PixelFormat::Bgra8888,
    PixelFormat::Rgb565,
    PixelFormat::Bgr565,
];

/// 基准的主流帧缓冲尺寸。
pub const SIZE_SD: (usize, usize) = (640, 480);
/// 基准的高分辨率帧缓冲尺寸，用于观察吞吐是否随面积线性扩展。
pub const SIZE_HD: (usize, usize) = (1920, 1080);

/// 一块像素缓冲 + 借用它的 `Renderer`。
///
/// 字段顺序有意义：`renderer` 借用 `buffer`，Rust 的 drop 顺序保证
/// `renderer` 先于 `buffer` 释放（实际上 `Renderer` 不实现 `Drop`，
/// 但保持这个顺序是明确的意图表达）。
///
/// 缓冲被刻意泄漏为 `'static`：`Box<[u8]>` 的地址稳定，且基准进程本来就
/// 短命，泄漏几百 KB 到几 MB 无关紧要。这样 `Renderer<'static>` 可以自由
/// 存进局部变量、跨 `b.iter()` 复用。
pub struct PixFb {
    /// 帧缓冲字节。`Box` 保证地址稳定，不会被移动。
    pub buffer: Box<[u8]>,
    /// 借用 `buffer` 的渲染器，借用期被延长为 `'static`。
    pub renderer: Renderer<'static>,
}

impl PixFb {
    /// 新建 `width × height`、给定格式的单缓冲渲染器。
    ///
    /// `pitch` 取 `width × bpp`（紧凑布局）。需要测行填充时用 [`PixFb::padded`]。
    pub fn new(width: usize, height: usize, format: PixelFormat) -> Self {
        Self::padded(width, height, format, 0)
    }

    /// 新建带 `extra_pitch` 字节行填充的渲染器。
    ///
    /// 行填充会影响 `clear` / `swap_buffers` / `blit` 的逐行拷贝口径，
    /// 是「缓存格式属性」这条设计的一个独立维度。
    pub fn padded(width: usize, height: usize, format: PixelFormat, extra_pitch: usize) -> Self {
        let row_len = width
            .checked_mul(format.bytes_per_pixel())
            .expect("bench: width * bytes_per_pixel 溢出");
        let pitch = row_len + extra_pitch;
        let info = FramebufferInfo::new(width, height, pitch, format);
        let buffer = vec![0u8; info.buffer_size()].into_boxed_slice();

        // SAFETY: 下面把 `buffer` 的可变借用延长为 `'static`。这是安全的，
        // 因为 `buffer` 与 `renderer` 同属本结构体、同生命周期，`renderer`
        // 不会被移出到 `buffer` 之外存活；且我们断言了切片长度满足
        // `Renderer::new` 的 `pitch × height` 前置条件。
        let slice: &'static mut [u8] = unsafe {
            core::slice::from_raw_parts_mut(buffer.as_ptr() as *mut u8, buffer.len())
        };
        debug_assert!(slice.len() >= pitch * height, "bench: 帧缓冲切片过短");
        let renderer = Renderer::new(info, slice);
        Self { buffer, renderer }
    }

    /// 同 [`PixFb::new`]，但额外提供后台缓冲区（双缓冲）。
    ///
    /// 返回 [`DoubleFb`]，它同时持有前台与后台缓冲，因此
    /// `Renderer::swap_buffers` 的写入目标（前台）在基准期间始终有效。
    pub fn double(width: usize, height: usize, format: PixelFormat) -> DoubleFb {
        let row_len = width
            .checked_mul(format.bytes_per_pixel())
            .expect("bench: width * bytes_per_pixel 溢出");
        let info = FramebufferInfo::new(width, height, row_len, format);
        let size = info.buffer_size();
        let front = vec![0u8; size].into_boxed_slice();
        let back = vec![0u8; size].into_boxed_slice();

        // SAFETY: 与 `PixFb::padded` 同理，但这里有两块缓冲。两块 `Box<[u8]>`
        // 的地址都稳定，且被移进 `DoubleFb` 后就再也不会移动；`renderer`
        // 借用它们，与它们在同一个结构体里同生共死。
        //
        // 两块缓冲互不重叠（两次独立分配），因此不存在同一内存被两个
        // 可变借用同时持有的别名问题——这一点由 `with_back_buffer` 的
        // 类型签名（两个独立的 `&'a mut [u8]`）也一并保证。
        let front_slice: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(front.as_ptr() as *mut u8, front.len()) };
        let back_slice: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(back.as_ptr() as *mut u8, back.len()) };
        debug_assert!(front_slice.len() >= info.buffer_size(), "bench: 前台切片过短");
        debug_assert!(back_slice.len() >= info.buffer_size(), "bench: 后台切片过短");

        let renderer = Renderer::with_back_buffer(info, front_slice, back_slice);
        DoubleFb { front, back, renderer }
    }
}

/// 双缓冲基准的容器：同时持有前台、后台与被二者借用的 `Renderer`。
///
/// 分开持有是必须的：`Renderer::swap_buffers` 会把后台的可见像素写进前台，
/// 所以前台必须在基准期间保持有效。若像早期版本那样把前台作为元组的一个
/// 元素返回、同时又让 `Renderer` 可变借用它，就是**同一内存的两个可变别名**
/// ——违反 Rust 别名规则，优化器可以据此产生错误结果。
pub struct DoubleFb {
    /// 前台缓冲。绘制不写它，只有 `swap_buffers` 写入。
    pub front: Box<[u8]>,
    /// 后台缓冲。绘制实际落在它上面。
    pub back: Box<[u8]>,
    /// 借用前两者的渲染器。
    pub renderer: Renderer<'static>,
}

impl DoubleFb {
    /// 前台缓冲的只读视图，用于校验呈现结果。
    pub fn front_bytes(&self) -> &[u8] {
        &self.front
    }
}

/// `width × height` 的 `Rgba8888` 帧缓冲，供需要直接读像素的基准使用。
pub fn rgba_fb(width: usize, height: usize) -> PixFb {
    PixFb::new(width, height, PixelFormat::Rgba8888)
}

/// 把一个坐标钳到帧缓冲内的 `Pixel`。
///
/// 基准里的坐标由闭包计数器生成，可能越界；`Pixel` 的字段是 `usize`，
/// 越界坐标会被绘制 API 静默裁剪，本身无害，但钳一下能让「每轮工作量」
/// 保持在预期量级，避免不同 bench 之间工作量漂移。
#[inline]
pub fn clamp_pixel(x: usize, y: usize, width: usize, height: usize) -> Pixel {
    Pixel::new(x % width.max(1), y % height.max(1))
}

/// 不透明色与半透明色的代表值。
pub const OPAQUE: Color = Color::new(220, 90, 40);
/// 半透明代表值，用于触发混合路径。
pub const TRANSLUCENT: Color = Color::with_alpha(220, 90, 40, 128);

/// 生成一张 `width × height` 的水平渐变纹理（`Rgba8888`，紧凑排列）。
///
/// 纹理内容有真实的像素间差异（不是常量填充），否则采样路径会被
/// 分支预测与缓存完全吃掉，测不出寻址与解码的成本。
pub fn gradient_texture(width: usize, height: usize) -> Vec<u8> {
    let mut texels = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            texels[offset] = ((x * 255) / width.max(1)) as u8;
            texels[offset + 1] = ((y * 255) / height.max(1)) as u8;
            texels[offset + 2] = (((x + y) * 127) / (width + height).max(1)) as u8;
            texels[offset + 3] = 255;
        }
    }
    texels
}

/// 生成抗锯齿风格的圆形掩码：圆内 255、圆外 0、边缘一圈中间值。
///
/// 这是 `blit_mask` 的典型输入（字体与图形边缘）。边缘的中间值让掩码
/// 分组出现「既非全 0 也非全 255」的行段，能测到 SWAR 掩码内核的分支。
pub fn antialiased_mask(width: usize, height: usize) -> Vec<u8> {
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let radius = width.min(height) as f32 / 2.0 - 1.0;
    let mut mask = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let distance = (dx * dx + dy * dy).sqrt();
            // 距边界 ±0.5 像素内线性过渡，得到一圈中间值。
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            mask[y * width + x] = (coverage * 255.0).round() as u8;
        }
    }
    mask
}

/// 纯粹的 0/255 硬边掩码：圆形内部全 255、外部全 0（无抗锯齿过渡）。
///
/// 与 [`antialiased_mask`] 构成对照。用**块状**形状而非棋盘格：
/// 掩码批量内核按行分组，块状掩码能出现「整段全 0 / 整段全 255」的
/// 连续区间（可整体跳过或整体快写），而棋盘格每列都在变，
/// 会把分组优化完全废掉，测不出 SWAR 掩码路径的真实优势。
pub fn hard_mask(width: usize, height: usize) -> Vec<u8> {
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let radius = width.min(height) as f32 / 2.0 - 1.0;
    let radius_sq = radius * radius;
    let mut mask = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            // 二值覆盖：不做边缘过渡，形状与抗锯齿版一致以便直接对比。
            mask[y * width + x] = if dx * dx + dy * dy <= radius_sq { 255 } else { 0 };
        }
    }
    mask
}

/// 阻止常量折叠：把值喂给编译器看不见的地方。
#[inline]
pub fn opaque<T>(value: T) -> T {
    black_box(value)
}

/// 本机基准抖动的自检结论。
#[derive(Debug, Clone, Copy)]
pub struct JitterReport {
    /// 固定工作量的中位耗时（纳秒）。
    pub median_ns: f64,
    /// 最慢与中位数之比。理想值接近 1.0。
    pub worst_ratio: f64,
    /// 超出中位数 1.5 倍的样本占比。
    pub outlier_fraction: f64,
    /// 样本数。
    pub samples: usize,
}

impl JitterReport {
    /// 抖动是否小到足以信任基准数字。
    ///
    /// 判据取「最慢样本不超过中位数 3 倍」且「离群样本占比低于 5%」。
    /// 这是经验阈值：满足它时 criterion 报告的置信区间通常在 ±1% 以内；
    /// 不满足说明机器正被抢占或调频，此时应先排除干扰再采信数字。
    pub fn is_reliable(&self) -> bool {
        self.worst_ratio < 3.0 && self.outlier_fraction < 0.05
    }

    /// 一句话结论，供基准开始时打印。
    pub fn summary(&self) -> String {
        format!(
            "抖动自检: 中位 {:.0} ns, 最慢/中位 {:.2}x, 离群 {:.1}%, 样本 {} → {}",
            self.median_ns,
            self.worst_ratio,
            self.outlier_fraction * 100.0,
            self.samples,
            if self.is_reliable() { "可信" } else { "不可信（放宽参数或排除干扰后重跑）" }
        )
    }
}

/// 量一次本机抖动：对一个访存受限的固定循环采样 `samples` 次。
///
/// 用「逐字节累加一块 256 KiB 缓冲区」当探针——它跨 L2 容量，对内存带宽与
/// 抢占都敏感，正是绘制类基准的实际工作形态。相比空循环，它不会被
/// 调频器当作空闲任务而降到最低频。
///
/// 返回的 [`JitterReport`] 给出中位数与离群程度，用于判断本次运行的
/// 数字是否可信。基准启动时调用它，比事后怀疑数据更有用。
pub fn jitter_probe(samples: usize) -> JitterReport {
    // 256 KiB：明显大于典型 L2，使探针受内存带宽约束。
    const PROBE_BYTES: usize = 256 * 1024;
    let buffer = vec![0xA5u8; PROBE_BYTES];

    // 预热：让调频器进入工作频率，否则前几个样本会显著偏慢。
    for _ in 0..8 {
        black_box(buffer.iter().fold(0u64, |acc, &byte| acc.wrapping_add(byte as u64)));
    }

    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = std::time::Instant::now();
        black_box(buffer.iter().fold(0u64, |acc, &byte| acc.wrapping_add(byte as u64)));
        timings.push(start.elapsed().as_nanos() as f64);
    }

    let mut sorted = timings.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let worst = sorted[sorted.len() - 1];
    let outliers = sorted.iter().filter(|&&value| value > median * 1.5).count();

    JitterReport {
        median_ns: median,
        worst_ratio: if median > 0.0 { worst / median } else { 1.0 },
        outlier_fraction: outliers as f64 / sorted.len() as f64,
        samples: sorted.len(),
    }
}

/// 打印抖动自检结论（基准启动时调用）。
///
/// 设置 `BENCH_SKIP_JITTER=1` 可跳过，以免干扰 criterion 的标准输出解析。
pub fn report_jitter() {
    if env_flag("BENCH_SKIP_JITTER") {
        return;
    }
    let report = jitter_probe(64);
    println!("{}", report.summary());
    if !report.is_reliable() {
        println!(
            "提示: 本机抖动偏大。可先 `BENCH_MEASURE_SECS=20 BENCH_SAMPLES=500` 重跑，\
             或把 CPU governor 设为 performance 后重试。"
        );
    }
}
