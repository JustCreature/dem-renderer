# Phase 0 — Main Session Log
## Session: lesson_01.1 | Date: 2026-03-21

---

## Session Overview

This session covered all of Phase 0: workspace setup, profiling harness, baseline bandwidth benchmarks, SIMD implementation, and a wide range of hardware/Rust fundamentals that arose naturally from the work.

---

## Topics Covered (in order)

### 1. Rust Installation
- Installed via `curl https://sh.rustup.rs | sh`
- Installed nightly toolchain alongside stable: `rustup toolchain install nightly`
- Platform: Apple M4 Max, macOS Darwin 25.3.0, AArch64

### 2. Cargo Workspace Scaffolding
- Created root `Cargo.toml` with `[workspace]` + `[package]`
- `cargo new --lib` for 5 crates: `dem_io`, `terrain`, `render_cpu`, `render_gpu`, `profiling`
- Verified with `cargo build --workspace`

### 3. AArch64 Explained
- ARM 64-bit ISA. Apple Silicon is AArch64.
- Different from x86-64: different SIMD (NEON vs AVX2), different cycle counter, different profiling tools

### 4. SIMD Explained
- Single Instruction Multiple Data. One instruction, multiple values in parallel.
- NEON: 128-bit registers, 4 f32 lanes per register
- Divergence: lanes executing different branches → inactive lanes waste compute

### 5. Profiling Harness (`crates/profiling`)
- `cntvct_el0` not available on stable Rust → used inline assembly `mrs {}, cntvct_el0`
- Built `now() -> u64` and `timed<F: FnMut()>(label, f) -> u64`
- Tests: `now_is_monotonically_increasing` (with 1ms sleep), `timed_calls_the_closure`

### 6. Rust Fundamentals Encountered
- **`#[cfg(...)]`**: conditional compilation, code literally absent when condition false
- **Closures**: `|args| body`, captures surrounding scope, `Fn` vs `FnMut` vs `FnOnce`
- **`FnMut` + `mut f`**: calling `FnMut` requires mutable borrow of closure → needs `mut f: F`
- **Ownership/Borrowing**: move, `&T`, `&mut T`, `mut T` — four ways to pass to functions
- **Types**: `[T;N]` (stack, compile-time size), `Vec<T>` (heap, runtime size), `&[T]` (slice ref, no ownership)
- **Macros**: `!` suffix means macro. `vec!`, `println!`, `asm!` — generate code at compile time
- **`black_box`**: prevents optimizer from deleting "unused" work
- **`wrapping_mul`**: intentional overflow arithmetic for LCG

### 7. Counter Frequency Bug
- `sysctl hw.tbfrequency` returns 24,000,000 (24 MHz)
- Actual `cntvct_el0` on M4 Max ticks at ~1 GHz (verified by comparing with `Instant::now()`)
- Fix: measure frequency empirically via `sleep(100ms)` and count ticks

### 8. Baseline Bandwidth Benchmark

**Patterns implemented:**
- `seq_read`: iterate `&data` in order, sum with `black_box`
- `random_read`: Fisher-Yates shuffled indices, follow permutation
- `seq_write`: write to output Vec in order
- `random_write`: write to shuffled indices

**Fisher-Yates LCG shuffle:**
- Knuth LCG constants, `wrapping_mul`, `>> 33` for high bits
- `indices.swap(i, j)` — visits every element exactly once

**Results (256 MB / 64M f32):**
```
seq_read scalar:    5.7–6.7 GB/s
seq_write scalar:   8.0–13.6 GB/s
random_read scalar: 0.6 GB/s
random_write:       0.5 GB/s
```
Sequential/random ratio: 11–16×

**Initial stride-based "random" was only 1.3× slower** — prefetcher detected the fixed stride pattern.

### 9. Hardware Concepts Explored

**Prefetcher**: detects access patterns, prefetches ahead. Defeated by true random. Stride patterns may still be tracked.

**Load buffer**: on-die SRAM tracking in-flight loads. Limited slots. When full of 100ns DRAM-latency requests, CPU stalls.

**MLP (Memory Level Parallelism)**: overlapping cache misses. ~15 misses × 4 bytes / 100ns ≈ 0.6 GB/s. Matches measurement.

**Store buffer**: writes are fire-and-forget. Why seq_write > seq_read scalar.

