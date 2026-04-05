use dem_io::Heightmap;
use terrain::NormalMap;

static FREQ: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
pub(crate) const N: usize = 64 * 1024 * 1024;

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

pub(crate) fn counter_frequency() -> f64 {
    *FREQ.get_or_init(|| {
        let t0 = profiling::now();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let t1 = profiling::now();
        (t1 - t0) as f64 / 0.1
    })
}

pub(crate) fn count_gb_per_sec(ticks: u64, bytes: Option<usize>) -> f64 {
    let freq = counter_frequency();
    let seconds = ticks as f64 / freq;
    let bytes = bytes.unwrap_or(N * std::mem::size_of::<f32>());
    let gb_per_sec = bytes as f64 / seconds / 1_000_000_000.0;
    gb_per_sec
}

pub(crate) fn create_cropped_image(heightmap: &Heightmap, normal_map: &NormalMap) {
    let r_start = 1000;
    let c_start = 1500;
    let height = 500;
    let width = 500;

    let nz = &normal_map.nz;
    let pixels: Vec<u8> = (r_start..r_start + height)
        .flat_map(|r| {
            (c_start..c_start + width).map(move |c| (nz[r * heightmap.cols + c] * 255.0) as u8)
        })
        .collect();

    image::GrayImage::from_raw(width as u32, height as u32, pixels)
        .unwrap()
        .save("artifacts/normals_crop.png")
        .unwrap();
}

pub(crate) fn create_rgb_png(heightmap: &Heightmap, normal_map: &NormalMap) {
    let pixels: Vec<u8> = (0..heightmap.rows * heightmap.cols)
        .flat_map(|i| {
            let nx = ((normal_map.nx[i] + 1.0) * 0.5 * 255.0) as u8;
            let ny = ((normal_map.ny[i] + 1.0) * 0.5 * 255.0) as u8;
            let nz = (normal_map.nz[i] * 255.0) as u8;
            [nx, ny, nz]
        })
        .collect();

    image::RgbImage::from_raw(heightmap.cols as u32, heightmap.rows as u32, pixels)
        .unwrap()
        .save("artifacts/normals_rgb.png")
        .unwrap();
}
