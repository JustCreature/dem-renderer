#[allow(dead_code)]
static FREQ: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
pub(crate) const N: usize = 64 * 1024 * 1024;

#[allow(dead_code)]
pub(crate) fn shuffle(indices: &mut Vec<usize>) {
    let mut rng = 12345u64; // seed
    for i in (1..indices.len()).rev() {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let j = (rng >> 33) as usize % (i + 1);
        indices.swap(i, j);
    }
}

#[allow(dead_code)]
pub(crate) fn counter_frequency() -> f64 {
    *FREQ.get_or_init(|| {
        let t0 = profiling::now();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let t1 = profiling::now();
        (t1 - t0) as f64 / 0.1
    })
}

#[allow(dead_code)]
pub(crate) fn count_gb_per_sec(ticks: u64, bytes: Option<usize>) -> f64 {
    let freq = counter_frequency();
    let seconds = ticks as f64 / freq;
    let bytes = bytes.unwrap_or(N * std::mem::size_of::<f32>());
    let gb_per_sec = bytes as f64 / seconds / 1_000_000_000.0;
    gb_per_sec
}
