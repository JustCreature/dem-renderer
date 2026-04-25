# Phase 0 — Short Report (Comprehensive Reference)

---

## 1. Platform

**Apple M4 Max — AArch64 (ARM 64-bit)**

AArch64 is ARM's 64-bit instruction set architecture. Apple Silicon (M1–M4) is AArch64. It is *not* x86-64 (Intel/AMD). The differences that matter for this project:

| | AArch64 | x86-64 |
|---|---|---|
| SIMD | NEON (128-bit, 4×f32) | AVX2 (256-bit, 8×f32) / AVX-512 |
| Cycle counter | `cntvct_el0` via `mrs` instruction | `rdtsc` |
| Profiling tools | Instruments (Apple), `perf` (Linux) | `perf`, VTune |
| Cache line | 64 bytes | 64 bytes |

**M4 Max specs that shaped every benchmark result:**

- 10 P-cores (Performance) + 4 E-cores (Efficiency)
- 192 KB L1D per P-core, 16 MB L2 shared
- 546 GB/s unified LPDDR5 memory bandwidth
- 36 GB unified memory (CPU and GPU share it — no PCIe copy)

---

## 2. SIMD and NEON

**SIMD = Single Instruction, Multiple Data.** One instruction operates on multiple values packed into a wide register. On NEON a 128-bit register holds 4 × f32. One `vaddq_f32` adds 4 pairs at once.

**NEON registers**: 32 × 128-bit registers (`v0`–`v31`). Each can be viewed as 4 lanes of f32, 2 lanes of f64, 8 lanes of i16, etc.

**Lane**: one slot in a SIMD register. `v0.4s` has lanes 0–3, each 32 bits.

**Divergence**: SIMD executes the same instruction on all lanes simultaneously. If different lanes need different branches, the hardware must mask off inactive lanes — wasted compute. Branch-free (branchless) code avoids this. Inner loops in this project are branchless for this reason.

**Key NEON intrinsics used:**

```rust
use core::arch::aarch64::*;

vdupq_n_f32(0.0)          // broadcast scalar → [0.0, 0.0, 0.0, 0.0]
vld1q_f32(ptr)            // load 4 adjacent f32 from memory
vaddq_f32(a, b)           // [a0+b0, a1+b1, a2+b2, a3+b3]
vaddvq_f32(v)             // horizontal sum: v0+v1+v2+v3 → scalar
```

All SIMD intrinsics are `unsafe`. Safety requirement: pointer passed to `vld1q_f32` must be valid and point to at least 4 × 4 = 16 bytes of readable memory.

---

## 3. Cargo Workspace

**Workspace** = one `Cargo.toml` at the root that owns multiple crates. All share one `target/` directory and one lock file. Dependencies between crates are explicit.

```
dem_renderer/
├── Cargo.toml          # [workspace] members = [...]
├── src/main.rs         # binary
└── crates/
    ├── dem_io/
    ├── terrain/
    ├── render_cpu/
    ├── render_gpu/
    └── profiling/      # ← built in Phase 0
```

```sh
cargo build --workspace         # build everything
cargo build --release           # optimized build (required for benchmarks)
cargo test -p profiling         # test one crate
```

`opt-level = 3`, `lto = "thin"`, `codegen-units = 1` in `[profile.release]` — enables LLVM to inline across crates and apply full optimizations.

---

## 4. Profiling Harness (`crates/profiling`)

### Hardware cycle counter

**AArch64**: `cntvct_el0` — the virtual timer count register. Readable from userspace. Monotonically increasing.

`core::arch::aarch64::__cntvct_el0()` is nightly-only. Use inline assembly on stable:

```rust
#[cfg(target_arch = "aarch64")]
pub fn now() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val);
    }
    val
}
```

`mrs` = "Move to Register from System register". `out(reg) val` tells the assembler to write the result into a general-purpose register and bind it to `val`.

**x86-64** equivalent: `_rdtsc()` reads the time-stamp counter.

### Counter frequency gotcha

`sysctl hw.tbfrequency` reports **24,000,000** (24 MHz). The actual `cntvct_el0` on M4 Max ticks at **~1 GHz**. If you use 24 MHz as your frequency, every bandwidth calculation is ~42× too low.

**Always measure empirically:**

```rust
fn counter_frequency() -> f64 {
    let t0 = profiling::now();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let t1 = profiling::now();
    (t1 - t0) as f64 / 0.1   // ticks per second
}
```