**Out-of-order execution + ROB**: CPU runs independent instructions in parallel. ROB tracks all in-flight work.

**SRAM vs DRAM**: SRAM = 6 transistors/bit, fast (1–10 ns), expensive. DRAM = 1T+cap/bit, slow (60–100 ns), cheap.

**Die**: physical silicon rectangle. On-die = nanometers. Off-die = millimeters.

**Execution units**: ALU, FPU, SIMD, Load, Store, Branch. Each type has limited units. Port pressure = operations queuing for same unit.

**P-cores vs E-cores**: 10 P-cores (complex, fast) + 4 E-cores (simple, efficient) on M4 Max.

**Unified memory**: CPU + GPU share 546 GB/s LPDDR5. No PCIe upload needed.

### 10. LLVM and Auto-Vectorization
- Rust uses LLVM backend. rustc → LLVM IR → machine code.
- LLVM auto-vectorizer: exists but unreliable for FP reductions (FP add not associative)
- Verified via assembly: `seq_read` loop emitted scalar `fadd s0` not NEON `vaddq`
- Check: `cargo rustc --release -- --emit=asm`, look for `ld1`/`st1`

### 11. NEON SIMD seq_read_simd

Key insight: 4 independent accumulators break the serial dependency chain.

Result: **21.8–37 GB/s** — 6.5× improvement over scalar.
- ~4× from SIMD (4 floats/instruction)
- ~1.6× from breaking dependency chain via multiple accumulators

### 12. NEON SIMD random_read_simd

Implemented by manually gathering 4 random floats into stack arrays → vld1q_f32.

Result: **1.4 GB/s** vs 0.6 GB/s scalar — 2.3× improvement.
Reason: increased MLP (16 loads/iteration vs 1). Not SIMD arithmetic.
NEON has no hardware gather instruction (unlike AVX2's `_mm256_i32gather_ps`).

---

## Final Benchmark Summary

| Pattern | GB/s | % of 546 GB/s |
|---|---|---|
| seq_read scalar | 5.7–6.7 | 1.1% |
| seq_read SIMD | 21.8–37 | 4–7% |
| seq_write scalar | 8.0–13.6 | 1.5–2.5% |
| random_read scalar | 0.6 | 0.1% |
| random_read SIMD | 1.4 | 0.25% |
| random_write scalar | 0.5 | 0.09% |
| **M4 Max peak** | **546** | 100% |

---

## Hardware Tangents (Came Up Organically)

- **Unified memory vs discrete GPU**: Apple M4 Max 546 GB/s unified vs RTX 4090 900 GB/s VRAM + 60 GB/s PCIe. Unified wins for CPU↔GPU transfer-heavy workloads. Discrete wins for sustained GPU compute with data resident in VRAM.
- **Gaming**: RTX 2080 likely beats M4 Max in gaming due to Windows + CUDA + game-specific driver tuning, despite hardware disadvantage.
- **Stack overflow demo**: `let _arr = [0f32; 16*1024*1024]` — compiler optimizes away unused array. Needs `black_box` to actually crash.

---

## Code Written

### `crates/profiling/src/lib.rs`
- `now() -> u64` — reads `cntvct_el0` via inline assembly (AArch64) or `_rdtsc` (x86)
- `timed<F: FnMut()>(label, f) -> u64` — wraps closure, prints CSV, returns ticks
- Tests: monotonicity with sleep, closure execution

### `src/main.rs`
- `counter_frequency() -> f64` — empirical tick frequency measurement
- `count_gb_per_sec(ticks) -> f64` — bandwidth calculation
- `seq_read`, `random_read`, `seq_write`, `random_write` — scalar benchmarks
- `seq_read_simd` — 4-accumulator NEON implementation, `chunks_exact(16)`
- `random_read_simd` — gather via stack arrays, NEON accumulation
- `shuffle(&mut Vec<usize>)` — Fisher-Yates with Knuth LCG

---

## Open Items / Notes for Phase 1

- `random_write` and `seq_write` don't use SIMD yet — not worth it until Phase 2
- `count_gb_per_sec` calls `counter_frequency()` on every invocation (sleeps 100ms each time) — should cache the result
- All timed labels are still "seq_read" in some functions — fix before Phase 1 reporting
- Phase 1 goal: tiled memory layout to keep working set in L1/L2, enabling the sequential bandwidth we measured here for what would otherwise be random access patterns during normal computation
