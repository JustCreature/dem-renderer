
const N: usize = 64 * 1024 * 1024;

fn shuffle(indices: &mut Vec<usize>) {
    let mut rng = 12345u64;  // seed
    for i in (1..indices.len()).rev() {
        rng = rng.wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
        let j = (rng >> 33) as usize % (i + 1);
        indices.swap(i, j);
    }
}

fn counter_frequency() -> f64 {
    let t0 = profiling::now();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let t1 = profiling::now();
    (t1 - t0) as f64 / 0.1
}

fn count_gb_per_sec(ticks: u64) -> f64 {
    let freq = counter_frequency();
    let seconds = ticks as f64 / freq;
    let bytes = N * std::mem::size_of::<f32>();
    let gb_per_sec = bytes as f64 / seconds / 1_000_000_000.0;
    gb_per_sec
}

#[cfg(target_arch = "aarch64")]
fn seq_read_simd(data: &[f32]) {
    use core::arch::aarch64::*;

    let t_wall = std::time::Instant::now();
    let ticks = profiling::timed("seq_read", || {

        unsafe {
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
        }


    });
    let wall_secs = t_wall.elapsed().as_secs_f64();
    println!("wall clock: {:.4} seconds", wall_secs);

    let gb_per_sec = count_gb_per_sec(ticks);
    println!("seq_read_simd: {:.1} GB/s", gb_per_sec);
}

fn random_read_simd(data: &[f32]) {
      use core::arch::aarch64::*;

      let mut indices: Vec<usize> = (0..N).collect();
        shuffle(&mut indices);

      let t_wall = std::time::Instant::now();
      let ticks = profiling::timed("random_read_simd", || {
          unsafe {
              let ptr = data.as_ptr();
              let mut acc0 = vdupq_n_f32(0.0);
              let mut acc1 = vdupq_n_f32(0.0);
              let mut acc2 = vdupq_n_f32(0.0);
              let mut acc3 = vdupq_n_f32(0.0);

              for chunk in indices.chunks_exact(16) {
                  let v0 = vld1q_f32([*ptr.add(chunk[0]),  *ptr.add(chunk[1]),  *ptr.add(chunk[2]),  *ptr.add(chunk[3])].as_ptr());
                  let v1 = vld1q_f32([*ptr.add(chunk[4]),  *ptr.add(chunk[5]),  *ptr.add(chunk[6]),  *ptr.add(chunk[7])].as_ptr());
                  let v2 = vld1q_f32([*ptr.add(chunk[8]),  *ptr.add(chunk[9]),  *ptr.add(chunk[10]), *ptr.add(chunk[11])].as_ptr());
                  let v3 = vld1q_f32([*ptr.add(chunk[12]), *ptr.add(chunk[13]), *ptr.add(chunk[14]), *ptr.add(chunk[15])].as_ptr());

                  acc0 = vaddq_f32(acc0, v0);
                  acc1 = vaddq_f32(acc1, v1);
                  acc2 = vaddq_f32(acc2, v2);
                  acc3 = vaddq_f32(acc3, v3);
              }

              let total = vaddq_f32(vaddq_f32(acc0, acc1), vaddq_f32(acc2, acc3));
              std::hint::black_box(vaddvq_f32(total));
          }
      });

      let wall_secs = t_wall.elapsed().as_secs_f64();                                                                                                            
      println!("wall clock: {:.4} seconds", wall_secs);                                                                                                          
      let gb_per_sec = count_gb_per_sec(ticks);                                                                                                                  
      println!("random_read_simd: {:.1} GB/s", gb_per_sec);                                                                                                      
  } 

fn seq_read(data: &[f32]) {
    let t_wall = std::time::Instant::now();
    let ticks = profiling::timed("seq_read", || {
        let mut sum = 0.0f32;
        for &x in data {
            sum += x;
        }
        std::hint::black_box(sum);
    });
    let wall_secs = t_wall.elapsed().as_secs_f64();
    println!("wall clock: {:.4} seconds", wall_secs);

    let gb_per_sec = count_gb_per_sec(ticks);
    println!("seq_read: {:.1} GB/s", gb_per_sec);
}

fn random_read(data: &[f32]) {
    let t_wall = std::time::Instant::now();
    let ticks = profiling::timed("seq_read", || {
        let mut sum = 0.0f32;
        let mut indices: Vec<usize> = (0..N).collect();
        shuffle(&mut indices);
        for i in 0..N {
            sum += data[indices[i]];
        }
        std::hint::black_box(sum);
    });
    let wall_secs = t_wall.elapsed().as_secs_f64();
    println!("wall clock: {:.4} seconds", wall_secs);

    let gb_per_sec = count_gb_per_sec(ticks);
    println!("random_read: {:.1} GB/s", gb_per_sec);
}


fn seq_write() {
    let t_wall = std::time::Instant::now();
    let ticks = profiling::timed("seq_read", || {
        let mut output = vec![0.0f32; N];
        for i in 0..N {
            output[i] = i as f32;
        }
        std::hint::black_box(output);
    });
    let wall_secs = t_wall.elapsed().as_secs_f64();
    println!("wall clock: {:.4} seconds", wall_secs);

    let gb_per_sec = count_gb_per_sec(ticks);
    println!("seq_write: {:.1} GB/s", gb_per_sec);
}

fn random_write() {
    let t_wall = std::time::Instant::now();
    let ticks = profiling::timed("seq_read", || {
        let mut output = vec![0.0f32; N];
        let mut indices: Vec<usize> = (0..N).collect();
        shuffle(&mut indices);
        for i in 0..N {
            output[indices[i]] = i as f32;
        }
        std::hint::black_box(output);
    });
    let wall_secs = t_wall.elapsed().as_secs_f64();
    println!("wall clock: {:.4} seconds", wall_secs);

    let gb_per_sec = count_gb_per_sec(ticks);
    println!("random_write: {:.1} GB/s", gb_per_sec);
}

fn main() {
    println!("dem_renderer");
    let data: Vec<f32> = (0..N).map(|i| i as f32).collect();

    seq_read_simd(&data);
    println!("--------");
    seq_read(&data);
    println!("--------");
    random_read_simd(&data);
    println!("--------");
    random_read(&data);
    println!("--------");
    seq_write();
    println!("--------");
    random_write();
    println!("--------");
    
}