Cross-check: measure the same 100ms interval with `std::time::Instant`. If tick count / Instant duration ≠ ~1 GHz, the formula is wrong.

### GB/s formula

```rust
let seconds = ticks as f64 / freq;
let bytes = N * std::mem::size_of::<f32>();  // N * 4
let gb_per_sec = bytes as f64 / seconds / 1_000_000_000.0;
```

`size_of::<f32>()` = 4. For N = 64M elements: `64 * 1024 * 1024 * 4` = 256 MB touched per benchmark pass.

---

## 5. Rust Fundamentals

### `#[cfg(...)]` — conditional compilation

```rust
#[cfg(target_arch = "aarch64")]
fn seq_read_simd(data: &[f32]) { ... }
```

The function body is **completely absent** from the compiled binary on non-AArch64 targets. Not dead code eliminated — literally never parsed into IR. Used for platform-specific intrinsics, inline assembly, and architecture-specific benchmarks.

### Ownership and borrowing — four calling patterns

```rust
fn takes_owned(v: Vec<f32>)         // move: caller loses v
fn reads_only(v: &[f32])            // immutable borrow: caller keeps v, no mutation
fn reads_writes(v: &mut Vec<f32>)   // mutable borrow: caller keeps v, can mutate
fn takes_owned_mut(mut v: Vec<f32>) // move + locally rebindable (rare; same as move)
```

The borrow checker enforces: you can have **one `&mut T`** OR **any number of `&T`** at a time, never both. This prevents data races at compile time.

`&[f32]` (slice reference) is the idiomatic parameter type for read-only access to contiguous data — works whether the caller has a `Vec<f32>`, `[f32; N]`, or another slice.

### Types: array, Vec, slice

```
[T; N]   — stack-allocated, fixed size known at compile time.
             Pushed onto the stack frame. Freed when scope ends.
             Stack is ~8 MB. [f32; 16*1024*1024] (~64 MB) = stack overflow.

Vec<T>   — heap-allocated, runtime size. Stack holds ptr + len + cap (24 bytes).
             Actual data on heap. Freed when Vec is dropped.

&[T]     — slice reference = ptr + len (16 bytes). No ownership, no allocation.
             Read-only window into an [T;N] or Vec<T>.
             Universal parameter type — works for both.
```

### Closures

```rust
let threshold = 1.0f32;
let above = |x: f32| x > threshold;   // captures threshold by copy
```

A closure is an anonymous function that captures variables from the surrounding scope.

- `Fn` — captures by shared reference, can be called repeatedly, doesn't mutate captures
- `FnMut` — captures by mutable reference, can be called repeatedly, may mutate captures
- `FnOnce` — consumes captures, can be called only once

`timed` uses `FnMut` because callers may pass closures that mutate local variables (e.g., `sum += x`). The `mut f: F` binding is required to allow calling `f()` — calling `FnMut` requires `&mut f`.

### Macros

The `!` suffix marks a macro: `vec!`, `println!`, `asm!`, `black_box`. Macros expand at compile time into regular code. They can do things functions cannot — variable argument counts, accept non-expression syntax, generate code based on tokens.

### `black_box`

```rust
std::hint::black_box(sum);
```

Tells the compiler "assume this value is used externally." Without it, the optimizer sees that `sum` is never returned or printed and **deletes the entire computation**. With it, the benchmark measures real work. Not a real function call — emits a compiler fence in LLVM IR.

### Numeric overflow and `wrapping_mul`

Rust integer overflow **panics in debug mode**, **wraps in release** (on most targets — but this is not guaranteed). For intentional modular arithmetic (e.g., LCG), use `wrapping_mul` explicitly:

```rust
rng = rng.wrapping_mul(6_364_136_223_846_793_005)
         .wrapping_add(1_442_695_040_888_963_407);
```

This is always defined behavior regardless of build profile.

---

## 6. Bandwidth Benchmark Setup

### Data

```rust
const N: usize = 64 * 1024 * 1024;  // 64M elements
let data: Vec<f32> = (0..N).map(|i| i as f32).collect();
// 64M * 4 bytes = 256 MB
```

256 MB >> L2 (16 MB), so every access is a guaranteed DRAM fetch. This isolates memory bandwidth from cache effects.

### Fisher-Yates shuffle with LCG

