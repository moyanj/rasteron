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
