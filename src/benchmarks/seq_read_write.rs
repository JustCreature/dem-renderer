use crate::utils::*;

#[cfg(target_arch = "aarch64")]
pub(crate) fn seq_read_simd(data: &[f32]) {
    use core::arch::aarch64::*;

    let (ticks, _) = profiling::timed("seq_read", || unsafe {
        let mut acc0 = vdupq_n_f32(0.0);
        let mut acc1 = vdupq_n_f32(0.0);
        let mut acc2 = vdupq_n_f32(0.0);
        let mut acc3 = vdupq_n_f32(0.0);

        for chunk in data.chunks_exact(16) {
            let ptr = chunk.as_ptr();

            let v0 = vld1q_f32(ptr);
            let v1 = vld1q_f32(ptr.add(4));
            let v2 = vld1q_f32(ptr.add(8));
            let v3 = vld1q_f32(ptr.add(12));

            acc0 = vaddq_f32(acc0, v0);
            acc1 = vaddq_f32(acc1, v1);
            acc2 = vaddq_f32(acc2, v2);
            acc3 = vaddq_f32(acc3, v3);
        }

        let sum01 = vaddq_f32(acc0, acc1);
        let sum23 = vaddq_f32(acc2, acc3);
        let total = vaddq_f32(sum01, sum23);

        let sum = vaddvq_f32(total);

        let remainder: f32 = data.chunks_exact(16).remainder().iter().sum();

        std::hint::black_box(sum + remainder);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("seq_read_simd: {:.1} GB/s", gb_per_sec);
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn random_read_simd(data: &[f32]) {
    use core::arch::aarch64::*;

    let mut indices: Vec<usize> = (0..N).collect();
    shuffle(&mut indices);

    let (ticks, _) = profiling::timed("random_read_simd", || unsafe {
        let ptr = data.as_ptr();
        let mut acc0 = vdupq_n_f32(0.0);
        let mut acc1 = vdupq_n_f32(0.0);
        let mut acc2 = vdupq_n_f32(0.0);
        let mut acc3 = vdupq_n_f32(0.0);

        for chunk in indices.chunks_exact(16) {
            let v0 = vld1q_f32(
                [
                    *ptr.add(chunk[0]),
                    *ptr.add(chunk[1]),
                    *ptr.add(chunk[2]),
                    *ptr.add(chunk[3]),
                ]
                .as_ptr(),
            );
            let v1 = vld1q_f32(
                [
                    *ptr.add(chunk[4]),
                    *ptr.add(chunk[5]),
                    *ptr.add(chunk[6]),
                    *ptr.add(chunk[7]),
                ]
                .as_ptr(),
            );
            let v2 = vld1q_f32(
                [
                    *ptr.add(chunk[8]),
                    *ptr.add(chunk[9]),
                    *ptr.add(chunk[10]),
                    *ptr.add(chunk[11]),
                ]
                .as_ptr(),
            );
            let v3 = vld1q_f32(
                [
                    *ptr.add(chunk[12]),
                    *ptr.add(chunk[13]),
                    *ptr.add(chunk[14]),
                    *ptr.add(chunk[15]),
                ]
                .as_ptr(),
            );

            acc0 = vaddq_f32(acc0, v0);
            acc1 = vaddq_f32(acc1, v1);
            acc2 = vaddq_f32(acc2, v2);
            acc3 = vaddq_f32(acc3, v3);
        }

        let total = vaddq_f32(vaddq_f32(acc0, acc1), vaddq_f32(acc2, acc3));
        std::hint::black_box(vaddvq_f32(total));
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("random_read_simd: {:.1} GB/s", gb_per_sec);
}

// ── AVX2 sequential read (8-wide, 4 accumulators = 32 f32 per iteration) ─────

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn seq_read_avx2(data: &[f32]) {
    use core::arch::x86_64::*;

    let (ticks, _) = profiling::timed("seq_read_avx2", || {
        let mut acc0 = _mm256_setzero_ps();
        let mut acc1 = _mm256_setzero_ps();
        let mut acc2 = _mm256_setzero_ps();
        let mut acc3 = _mm256_setzero_ps();

        for chunk in data.chunks_exact(32) {
            let ptr = chunk.as_ptr();
            let v0 = _mm256_loadu_ps(ptr);
            let v1 = _mm256_loadu_ps(ptr.add(8));
            let v2 = _mm256_loadu_ps(ptr.add(16));
            let v3 = _mm256_loadu_ps(ptr.add(24));

            acc0 = _mm256_add_ps(acc0, v0);
            acc1 = _mm256_add_ps(acc1, v1);
            acc2 = _mm256_add_ps(acc2, v2);
            acc3 = _mm256_add_ps(acc3, v3);
        }

        let total = _mm256_add_ps(
            _mm256_add_ps(acc0, acc1),
            _mm256_add_ps(acc2, acc3),
        );
        let mut vals = [0.0f32; 8];
        _mm256_storeu_ps(vals.as_mut_ptr(), total);
        let sum: f32 = vals.iter().sum();
        let remainder: f32 = data.chunks_exact(32).remainder().iter().sum();
        std::hint::black_box(sum + remainder);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("seq_read_avx2: {:.1} GB/s", gb_per_sec);
}

// ── AVX2 random read (8-wide gather with _mm256_i32gather_ps) ────────────────
//
// Uses true hardware gather: one instruction fetches 8 non-contiguous f32 values.
// N = 64 MB elements = 67M, fits in i32 (max 2.1B), so cast from usize is safe.

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn random_read_avx2(data: &[f32]) {
    use core::arch::x86_64::*;

    let mut indices: Vec<usize> = (0..N).collect();
    shuffle(&mut indices);

    let (ticks, _) = profiling::timed("random_read_avx2", || {
        let ptr = data.as_ptr();
        let mut acc0 = _mm256_setzero_ps();
        let mut acc1 = _mm256_setzero_ps();
        let mut acc2 = _mm256_setzero_ps();
        let mut acc3 = _mm256_setzero_ps();

        // 4 gather operations per loop iteration = 32 random reads
        for chunk in indices.chunks_exact(32) {
            // _mm256_set_epi32(e7, e6, e5, e4, e3, e2, e1, e0): e0 → lane 0
            let vi0 = _mm256_set_epi32(
                chunk[7] as i32, chunk[6] as i32, chunk[5] as i32, chunk[4] as i32,
                chunk[3] as i32, chunk[2] as i32, chunk[1] as i32, chunk[0] as i32,
            );
            let vi1 = _mm256_set_epi32(
                chunk[15] as i32, chunk[14] as i32, chunk[13] as i32, chunk[12] as i32,
                chunk[11] as i32, chunk[10] as i32, chunk[9] as i32, chunk[8] as i32,
            );
            let vi2 = _mm256_set_epi32(
                chunk[23] as i32, chunk[22] as i32, chunk[21] as i32, chunk[20] as i32,
                chunk[19] as i32, chunk[18] as i32, chunk[17] as i32, chunk[16] as i32,
            );
            let vi3 = _mm256_set_epi32(
                chunk[31] as i32, chunk[30] as i32, chunk[29] as i32, chunk[28] as i32,
                chunk[27] as i32, chunk[26] as i32, chunk[25] as i32, chunk[24] as i32,
            );

            // scale=4: ptr + index*4 = ptr[index] for f32
            let v0 = _mm256_i32gather_ps(ptr, vi0, 4);
            let v1 = _mm256_i32gather_ps(ptr, vi1, 4);
            let v2 = _mm256_i32gather_ps(ptr, vi2, 4);
            let v3 = _mm256_i32gather_ps(ptr, vi3, 4);

            acc0 = _mm256_add_ps(acc0, v0);
            acc1 = _mm256_add_ps(acc1, v1);
            acc2 = _mm256_add_ps(acc2, v2);
            acc3 = _mm256_add_ps(acc3, v3);
        }

        let total = _mm256_add_ps(
            _mm256_add_ps(acc0, acc1),
            _mm256_add_ps(acc2, acc3),
        );
        let mut vals = [0.0f32; 8];
        _mm256_storeu_ps(vals.as_mut_ptr(), total);
        std::hint::black_box(vals.iter().sum::<f32>());
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("random_read_avx2: {:.1} GB/s", gb_per_sec);
}

// ── Dispatchers (NEON on aarch64, AVX2 when available on x86_64, scalar otherwise) ──

pub(crate) fn seq_read_vector(data: &[f32]) {
    #[cfg(target_arch = "aarch64")]
    return seq_read_simd(data);

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { seq_read_avx2(data) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] seq_read_vector: AVX2 not detected");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] seq_read_vector: no SIMD for this architecture");
        seq_read(data);
    }
}

pub(crate) fn random_read_vector(data: &[f32]) {
    #[cfg(target_arch = "aarch64")]
    return random_read_simd(data);

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        return unsafe { random_read_avx2(data) };
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        eprintln!("[SCALAR FALLBACK] random_read_vector: AVX2 not detected");
        #[cfg(not(target_arch = "x86_64"))]
        eprintln!("[SCALAR FALLBACK] random_read_vector: no SIMD for this architecture");
        random_read(data);
    }
}

// ── Scalar baselines ──────────────────────────────────────────────────────────

pub(crate) fn seq_read(data: &[f32]) {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut sum = 0.0f32;
        for &x in data {
            sum += x;
        }
        std::hint::black_box(sum);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("seq_read: {:.1} GB/s", gb_per_sec);
}

pub(crate) fn random_read(data: &[f32]) {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut sum = 0.0f32;
        let mut indices: Vec<usize> = (0..N).collect();
        shuffle(&mut indices);
        for i in 0..N {
            sum += data[indices[i]];
        }
        std::hint::black_box(sum);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("random_read: {:.1} GB/s", gb_per_sec);
}

pub(crate) fn seq_write() {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut output = vec![0.0f32; N];
        for i in 0..N {
            output[i] = i as f32;
        }
        std::hint::black_box(output);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("seq_write: {:.1} GB/s", gb_per_sec);
}

pub(crate) fn random_write() {
    let (ticks, _) = profiling::timed("seq_read", || {
        let mut output = vec![0.0f32; N];
        let mut indices: Vec<usize> = (0..N).collect();
        shuffle(&mut indices);
        for i in 0..N {
            output[indices[i]] = i as f32;
        }
        std::hint::black_box(output);
    });

    let gb_per_sec = count_gb_per_sec(ticks, None);
    println!("random_write: {:.1} GB/s", gb_per_sec);
}