A true random permutation defeats the hardware prefetcher. Fisher-Yates produces a uniform permutation in O(N):

```rust
fn shuffle(indices: &mut Vec<usize>) {
    let mut rng = 12345u64;                              // seed
    for i in (1..indices.len()).rev() {                  // i = N-1 down to 1
        rng = rng.wrapping_mul(6_364_136_223_846_793_005)
                 .wrapping_add(1_442_695_040_888_963_407);
        let j = (rng >> 33) as usize % (i + 1);         // uniform in [0, i]
        indices.swap(i, j);                              // swap element at i with element at j
    }
}
```

**LCG** (Linear Congruential Generator): `x = a*x + c mod 2^64`. Knuth constants give good statistical properties. `>> 33` uses the high bits (better randomness than low bits in LCGs).

**Why Fisher-Yates and not a simple random index?** The permutation visits each element exactly once. A simple `index = rng % N` has collisions — some elements accessed multiple times, others never. Fisher-Yates gives true 1-to-1 random mapping.

**Stride pattern gotcha**: First attempt used `indices[i] = (i * stride) % N` — a fixed stride. Result was only 1.3× slower than sequential. The **hardware prefetcher detected the stride pattern** and prefetched ahead. True random (Fisher-Yates) is 11–16× slower.

---

## 7. Memory Hierarchy (M4 Max)

| Level | Size | Latency | Type | Location |
|---|---|---|---|---|
| Registers | ~KB | 0 cycles | flip-flops | on-die |
| L1D (per P-core) | 192 KB | ~4 cycles | SRAM | on-die |
| L2 (shared) | 16 MB | ~10 cycles | SRAM | on-die |
| SLC / L3 | ~? | ~40 cycles | SRAM | on-die |
| DRAM | 36 GB | ~100 ns | LPDDR5 | off-die |

**Die** = the physical silicon rectangle. "On-die" = nanometers from the CPU core. "Off-die" = millimeters away (DRAM chips, separate package). Distance = latency.

**SRAM** (Static RAM): 6 transistors per bit. Fast (1–10 ns), doesn't need refresh, expensive — so caches are small.

**DRAM** (Dynamic RAM): 1 transistor + 1 capacitor per bit. Slow (60–100 ns), must be periodically refreshed, cheap — so main memory is large.

**Cache line** = 64 bytes = 16 × f32. The minimum unit transferred between DRAM and cache. When you access one f32 that's not cached, the CPU fetches the surrounding 64 bytes. Sequential access reuses those 15 other floats "for free."

---

## 8. Key Hardware Concepts

### Prefetcher

A hardware unit that watches memory access patterns and issues loads in advance. Detects:
- Sequential access: trivially detected, aggressively prefetched
- Fixed stride (e.g., every 256 bytes): often detected, varies by microarchitecture
- True random: cannot be predicted, no prefetching

Result: stride access was only 1.3× slower than sequential in our benchmark — prefetcher was tracking it. Fisher-Yates random: 11–16× slower — prefetcher completely defeated.

### Load buffer

On-die SRAM structure tracking all in-flight load operations. Limited slots (tens to hundreds). When full, the CPU cannot issue new loads and **stalls**.

With random access, every load is a cache miss → ~100 ns DRAM round-trip. Load buffer has ~15 slots. Throughput limit: `15 loads × 4 bytes / 100 ns` ≈ **0.6 GB/s**. This exactly matches our measured random_read scalar result.

### MLP (Memory Level Parallelism)

Overlapping multiple cache misses simultaneously. The load buffer is the hardware mechanism; MLP is the effect. More independent loads issued before the first one returns → more misses in flight → higher sustained bandwidth.

`random_read_simd` issues 16 loads per loop iteration (vs 1 in scalar). More misses in flight. Result: **1.4 GB/s vs 0.6 GB/s** — 2.3× improvement, with zero SIMD arithmetic benefit. The gain is purely from increased MLP.

### Store buffer

Writes are **fire-and-forget**. The CPU places a write into the store buffer and immediately continues executing. The store buffer drains to cache/DRAM asynchronously. The CPU never stalls waiting for a write to complete.

Why `seq_write` (8–13.6 GB/s) > `seq_read` (5.7–6.7 GB/s) scalar: writes don't stall the pipeline waiting for data to come back from memory. The load for a read must complete before the loaded value can be used in a computation — this creates a dependency chain.

### Out-of-order execution and ROB

