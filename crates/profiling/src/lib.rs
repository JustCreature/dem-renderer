#[cfg(target_arch = "aarch64")]
pub fn now() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val);
    }
    val
}

#[cfg(target_arch = "x86_64")]
pub fn now() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

pub fn timed<R, F: FnMut() -> R>(label: &str, mut f: F) -> (u64, R) {
    let t_wall = std::time::Instant::now();

    let t0 = now();
    let result = f();
    let t1 = now();
    let elapsed = t1 - t0;
    println!("{},{}", label, elapsed);

    let wall_secs = t_wall.elapsed().as_secs_f64();
    println!("wall clock: {:.4} seconds", wall_secs);

    (elapsed, result)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn now_is_monotonically_increasing() {
        let mut samples = [0u64; 5];

        for i in 0..5 {
            samples[i] = now();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        for i in 1..5 {
            assert!(samples[i] > samples[i - 1]);
        }
    }

    #[test]
    fn timed_calls_the_closure() {
        let mut called = false;
        timed("test", || {
            called = true;
        });
        assert!(called)
    }
}
