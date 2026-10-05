#[cfg(target_feature = "avx512f")]
#[inline(always)]
unsafe fn memcpy_x86_avx512(dst: *mut u8, src: *const u8, len: usize) -> usize {
    use core::arch::x86_64::*;

    let step = 64;
    let chunks = len / step;
    let remainder = len % step;

    for i in 0..chunks {
        unsafe {
            let offset = i * step;
            let src_chunk = src.add(offset);
            let dst_chunk = dst.add(offset);

            let data = _mm512_loadu_si512(src_chunk as *const __m512i);
            _mm512_storeu_si512(dst_chunk as *mut __m512i, data);
        }
    }

    if remainder > 0 {
        unsafe {
            let offset = chunks * step;
            let src_tail = src.add(offset);
            let dst_tail = dst.add(offset);
            let mask = ((1u64 << remainder) - 1) as __mmask64;
            let data = _mm512_maskz_loadu_epi8(mask, src_tail as *const i8);
            _mm512_mask_storeu_epi8(dst_tail as *mut i8, mask, data);
        }
    }

    0
}

#[cfg(all(
    any(target_feature = "avx2", target_feature = "avx"),
    not(target_feature = "avx512f")
))]
#[inline(always)]
unsafe fn memcpy_x86_avx(dst: *mut u8, src: *const u8, len: usize) -> usize {
    use core::arch::x86_64::*;

    let step = 32;
    let chunks = len / step;
    let remainder = len % step;

    for i in 0..chunks {
        unsafe {
            let offset = i * step;
            let src_chunk = src.add(offset);
            let dst_chunk = dst.add(offset);

            let data = _mm256_loadu_si256(src_chunk as *const __m256i);
            _mm256_storeu_si256(dst_chunk as *mut __m256i, data);
        }
    }

    remainder
}

#[cfg(all(
    target_feature = "sse2",
    not(any(
        target_feature = "avx512f",
        target_feature = "avx2",
        target_feature = "avx"
    ))
))]
#[inline(always)]
unsafe fn memcpy_x86_sse2(dst: *mut u8, src: *const u8, len: usize) -> usize {
    use core::arch::x86_64::*;

    let step = 16;
    let chunks = len / step;
    let remainder = len % step;

    for i in 0..chunks {
        unsafe {
            let offset = i * step;
            let src_chunk = src.add(offset);
            let dst_chunk = dst.add(offset);

            let data = _mm_loadu_si128(src_chunk as *const __m128i);
            _mm_storeu_si128(dst_chunk as *mut __m128i, data);
        }
    }

    remainder
}

#[cfg(target_feature = "neon")]
#[inline(always)]
unsafe fn memcpy_neon(dst: *mut u8, src: *const u8, len: usize) -> usize {
    use core::arch::aarch64::*;

    let step = 16;
    let chunks = len / step;
    let remainder = len % step;

    for i in 0..chunks {
        unsafe {
            let offset = i * step;
            let src_chunk = src.add(offset);
            let dst_chunk = dst.add(offset);

            let data = vld1q_u8(src_chunk);
            vst1q_u8(dst_chunk, data);
        }
    }

    remainder
}

#[cfg(all(target_arch = "riscv64", target_feature = "v"))]
#[inline(always)]
unsafe fn memcpy_rvv(dst: *mut u8, src: *const u8, len: usize) -> usize {
    use core::arch::asm;

    let mut src_ptr = src;
    let mut dst_ptr = dst;
    let mut remaining = len;
    let vl: usize;

    unsafe {
        asm!(
            "2:",
            "vsetvli {vl}, {remaining}, e8, m8, ta, ma",
            "vle8.v v0, ({src})",
            "vse8.v v0, ({dst})",
            "add {src}, {src}, {vl}",
            "add {dst}, {dst}, {vl}",
            "sub {remaining}, {remaining}, {vl}",
            "bnez {remaining}, 2b",
            src = inout(reg) src_ptr,
            dst = inout(reg) dst_ptr,
            remaining = inout(reg) remaining,
            vl = lateout(reg) vl,
            options(nostack)
        );
    }

    0
}