The CPU maintains a **Reorder Buffer (ROB)** — a circular buffer of all in-flight instructions, ~600+ entries on M4 P-cores. Instructions are fetched in-order, executed out-of-order (whenever operands are ready and execution units are free), and retire in-order. This lets independent instructions overlap.

For a serial FP reduction `acc += data[i]`, each iteration depends on the previous result → serial dependency chain → out-of-order execution cannot help.

### Execution units and port pressure

Each core has multiple execution units: ALU (integer), FPU (floating-point scalar), SIMD, Load, Store, Branch. Multiple units of each type exist but are limited. **Port pressure** = when more instructions need the same unit type than there are units available, instructions queue up.

### P-cores vs E-cores

M4 Max has 10 P-cores (Performance) + 4 E-cores (Efficiency). P-cores: wide out-of-order pipelines, large ROB, high clock speed, high power. E-cores: narrow in-order or simple out-of-order, low power. The OS schedules compute-intensive work on P-cores. All our benchmarks ran on P-cores.

### Unified Memory (Apple Silicon)

CPU and GPU share the same physical DRAM at 546 GB/s. No PCIe bus between them. No "upload to GPU" step. The GPU can read any CPU-allocated buffer directly.

Compare with discrete GPU (RTX 4090): 900 GB/s GDDR6X bandwidth **inside** the GPU, but only 32–64 GB/s PCIe bandwidth to CPU. If the GPU needs to read data that's in CPU memory, it's bottlenecked by PCIe. Unified memory eliminates this entirely for CPU↔GPU transfer-heavy workloads.

For Phase 5 (GPU renderer), our heightmap lives in CPU memory and the GPU reads it directly over the 546 GB/s unified bus.

---

## 9. LLVM and Auto-Vectorization

Rust compiles to LLVM IR, which compiles to machine code. LLVM has an auto-vectorizer that tries to convert scalar loops into SIMD.

**Why it didn't vectorize `seq_read`**: The accumulation `sum += data[i]` creates a serial dependency: each iteration must wait for the previous FP add to complete before adding the next value. LLVM cannot reorder this because **floating-point addition is not associative** — `(a + b) + c ≠ a + (b + c)` in IEEE 754 due to rounding. Without `-ffast-math` (or Rust's equivalent), LLVM cannot reorder FP operations.

**Verify in assembly:**

```sh
cargo rustc --release -- --emit=asm
# look in target/release/deps/*.s
```

Scalar: `fadd s0, s0, s1` — scalar f32 registers (`s0`)
NEON: `fadd v0.4s, v0.4s, v1.4s` — 4-lane f32 vectors (`v0.4s`)

Our `seq_read` emitted `fadd s0` — confirmed scalar, no auto-vectorization.

---

## 10. NEON SIMD Implementation

### Why 4 accumulators?

With one accumulator:
```
acc += chunk[0]; acc += chunk[1]; acc += chunk[2]; ...
```
Each iteration depends on the previous → serial chain → 1 FP add per cycle.

With 4 independent accumulators, the 4 dependency chains are independent. The out-of-order engine can pipeline them, issuing multiple FP adds per cycle:
```
acc0 += v0; acc1 += v1; acc2 += v2; acc3 += v3;  // 4 independent adds
```

Result: ~1.6× extra speedup on top of the 4× SIMD width benefit.

### seq_read_simd — full pattern

```rust
#[cfg(target_arch = "aarch64")]
fn seq_read_simd(data: &[f32]) {
    use core::arch::aarch64::*;
    unsafe {
        let mut acc0 = vdupq_n_f32(0.0);
        let mut acc1 = vdupq_n_f32(0.0);
        let mut acc2 = vdupq_n_f32(0.0);
        let mut acc3 = vdupq_n_f32(0.0);

        for chunk in data.chunks_exact(16) {     // 16 f32 = 4 vectors of 4
            let ptr = chunk.as_ptr();
            acc0 = vaddq_f32(acc0, vld1q_f32(ptr));         // floats 0–3
            acc1 = vaddq_f32(acc1, vld1q_f32(ptr.add(4)));  // floats 4–7
            acc2 = vaddq_f32(acc2, vld1q_f32(ptr.add(8)));  // floats 8–11
            acc3 = vaddq_f32(acc3, vld1q_f32(ptr.add(12))); // floats 12–15
        }

        // reduce 4 accumulators to 1 vector, then to scalar
        let sum01 = vaddq_f32(acc0, acc1);
        let sum23 = vaddq_f32(acc2, acc3);
        let total = vaddq_f32(sum01, sum23);
        let sum = vaddvq_f32(total);               // horizontal sum

        // handle remaining elements not divisible by 16
        let remainder: f32 = data.chunks_exact(16).remainder().iter().sum();
        std::hint::black_box(sum + remainder);
    }
}
```

`ptr.add(4)` advances by 4 × 4 = 16 bytes (raw pointer arithmetic in units of the pointed-to type).

### random_read_simd — gather pattern

NEON has no hardware gather instruction (unlike AVX2's `_mm256_i32gather_ps`). Manual gather:

```rust
// indices[0..4] are random positions into data
let v0 = vld1q_f32(
    [*ptr.add(chunk[0]), *ptr.add(chunk[1]),
     *ptr.add(chunk[2]), *ptr.add(chunk[3])].as_ptr()
);
```

Creates a 4-element stack array, then loads it with `vld1q_f32`. Each `*ptr.add(idx)` is a separate random DRAM load. The benefit is not SIMD arithmetic — it's that 16 random loads are issued per loop iteration instead of 1, increasing MLP.

---

## 11. Benchmark Results (M4 Max, 256 MB / 64M f32)

| Pattern | GB/s | vs peak | Bottleneck |
|---|---|---|---|
| seq_read scalar | 5.7–6.7 | 1.1% | scalar serial dependency chain |
| seq_read SIMD | 21.8–37 | 4–7% | single sequential DRAM stream |
| seq_write scalar | 8.0–13.6 | 1.5–2.5% | store buffer throughput |
| random_read scalar | 0.6 | 0.1% | DRAM latency, ~15 slots × 4B / 100ns |
| random_read SIMD | 1.4 | 0.25% | DRAM latency (16 loads/iter vs 1) |
| random_write scalar | 0.5 | 0.09% | DRAM latency |
| **M4 Max peak** | **546** | 100% | — |

**Sequential/random ratio: 11–16×.** Every tiled memory layout decision in Phase 1+ is justified by this number.

**SIMD speedups broken down:**
- `seq_read SIMD`: 6.5× over scalar = ~4× from SIMD width + ~1.6× from independent accumulator chains
- `random_read SIMD`: 2.3× over scalar = purely from increased MLP (more concurrent loads), not arithmetic

**Single thread reached ~4–7% of peak 546 GB/s.** Approaching peak requires rayon + multiple memory streams + prefetcher cooperation.

---

## 12. Open Issues (carry to Phase 1)

- `count_gb_per_sec` calls `counter_frequency()` on every invocation, sleeping 100ms each time. Should be cached.
- All `profiling::timed(label, ...)` calls in `random_read`, `seq_write`, `random_write` still use `"seq_read"` as the label string — fix before Phase 1 reporting.
- `random_write` and `seq_write` have no SIMD variants — not needed until Phase 2+.
- Phase 1 goal: tiled memory layout so a working set fits in L1/L2, converting what would be random DRAM accesses during computation into sequential L1/L2 accesses — the 11–16× ratio is the win we're chasing.

---

## 13. Key Takeaways

1. **Layout first, SIMD second.** `vld1q_f32` requires 4 adjacent floats. SoA enables this; AoS breaks it.
2. **Identify the bottleneck before optimizing.** SIMD helps compute/bandwidth-bound work. For latency-bound random access, SIMD only helps by increasing MLP — not arithmetic.
3. **Sequential/random = 11–16×.** This is the hardware cost of a cache miss. It drives every layout decision.
4. **Prefetchers are pattern-detectors.** A fixed stride may look "random" in code but is transparent to hardware. Only a true random permutation defeats it.
5. **Never hardcode counter frequency.** Measure it empirically every time. Reported values (`hw.tbfrequency`) are wrong on Apple Silicon.
6. **Check assembly.** LLVM auto-vectorization is unreliable for FP reductions. `cargo rustc --release -- --emit=asm`. Look for `v0.4s` (NEON) vs `s0` (scalar).
7. **`--release` is mandatory for benchmarks.** Debug mode is 10–50× slower. Use `black_box` to prevent dead-code elimination.
8. **Single thread ≈ 4–7% of peak bandwidth.** Full utilization requires multiple threads (`rayon`) and multiple independent memory streams.