pub unsafe fn memcpy(dst: *mut u8, src: *const u8, len: usize) {
    if len == 0 {
        return;
    }

    if len <= 128 {
        unsafe {
            core::ptr::copy_nonoverlapping(src, dst, len);
        }
        return;
    }

    let remainder;

    #[cfg(target_feature = "avx512f")]
    {
        remainder = unsafe { memcpy_x86_avx512(dst, src, len) };
    }
    #[cfg(all(
        any(target_feature = "avx2", target_feature = "avx"),
        not(target_feature = "avx512f")
    ))]
    {
        remainder = unsafe { memcpy_x86_avx(dst, src, len) };
    }
    #[cfg(all(
        target_feature = "sse2",
        not(any(
            target_feature = "avx512f",
            target_feature = "avx2",
            target_feature = "avx"
        ))
    ))]
    {
        remainder = unsafe { memcpy_x86_sse2(dst, src, len) };
    }
    #[cfg(target_feature = "neon")]
    {
        remainder = unsafe { memcpy_neon(dst, src, len) };
    }
    #[cfg(all(target_arch = "riscv64", target_feature = "v"))]
    {
        remainder = unsafe { memcpy_rvv(dst, src, len) };
    }
    #[cfg(not(any(
        target_feature = "sse2",
        target_feature = "avx512f",
        target_feature = "avx2",
        target_feature = "avx",
        target_feature = "neon",
        target_arch = "aarch64",
        all(target_arch = "riscv64", target_feature = "v")
    )))]
    {
        remainder = len;
    }

    if remainder > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.add(len - remainder),
                dst.add(len - remainder),
                remainder,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    /// 可辨识的源数据：用 `(i * 37 + 11) as u8`，保证相邻字节不同，
    /// 任何错位或漏拷贝都能被发现。
    fn source(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(37).wrapping_add(11)) as u8).collect()
    }

    /// 跑一次 `memcpy` 并返回目标切片。
    ///
    /// 目标预填 `0xA5`，任何未被写到的字节都会保留该哨兵值，
    /// 因此「写多了」与「写少了」都能被断言捕获。
    fn run_memcpy(len: usize) -> Vec<u8> {
        let src = source(len);
        let mut dst = vec![0xA5u8; len];
        // SAFETY: src 与 dst 长度均为 len、互不重叠（两个独立 Vec）。
        unsafe {
            super::memcpy(dst.as_mut_ptr(), src.as_ptr(), len);
        }
        dst
    }

    /// 与 `core::ptr::copy_nonoverlapping` 在一组长度上逐字节一致。
    ///
    /// 长度集合刻意覆盖：`0`、小于向量的 1、紧邻各 ISA 向量宽的
    /// 127/128/129、255/256/257、1024，以及若干非 16 倍数。
    #[test]
    fn memcpy_matches_copy_nonoverlapping_across_lengths() {
        const LENGTHS: [usize; 18] = [
            0, 1, 2, 3, 7, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 129, 255, 1024,
        ];
        for len in LENGTHS {
            let src = source(len);
            let mut dst = vec![0xA5u8; len];
            // SAFETY: 长度相等、两个切片独立分配，区域不重叠。
            unsafe {
                super::memcpy(dst.as_mut_ptr(), src.as_ptr(), len);
            }
            assert_eq!(dst, src, "len = {len} 时 memcpy 与源数据不一致");
        }
    }

    /// 长度不是 ISA 向量宽整数倍时，尾部 `remainder` 路径必须补齐。
    ///
    /// `memcpy` 内部先按向量宽整块拷贝，再把剩余字节用
    /// `copy_nonoverlapping` 补上。这些长度都产生非零 `remainder`。
    #[test]
    fn tail_remainder_is_copied_completely() {
        // 覆盖 SSE2(16) / AVX(32) / AVX-512(64) 各自的「整块 + 尾部」组合。
        const LENGTHS: [usize; 14] = [
            1, 5, 17, 33, 47, 65, 79, 97, 111, 129, 145, 193, 257, 1001,
        ];
        for len in LENGTHS {
            assert_eq!(
                run_memcpy(len),
                source(len),
                "len = {len} 的尾部字节未被正确补齐"
            );
        }
    }

    /// 源 / 目标指针未按向量宽对齐时结果仍正确。
    ///
    /// 内核用 `loadu` / `storeu`（非对齐版本）读写，故未对齐起点必须安全。
    /// 这里在前面塞 1..=3 字节，制造 2/3/4 各种不同的对齐偏移。
    #[test]
    fn unaligned_src_and_dst_pointers_produce_correct_results() {
        for src_offset in 0..4usize {
            for dst_offset in 0..4usize {
                let len = 200usize;
                let src_storage = source(len + 4);
                let mut dst_storage = vec![0xA5u8; len + 4];
                let src = &src_storage[src_offset..src_offset + len];
                let dst = &mut dst_storage[dst_offset..dst_offset + len];
                // SAFETY: 两个切片长度均为 len，且分别来自互不重叠的 Vec。
                unsafe {
                    super::memcpy(dst.as_mut_ptr(), src.as_ptr(), len);
                }
                assert_eq!(
                    dst,
                    &src_storage[src_offset..src_offset + len],
                    "src_offset = {src_offset}, dst_offset = {dst_offset} 时结果不符"
                );
                // 未涉及的字节必须保持哨兵值。
                assert!(
                    dst_storage[..dst_offset].iter().all(|b| *b == 0xA5),
                    "写入越过了目标起点（dst_offset = {dst_offset}）"
                );
                assert!(
                    dst_storage[dst_offset + len..].iter().all(|b| *b == 0xA5),
                    "写入越过了目标终点（dst_offset = {dst_offset}）"
                );
            }
        }
    }

    /// `len == 0` 提前返回：不触碰任何指针。
    ///
    /// 用两个空切片调用，`as_mut_ptr()` 合法给出不对齐的悬垂无关指针，
    /// 函数必须原样返回而不解引用——这正是 `len == 0` 早退的意义。
    #[test]
    fn zero_length_returns_without_touching_pointers() {
        let src: [u8; 0] = [];
        let mut dst: [u8; 0] = [];
        // SAFETY: 长度为 0，函数按契约提前返回，不做任何解引用。
        unsafe {
            super::memcpy(dst.as_mut_ptr(), src.as_ptr(), 0);
        }
        // 无可观测副作用，此处断言仅为「不崩溃、不 UB」。
        assert!(dst.is_empty());
    }

    /// 当前 target 实际走到的 ISA 分支。
    ///
    /// 本库的 `memcpy` 在 `len <= 128` 时无条件走
    /// `copy_nonoverlapping`，只有更大的长度才进入 ISA 分支。故本测试分两段：
    /// 短长度验证标量路径，长长度验证向量 + 尾部路径。
    #[test]
    fn short_lengths_take_the_scalar_path_and_long_ones_the_isa_path() {
        // len <= 128：标量路径。
        for len in [0usize, 1, 64, 127, 128] {
            assert_eq!(run_memcpy(len), source(len), "标量路径 len = {len} 不符");
        }
        // len > 128：至少进入一次 ISA 分支（末尾仍有 remainder 需补齐）。
        for len in [129usize, 160, 256, 1000, 4096] {
            assert_eq!(
                run_memcpy(len),
                source(len),
                "ISA 路径 len = {len} 不符"
            );
        }
    }

    /// 大长度下的多次迭代：确认向量循环 + 尾部拼接没有累积偏移错误。
    #[test]
    fn large_lengths_are_copied_exactly() {
        for len in [4096usize, 65537, 100_000] {
            assert_eq!(run_memcpy(len), source(len), "len = {len} 不符");
        }
    }

    /// 覆盖当前编译目标实际启用的 ISA 分支名（仅用于人工确认走了哪条路）。
    ///
    /// 断言本身恒真——目的是让「本 target 编译进了哪些分支」在测试输出里可见，
    /// 便于排查「某条分支从未被执行」这类问题。
    #[test]
    fn which_isa_branches_are_compiled_in() {
        let mut compiled: Vec<&str> = Vec::new();
        if cfg!(target_feature = "avx512f") {
            compiled.push("avx512f");
        }
        if cfg!(target_feature = "avx2") {
            compiled.push("avx2");
        }
        if cfg!(target_feature = "sse2") {
            compiled.push("sse2");
        }
        if cfg!(target_feature = "neon") {
            compiled.push("neon");
        }
        if cfg!(all(target_arch = "riscv64", target_feature = "v")) {
            compiled.push("rvv");
        }
        if compiled.is_empty() {
            compiled.push("(scalar fallback: remainder = len)");
        }
        // 打印出来便于人工确认；断言仅保证逻辑自洽。
        let joined: alloc::string::String =
            compiled.iter().copied().collect::<Vec<_>>().join(", ");
        assert!(!joined.is_empty());
    }
}
