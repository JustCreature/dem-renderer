# Phase 0 — Complete Learning Report
## Everything We Covered, From First Principles

This document is written for someone who is learning. Every term is defined. Every concept is explained from the ground up. If you read this after a break, you should be able to reconstruct the full mental model from scratch.

---

# Part 1: Setting Up the Environment

## 1.1 What is Rust?

Rust is a systems programming language. "Systems" means it compiles to native machine code (like C or C++), gives you direct control over memory, and has no garbage collector. It runs as fast as C, but enforces memory safety rules at compile time — so entire classes of bugs (use-after-free, data races, null pointer dereferences) are impossible if your code compiles.

## 1.2 What is Cargo?

Cargo is Rust's build system and package manager. It handles:
- Compiling your code (`cargo build`)
- Running it (`cargo run`)
- Running tests (`cargo test`)
- Running benchmarks (`cargo bench`)
- Managing dependencies (downloading libraries)

Everything in this project is done through Cargo. You never call `rustc` directly.

## 1.3 What is rustup?

`rustup` is the Rust toolchain manager. It installs and manages versions of Rust itself. One machine can have multiple Rust versions installed simultaneously (stable, nightly, etc.).

Install command (macOS):
```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

This installs: `rustc` (the compiler), `cargo` (the build tool), `rustfmt` (formatter), `clippy` (linter).

**Stable vs Nightly**: Stable Rust releases every 6 weeks and is production-ready. Nightly Rust builds every night from the latest source — it has experimental features not yet stabilized. This project uses stable for most things but will need nightly for some SIMD features later.

Install nightly alongside stable:
```sh
rustup toolchain install nightly
```

## 1.4 What is a Cargo Workspace?

A workspace is a collection of Rust packages (called "crates") that:
- Share one `target/` directory (compiled output)
- Share one `Cargo.lock` (pinned dependency versions)
- Can depend on each other via path dependencies

Why use a workspace? So you can benchmark each crate in isolation (`cargo bench -p terrain`) without rebuilding everything.

The root `Cargo.toml` declares the workspace:
```toml
[workspace]
members = [
    "crates/dem_io",
    "crates/terrain",
    "crates/render_cpu",
    "crates/render_gpu",
    "crates/profiling",
]
resolver = "2"

[package]
name = "dem_renderer"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "dem_renderer"
path = "src/main.rs"
```

Each library crate under `crates/` has its own `Cargo.toml` and `src/lib.rs`. Create them with:
```sh
cargo new --lib crates/dem_io
```

Path dependencies between crates:
```toml
# terrain/Cargo.toml
[dependencies]
dem_io = { path = "../dem_io" }
```

**Build modes**: `cargo build` = debug (no optimizations, bounds checks everywhere, slow). `cargo build --release` = release (fully optimized, fast). **Always use `--release` when measuring performance.** Debug mode can be 10–50× slower.

---

# Part 2: Architecture — What Hardware Are We Running On?

## 2.1 What is AArch64?

AArch64 is the 64-bit execution state of the ARM architecture (introduced with ARMv8). Also called ARM64. Your Apple Silicon Mac (M4 Max) uses AArch64.

The name breaks down:
- **ARM**: the processor architecture family
- **64**: 64-bit registers and address space (can address up to 16 exabytes of memory)

This is a **completely different instruction set** from x86-64 (Intel/AMD). The two architectures are not binary-compatible — a program compiled for one won't run on the other.

Key differences relevant to this project:

| Thing | x86-64 (Intel/AMD) | AArch64 (Apple Silicon) |
|---|---|---|
| SIMD instruction set | AVX2 (256-bit) / AVX-512 (512-bit) | NEON (128-bit) |
| SIMD lanes (f32) | 8 (AVX2) | 4 (NEON) |
| Cycle counter instruction | `rdtsc` | `mrs cntvct_el0` |
| L1D cache (perf cores) | 32–48 KB | 192 KB (M4 Max) |
| Profiling tools | `perf stat`, `perf record` | Instruments |

## 2.2 What is a CPU Die?

The die is the actual piece of silicon — the physical chip. Manufacturing takes a large silicon wafer (~300mm diameter) and etches billions of transistors onto it using photolithography (essentially very precise UV light printing). The wafer is then cut into individual rectangles — each rectangle is one die.

The die gets placed into a ceramic or plastic **package** (the physical chip with metal pins you'd hold in your hand). The die itself is the tiny silicon square inside.

**On-die** = physically etched on the same piece of silicon. Electrons travel nanometers. Latency is measured in picoseconds to nanoseconds.

**Off-die** = on separate chips, connected by PCIe, solder bumps, or copper traces. Electrons travel millimeters. Latency jumps significantly.

On Apple Silicon M4 Max: CPU cores, GPU cores, Neural Engine, L1 cache, L2 cache, SLC (L3), and the memory controller are all on one die. The LPDDR5 DRAM (your 36 GB) is on separate dies, physically stacked very close in the same package but still off-die.

## 2.3 Performance Cores vs Efficiency Cores

The M4 Max has 14 cores: **10 Performance (P-cores) + 4 Efficiency (E-cores)**.

**Performance cores**: designed for maximum single-thread speed. They achieve this by being physically large and power-hungry:
- Deep **out-of-order execution** (explained in Part 4)
- Large ROB (Reorder Buffer, 600+ entries on M4 P-cores)
- Aggressive branch predictor
- **Wide superscalar**: can issue 8+ instructions per clock cycle
- Large L1D cache: 192 KB per P-core
- High clock speed (~3.5+ GHz)
- High power consumption (~5–10 W per core under load)

**Efficiency cores**: designed for low power consumption. Physically much smaller:
- Shallower, simpler pipeline
- Smaller ROB
- Narrower issue width (fewer instructions per cycle)
- Smaller caches
- Low clock speed
- Very low power (~0.5 W per core)

**Why it matters for benchmarking**: macOS automatically schedules compute-heavy work to P-cores. All your benchmark numbers reflect P-core performance. The hardware concepts we discuss (ROB size, store buffer depth, branch predictor behavior) refer to the P-core microarchitecture. E-cores are architecturally much simpler.

In Phase 6, when you scale from 1 to N threads, threads 1–10 land on P-cores. Thread 11 spills to an E-core and you'll see a cliff in your scaling curve — a direct measurement of the P/E performance gap.

## 2.4 Unified Memory Architecture

### Conventional systems (Intel/AMD + discrete GPU)

```
CPU ──→ CPU RAM (DDR5, ~50 GB/s bandwidth)
                    │
                    ↕ PCIe bus (60 GB/s, ~microsecond latency)
                    │
GPU ──→ VRAM (GDDR6, ~900 GB/s bandwidth on RTX 4090)
```

The CPU and GPU have **separate, independent memory pools**. Passing data from CPU to GPU requires an explicit copy across the PCIe bus:

1. CPU loads data into RAM (~50 GB/s)
2. CPU copies data to GPU over PCIe (~60 GB/s) ← bottleneck
3. GPU processes data in VRAM (~900 GB/s)
4. GPU copies result back over PCIe (~60 GB/s) ← bottleneck again

For your heightmap renderer: 64 MB heightmap / 60 GB/s = ~1 ms just for the PCIe transfer. This happens every frame if you're not careful.

### Apple Silicon unified memory

```
CPU cores ──┐
GPU cores ──┼──→ shared LPDDR5 (546 GB/s on M4 Max)
Neural Engine──┘
```

All compute units share the **same physical DRAM pool**. Steps 2 and 4 above simply don't exist. The GPU reads data the CPU just wrote, at full 546 GB/s bandwidth, zero transfer latency.

**The tradeoff**: CPU and GPU compete for the same 546 GB/s. An RTX 4090's 900 GB/s is **exclusive** to the GPU — the CPU doesn't touch it. For sustained GPU-only compute (games, large model training), a discrete GPU can win because its VRAM bandwidth is unshared.

**Your chip (M4 Max) specs**:
- 546 GB/s unified memory bandwidth
- 36 GB unified memory (CPU and GPU see all 36 GB)
- 10+4 cores, 32 GPU cores, Neural Engine

**Practical implication for Phase 5 (GPU renderer)**: no upload step needed. The GPU reads the heightmap directly from the Vec<f32> you allocated on the CPU side.

---

# Part 3: Rust Language Fundamentals

## 3.1 Ownership

In Rust, every value has exactly one **owner**. When the owner goes out of scope, the value is automatically dropped (memory freed). No garbage collector, no manual `free()`.

```rust
{
    let v = vec![1.0f32, 2.0, 3.0];  // v owns the Vec and its heap memory
    // use v...
}  // v goes out of scope → Vec is dropped, heap memory freed automatically
```

**Moving**: passing a value to a function transfers ownership:
```rust
let data = vec![1.0f32, 2.0, 3.0];
consume(data);                        // ownership moves into consume()
println!("{}", data[0]);              // ERROR: data was moved, it's gone
```

For a 256 MB Vec, you obviously don't want to move it into every function. That's what borrowing is for.

## 3.2 Borrowing

Borrowing lets you lend access to a value without transferring ownership.

### Four ways to pass a value to a function:

```rust
// 1. Move ownership — caller loses the value after this call
fn move_it(data: Vec<f32>) { ... }
move_it(data);
// data is gone here

// 2. Immutable borrow — read-only, caller keeps ownership
fn read_it(data: &[f32]) { ... }
read_it(&data);
// data still accessible here, unchanged

// 3. Mutable borrow — read/write, caller keeps ownership
fn modify_it(data: &mut Vec<f32>) { ... }
modify_it(&mut data);
// data still accessible here, possibly modified

// 4. Move + mutable local binding — moves ownership but allows mutation inside
fn move_and_modify(mut data: Vec<f32>) -> Vec<f32> {
    data.push(4.0);
    data  // return ownership back to caller
}
```

**The `mut` in case 4** is NOT a fourth category — it's identical to case 1 (ownership moves) but the local binding inside the function is mutable. You could also write `let mut data = data;` inside the function body for the same effect. It's just syntactic convenience.

**The borrowing rule**: **either one mutable reference (`&mut T`) OR any number of immutable references (`&T`) — never both simultaneously.** This is enforced at compile time by the borrow checker. If `&` allowed mutation, two functions holding `&data` at the same time could corrupt each other's view. By making `&` read-only, Rust guarantees: if you hold a `&T`, the data won't change under you.

### The `mut f: F` requirement for FnMut

When `timed` takes `F: FnMut()`, calling `f()` internally does `FnMut::call_mut(&mut f, ())` — a mutable borrow of `f` itself. This requires `f` to be a mutable binding:

```rust
// ERROR: f is immutable binding, can't get &mut f
pub fn timed<F: FnMut()>(label: &str, f: F) {
    f();  // error: cannot borrow `f` as mutable
}

// CORRECT: mut f allows &mut f for calling FnMut
pub fn timed<F: FnMut()>(label: &str, mut f: F) {
    f();  // works
}
```

The `mut` says "I need mutable access to this local binding." It doesn't affect what the caller passes.

## 3.3 Types: Array, Vec, Slice

Three different ways to store a sequence of values. Understanding when to use each is fundamental.

### Array `[T; N]`

Size is part of the type. `[f32; 4]` and `[f32; 8]` are **different types**.

```rust
let arr: [f32; 4] = [1.0, 2.0, 3.0, 4.0];
```

Lives on the **stack** (explained in section 3.4). The compiler knows the exact size at compile time and reserves that many bytes in the stack frame. Zero heap allocation.

**Use when**: size is known at compile time and small. SIMD register-width buffers, fixed-size tiles, lookup tables. In this project: processing 4 floats in a NEON register.

### Vec `Vec<T>`

Dynamically sized. Grows and shrinks at runtime.

Internally three fields (all on the stack):
```
ptr  → [ heap memory: f32, f32, f32, ... ]
len    (how many elements currently stored)
cap    (how many elements fit before reallocation)
```

The data buffer itself is on the **heap**.

**Use when**: size is unknown at compile time, or large. Your 256 MB heightmap, normal maps, output framebuffer — anything that can't fit on the stack.

Methods: `push`, `pop`, `resize`, `reserve`, plus everything a slice has.

### Slice `&[T]`

A reference to a contiguous sequence of `T` — doesn't own the data, just points at it. Always behind a reference (`&[T]` or `&mut [T]`).

Internally two words (on the stack):
```
ptr  → somewhere in memory (into a Vec, array, or any contiguous block)
len    (how many elements)
```

```rust
let v: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0];
let s: &[f32] = &v[1..4];  // view of elements 1,2,3 — no allocation, no copy
```

**Use when**: passing data to a function that only needs to read it. The idiomatic parameter type for "give me a sequence to look at."

`&Vec<f32>` vs `&[f32]`: both compile. `&[f32]` is more general — it accepts Vec, arrays, and sub-slices via automatic coercion. Always prefer `&[f32]` for function parameters.

**Methods on `&[T]`** (read-only): `len()`, `iter()`, `contains()`, `windows()`, `chunks()`, `chunks_exact()`.
**Methods on `&mut [T]`** (read/write): `sort()`, `swap()`, `chunks_mut()`, plus all the above.

`sort()` requires `&mut [T]`, not `&[T]` — sorting rearranges elements, which is mutation.

### Summary table

| | `[T; N]` | `Vec<T>` | `&[T]` |
|---|---|---|---|
| Size known at | compile time | runtime | runtime |
| Memory | stack | heap | points elsewhere |
| Owns data | yes | yes | no |
| Can grow | no | yes | no |
| Use for | small fixed buffers | large dynamic data | function parameters |

Both `[T; N]` and `Vec<T>` coerce to `&[T]` automatically when you write `&arr` or `&vec`.

## 3.4 Stack vs Heap

Two memory regions every program uses, with fundamentally different characteristics.

### The Stack

A fixed-size block of memory per thread (default: ~8 MB on macOS). Works like a stack of plates — push on top, pop from top.

The CPU has a register called the **stack pointer** that tracks the current top. When you call a function:
1. The compiler calculates how many bytes that function's local variables need
2. The stack pointer is decremented by that amount (stack grows downward)
3. That region becomes the function's **stack frame**
4. When the function returns, the stack pointer is incremented back — variables gone instantly

**Cost**: one pointer decrement to allocate an entire frame. Essentially free.

**Constraint**: size must be known at compile time. `[f32; 4]` = 16 bytes → compiler can reserve exactly 16 bytes. `Vec<f32>` with runtime-determined size → cannot be on the stack. Well, the Vec struct itself (24 bytes: ptr + len + cap) can be on the stack, but the data buffer must be on the heap.

**Performance**: stack data is almost certainly in L1 cache because the stack pointer barely moves. Local variables, loop counters, SIMD register-width arrays — all hot in cache.

**Stack overflow**: the stack has a fixed size limit. Trying to allocate too much causes a crash:
```rust
fn will_crash() {
    let arr = [0f32; 16 * 1024 * 1024];  // 64 MB — exceeds ~8 MB stack limit
    std::hint::black_box(&arr);          // black_box forces compiler to actually allocate it
    // crashes before reaching this line
}
```
Note: without `black_box`, the compiler sees `arr` is unused and removes it — no crash. `black_box` forces the allocation to actually exist.

### The Heap

A large memory pool managed by an allocator (Rust uses jemalloc or system allocator). You request a block at runtime:
- Allocator searches for a free block of the right size
- Returns a pointer to it
- You use it
- When the owner drops, the allocator reclaims it

**Cost**: allocation requires allocator overhead — bookkeeping, potentially a system call (`mmap`/`sbrk`) if more memory is needed. Not free, but usually fast enough to not matter except in very hot loops.

**Benefit**: any size, determined at runtime. Your 256 MB heightmap can't possibly go on the stack.

**Performance**: the data lives at an arbitrary heap address. First access may be a cache miss. Large allocations span many pages — touching them sequentially trains the prefetcher. Touching them randomly is expensive.

## 3.5 Closures

A closure is an inline anonymous function that can **capture variables from the surrounding scope**.

Regular named function:
```rust
fn add(x: i32, y: i32) -> i32 { x + y }
```

Same logic as a closure:
```rust
let add = |x: i32, y: i32| x + y;
```

The `|...|` bars hold parameters. The body follows.

**The key difference from functions — capturing**:
```rust
let threshold = 10;
let is_big = |x: i32| x > threshold;  // captures `threshold` from outer scope
```
A regular function can only see its own parameters. A closure can reach out and grab variables from the scope where it was written.

**Three closure traits**:

| Trait | Meaning | When to use |
|---|---|---|
| `Fn` | Called multiple times, no mutation of captured state | Simple read-only closures |
| `FnMut` | Called multiple times, can mutate captured state | Closures that update accumulators |
| `FnOnce` | Called exactly once, can consume captured state | Closures that move captured values |

They form a hierarchy: every `Fn` is also `FnMut`, every `FnMut` is also `FnOnce`.

For `timed`, `FnMut` is correct because real benchmarking closures mutate accumulators:
```rust
timed("seq_read", || {
    let mut sum = 0.0f32;
    for &x in data {
        sum += x;      // mutates sum — requires FnMut
    }
    std::hint::black_box(sum);
});
```

## 3.6 Conditional Compilation: `#[cfg(...)]`

`#[cfg(condition)]` tells the compiler: include this item **only if this condition is true**. If false, the item is completely excluded — it doesn't exist in the binary.

```rust
#[cfg(target_arch = "aarch64")]
pub fn now() -> u64 {
    // ARM-specific implementation
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) val); }
    val
}

#[cfg(target_arch = "x86_64")]
pub fn now() -> u64 {
    // x86-specific implementation
    unsafe { core::arch::x86_64::_rdtsc() }
}
```

On your Apple Silicon Mac, only the first function exists. The second is completely absent from the binary.

Common conditions:
```rust
#[cfg(target_arch = "aarch64")]   // CPU architecture
#[cfg(target_os = "macos")]        // operating system
#[cfg(debug_assertions)]           // true in debug, false in --release
#[cfg(feature = "simd")]           // user-defined cargo feature flag
#[cfg(test)]                       // only during cargo test
```

**`#[cfg(test)]`** is specifically for code that should only exist when running tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;  // bring parent module's items into scope

    #[test]
    fn my_test() {
        assert!(something_true());
    }
}
```

## 3.7 Macros

In Rust, anything called with `!` is a **macro**, not a regular function:
```rust
vec![1, 2, 3]        // macro
println!("{}", x)    // macro
assert!(x > 0)       // macro
asm!("mrs ...")       // macro
```

A macro generates code at compile time. `vec![1, 2, 3]` expands to roughly:
```rust
{
    let mut v = Vec::new();
    v.push(1);
    v.push(2);
    v.push(3);
    v
}
```

This can't be a regular function because regular functions take a fixed number of arguments. Macros can accept variable argument lists and generate arbitrary code. The `!` is the visual signal: "this bends the normal language rules."

## 3.8 `std::hint::black_box`

Prevents the compiler from optimizing away code whose result appears unused.

**The problem**: the compiler is very smart. If you write:
```rust
let mut sum = 0.0f32;
for &x in &data { sum += x; }
// sum is never used after this
```
The compiler concludes: "this computation has no observable effect" and **deletes the entire loop**. Your benchmark times an empty loop.

**The solution**:
```rust
let mut sum = 0.0f32;
for &x in &data { sum += x; }
std::hint::black_box(sum);  // "pretend sum escapes to the outside world"
```

`black_box` compiles to nothing at runtime — it's purely a signal to the optimizer: "you cannot prove this value is unused, keep the computation."

**Important**: `black_box` must be called on the **final result** after the loop, not inside the loop. Putting it inside the loop would create a memory roundtrip on every iteration.

## 3.9 Numeric Literals and Type Suffixes

```rust
0u64          // value 0, type u64
6_700_417usize  // value 6700417 (underscores are visual separators), type usize
1.0f32        // value 1.0, type f32
```

**`usize`**: unsigned integer whose size matches the platform pointer width. On 64-bit systems: 64-bit. Used for array indices and memory sizes — you cannot index a slice with `u32` or `i32` directly in Rust.

**`wrapping_mul`**: multiplication that intentionally wraps on overflow instead of panicking. Normal integer multiplication in debug mode panics on overflow (a safety feature). For LCG random number generation, overflow is expected and desired — `wrapping_mul` says "this overflow is intentional."

**`>>`**: bitwise right shift. `x >> 33` shifts all bits of x right by 33 positions. Equivalent to dividing by 2³³ and discarding the remainder. Used in the LCG to extract high bits, which have better statistical randomness properties than low bits.

**`%`**: modulo operator. `a % b` = remainder after dividing a by b. `7 % 5 = 2`. Used to wrap large index values back into the valid range `0..N`.

---

# Part 4: Hardware Deep Dive

## 4.1 SIMD (Single Instruction, Multiple Data)

A normal CPU instruction operates on one value at a time:
```
ADD r1, r2  →  r1 = r1 + r2   (one addition)
```

A SIMD instruction operates on multiple values packed into one wide register, simultaneously:
```
VADD v1, v2  →  [a0+b0, a1+b1, a2+b2, a3+b3]   (four additions, one instruction)
```

The CPU does all four additions in **one clock cycle** in parallel — not by running four separate cores, but by having physically wider arithmetic circuits inside one core.

**Register widths by architecture:**
- NEON (AArch64/your chip): 128-bit registers → 4 × f32 per register
- AVX2 (x86): 256-bit registers → 8 × f32 per register
- AVX-512 (x86): 512-bit registers → 16 × f32 per register

**Why data layout must match SIMD**: `vld1q_f32(ptr)` loads 16 consecutive bytes starting at `ptr` into one NEON register. This only works if 4 floats are adjacent in memory:

```
GOOD (SoA — struct of arrays):
nx: [nx0][nx1][nx2][nx3][nx4]...   ← vld1q_f32 loads nx0,nx1,nx2,nx3 in one instruction

BAD (AoS — array of structs):
[nx0][ny0][nz0][nx1][ny1][nz1]...  ← nx values are 12 bytes apart, can't use vld1q_f32
```

This is the fundamental reason the plan uses SoA layout for normals — it makes SIMD trivial.

## 4.2 Lanes and Divergence

A **lane** is one slot inside a SIMD register.

```
float32x4_t register:
[ 1.0 | 2.0 | 3.0 | 4.0 ]
 lane0  lane1  lane2  lane3
```

When you execute one SIMD instruction, all 4 lanes execute the same operation simultaneously.

**Divergence** is when different lanes need to take different code paths. SIMD has no per-lane branching — the hardware executes all lanes or none. To handle divergence, you use a **mask**: a bitmask where 0 = "ignore this lane's result."

Example in raymarching: 4 rays packed into one NEON register. Ray 0 hits terrain at step 5. Rays 1, 2, 3 haven't hit yet.

```
active mask before step 6:  [ 0 | 1 | 1 | 1 ]  ← lane 0 done, masked out
continue stepping:          [ _ | step | step | step ]  ← lane 0 executes but result discarded
```

**The waste**: at 25% divergence (1 of 4 lanes inactive), you're running at 75% efficiency. Wasted SIMD work is called divergence loss. Minimized by grouping rays likely to terminate at similar depths (screen-space tiling in Phase 4).

On a GPU, the same concept applies but at 32-lane (warp) scale — this is why GPU divergence is even more important to avoid.

## 4.3 NEON Intrinsics

NEON is ARM's SIMD instruction set. In Rust, accessed via `core::arch::aarch64::*`.

```rust
use core::arch::aarch64::*;

// All NEON intrinsics require unsafe {}
unsafe {
    // Create a vector with same value in all 4 lanes
    let zeros = vdupq_n_f32(0.0);       // [0.0, 0.0, 0.0, 0.0]

    // Load 4 consecutive f32 from a raw pointer
    let v = vld1q_f32(ptr);              // reads 16 bytes starting at ptr

    // Add two float32x4 vectors lane-by-lane
    let sum = vaddq_f32(a, b);           // [a0+b0, a1+b1, a2+b2, a3+b3]

    // Horizontal reduce: collapse all 4 lanes to one scalar
    let total: f32 = vaddvq_f32(v);      // a0+a1+a2+a3

    // Advance a raw pointer by N elements (not bytes)
    let ptr2 = ptr.add(4);               // 4 f32 = 16 bytes forward
}
```

Naming convention: `v` prefix = vector, `add/mul/max` = operation, `q` = quadword (128-bit), `_f32` = element type.

**Why `unsafe`**: NEON intrinsics are raw hardware instructions. Rust can't verify that the pointer you pass to `vld1q_f32` is valid, aligned, and points to initialized memory. You take responsibility. Always document the safety invariant.

**Reading the hardware counter via inline assembly**:
```rust
let val: u64;
unsafe {
    core::arch::asm!("mrs {}, cntvct_el0", out(reg) val);
}
```
`mrs` = Move from system Register to general-purpose register. `cntvct_el0` = virtual timer counter register, readable from userspace (EL0 = Exception Level 0 = userspace).

Why assembly instead of `core::arch::aarch64::__cntvct_el0()`? That intrinsic is nightly-only. Inline `asm!` is stable since Rust 1.59.

## 4.4 The Memory Hierarchy

Every level is faster and smaller than the one below it.

```
Register file         ~64 × 64-bit       0 cycles         on-die, wired to execution units
L1 data cache        192 KB (M4 P-core)  ~4 cycles        on-die SRAM
L2 cache (shared)    16 MB               ~10 cycles       on-die SRAM
SLC (L3)             varies              ~40 cycles       on-die SRAM
LPDDR5 DRAM          36 GB               ~100+ ns         off-die

```

**SRAM** (Static RAM): what caches are made of. 6 transistors per bit — no refresh needed ("static"), very fast (1–10 ns), but physically large and expensive per bit. You can't make 36 GB of SRAM — it would cover an entire building.

**DRAM** (Dynamic RAM): what main memory is made of. 1 transistor + 1 capacitor per bit — cheaper and denser, but the capacitor leaks and must be **refreshed** thousands of times per second ("dynamic"), slower (60–100 ns).

**The cliff**: L1 hit = 4 cycles. DRAM miss = 100+ ns = 300–400 cycles. That's a 75–100× latency difference. Every tiling and layout decision in this project exists to exploit this cliff — keep working data in L1/L2 and avoid DRAM.

## 4.5 The Prefetcher

A hardware circuit inside the CPU that watches your memory access patterns and **fetches cache lines before you ask for them**.

**The problem without prefetching**:
```
iteration 0: request data[0] → stall 100 ns waiting for DRAM
iteration 1: request data[1] → stall 100 ns
iteration 2: request data[2] → stall 100 ns
```
CPU is idle ~99% of the time.

**How the prefetcher helps**:
1. CPU requests `data[0]` — cache miss, DRAM fetch initiated
2. Prefetcher observes: "address X, then X+4, then X+8 — stride 4 pattern"
3. Prefetcher immediately issues requests for X+64, X+128, X+192... (full cache lines ahead)
4. CPU processes `data[0]`, `data[1]`... by the time it needs them, they're already in L1

**What the prefetcher can detect**:
- Sequential access (stride 1) — trivially detected
- Fixed stride patterns — can be detected if consistent
- Multiple simultaneous streams — can track several independent streams

**What defeats the prefetcher**:
- True random access (no detectable pattern) — every access is a cold DRAM miss

**The stride experiment discovery**: initial "random" access using a fixed stride (`(i * 6_700_417) % N`) was only **1.3× slower** than sequential — the prefetcher detected the stride pattern. With true Fisher-Yates shuffle: **11–16× slower**. The difference reveals the prefetcher's contribution.

## 4.6 Out-of-Order Execution and the ROB

A naive CPU executes instructions strictly in order:
```
load r0, mem[A]   ← wait 100 ns for DRAM
add  r1, r0, 1   ← can't start
load r2, mem[B]   ← can't start
add  r3, r2, 1   ← can't start
```
Terrible utilization.

A modern **out-of-order CPU** looks ahead in the instruction stream, finds independent instructions, and runs them in parallel regardless of program order. Results are committed in order (so the program behaves correctly), but execution overlaps:

```
load r0, mem[A]   ← issue to memory system
load r2, mem[B]   ← also issue immediately — independent of first load!
add  r1, r0, 1   ← wait only until load A arrives (not until B arrives)
add  r3, r2, 1   ← wait only until load B arrives
```

**Reorder Buffer (ROB)**: the structure that tracks all in-flight instructions. Every instruction dispatched from the frontend gets a ROB slot. When execution completes, the result is written to the ROB slot. Instructions commit (become permanent) in program order from the head of the ROB.

- M4 Max P-core ROB: estimated 600+ entries
- Larger ROB = more instructions in-flight = more ILP exploited = faster execution

**When the ROB fills**: the frontend stalls. No more instructions can be dispatched. If the ROB is full of DRAM-latency loads (100+ ns each), the whole pipeline stalls.

## 4.7 Load Buffer, Store Buffer, and MLP

### Load Buffer

A physical on-die SRAM structure (part of the ROB) tracking all pending memory **reads**.

Each slot contains:
- Target memory address
- Destination register
- Status: pending / data arrived / ready to commit
- The data itself (once it arrives)

**Slot lifecycle**:
- L1 hit: slot occupied for ~4 cycles, then freed
- L2 hit: slot occupied for ~10 cycles
- DRAM miss: slot occupied for ~100+ ns (hundreds of cycles)

When all slots are full, the CPU cannot issue any new loads — it stalls even if there are independent loads in the instruction stream.

**Memory Level Parallelism (MLP)**: keeping multiple load buffer slots occupied with independent DRAM requests simultaneously. More concurrent DRAM requests = better DRAM bus utilization.

Example: at 100 ns DRAM latency with ~15 outstanding misses, effective bandwidth = 15 × 4 bytes / 100 ns = **0.6 GB/s**. This exactly matches the measured random_read scalar result.

### Store Buffer

A similar structure tracking pending **writes**. Key difference: the CPU does not wait for stores to reach DRAM. It writes to the store buffer and moves on immediately ("fire and forget"). The store buffer drains to cache/DRAM in the background.

This is why `seq_write` (8–13.6 GB/s) consistently outperforms `seq_read` (5.7–6.7 GB/s) in scalar code — reads stall waiting for data, writes don't.

## 4.8 Execution Units and Port Pressure

The CPU is not one monolithic unit — it's many specialized circuits, each capable of one category of operation.

```
┌─────────────────────────────────────────────────────┐
│  ALU × 4     integer arithmetic: +, -, &, |, ^, << │
│  FPU × 2     scalar floating point: fadd, fmul     │
│  SIMD × 4    NEON: vaddq_f32, vmulq_f32, vld1q_f32 │
│  Load × 2    memory reads, initiate cache lookups   │
│  Store × 2   memory writes, go to store buffer      │
│  Branch × 1  conditional jumps                      │
└─────────────────────────────────────────────────────┘
```

**Superscalar execution**: multiple units of the same type run simultaneously. If you have 4 independent NEON additions, all 4 can execute in one cycle on 4 SIMD units.

**Port pressure**: when more operations of one type exist than there are units, they queue. In Phase 2's dense normal computation, if the inner loop has more SIMD multiply-adds than there are SIMD units, the excess queue up — a bottleneck that `perf stat -e fp_ret_sse_avx_ops.all` (or Instruments equivalent) can detect.

---

# Part 5: The Profiling Harness

## 5.1 Hardware Cycle Counter

A hardware register inside the CPU that increments every clock cycle (or at a fixed frequency). Gives nanosecond-resolution timing without OS overhead.

- **x86-64**: TSC (Time Stamp Counter), read with `rdtsc`. Ticks at CPU clock frequency.
- **AArch64**: `cntvct_el0`, the ARM generic timer virtual counter. Fixed frequency (independent of CPU clock speed).

**The gotcha we discovered**: `sysctl hw.tbfrequency` on macOS returns 24,000,000 (24 MHz) but the actual `cntvct_el0` counter on M4 Max runs at ~1 GHz. They're different things. Never trust a reported frequency — always measure it empirically:

```rust
fn counter_frequency() -> f64 {
    let t0 = profiling::now();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let t1 = profiling::now();
    (t1 - t0) as f64 / 0.1   // ticks per second
}
```

**How we discovered the bug**: initial results showed 0.1 GB/s for sequential read. Cross-checked with `std::time::Instant` — wall clock showed 0.016 seconds while `cntvct_el0` implied 0.497 seconds. Ratio: ~31×. Derived actual frequency: 16M ticks / 0.016 s ≈ 1 GHz.

## 5.2 The `timed` Function

```rust
pub fn timed<F: FnMut()>(label: &str, mut f: F) -> u64 {
    let t0 = now();
    f();
    let t1 = now();
    let elapsed = t1 - t0;
    println!("{},{}", label, elapsed);  // CSV output
    elapsed
}
```

Returns elapsed ticks so the caller can compute GB/s. Prints CSV for easy parsing.

## 5.3 GB/s Calculation

```rust
let freq = counter_frequency();                        // ticks per second (~1e9)
let seconds = ticks as f64 / freq;                    // elapsed time
let bytes = N * std::mem::size_of::<f32>();           // total bytes accessed (N × 4)
let gb_per_sec = bytes as f64 / seconds / 1_000_000_000.0;
```

`std::mem::size_of::<f32>()` = 4 (bytes). Better than hardcoding.

---

# Part 6: The Bandwidth Benchmark

## 6.1 Fisher-Yates Shuffle

A true random permutation algorithm. Every possible ordering of N elements is equally likely. Visits every element exactly once (unlike sampling with replacement).

```rust
fn shuffle(indices: &mut Vec<usize>) {
    let mut rng = 12345u64;  // seed: deterministic starting value
    for i in (1..indices.len()).rev() {  // iterate from N-1 down to 1
        // LCG step: update the random state
        rng = rng.wrapping_mul(6_364_136_223_846_793_005)
                 .wrapping_add(1_442_695_040_888_963_407);
        // Extract a random index j in range 0..=i
        let j = (rng >> 33) as usize % (i + 1);
        // Swap element i with random earlier element j
        indices.swap(i, j);
    }
}
```

**LCG (Linear Congruential Generator)**: simplest pseudo-random number generator. Formula: `x_next = (x * a + c) mod m`. With carefully chosen constants (the Knuth multiplicative constants used here), produces a sequence that looks random but is deterministic given the same seed.

**Seed**: the starting value. Same seed = same sequence. Useful for reproducible experiments.

**`wrapping_mul`**: normal u64 multiplication would panic in debug mode when the result overflows 64 bits. LCG relies on overflow wrapping — the overflow is part of the algorithm. `wrapping_mul` permits intentional overflow.

**`>> 33`**: right-shift by 33 bits. The low bits of LCG output have poorer statistical properties than high bits. Taking the top 31 bits via `>> 33` gives better randomness.

**`% (i + 1)`**: constrains the random value to the range `0..=i` (valid positions in the unshuffled portion of the array).

## 6.2 Stride vs True Random

Initial attempt used a fixed stride: `(i * 6_700_417) % N`.

This visits every index exactly once (since stride and N are coprime — N is a power of 2, stride is an odd prime, so gcd = 1). But the access pattern has a mathematical regularity — it's a fixed stride of ~26 MB. The hardware prefetcher partially detected this.

**Result**: only **1.3× slower** than sequential.

With Fisher-Yates: **11–16× slower**. The difference shows the prefetcher's contribution to sequential performance.

## 6.3 Why seq_write > seq_read (Scalar)

Scalar `seq_write` (8–13.6 GB/s) consistently outperforms `seq_read` (5.7–6.7 GB/s). The store buffer is the reason:
- Reads: CPU must stall and wait for the data value to arrive before the dependent instruction can execute
- Writes: CPU queues the write in the store buffer and moves to the next instruction immediately

## 6.4 Why SIMD Doesn't Help Random Access

Random access is **latency-bound**, not **compute-bound** or **bandwidth-bound**.

The bottleneck: waiting 100 ns for each DRAM response.

SIMD arithmetic: makes computation faster (4 operations per instruction instead of 1). Does nothing for DRAM latency.

With random `SIMD`: you still have 4 independent loads to 4 random DRAM locations. Each takes 100 ns. Packing results into a NEON register and doing one `vaddq_f32` instead of 4 `fadd` doesn't save any time — the time was spent waiting for DRAM, not in arithmetic.

**However**: random_read_simd (1.4 GB/s) is 2.3× faster than random_read scalar (0.6 GB/s). The reason is **increased MLP** — issuing 16 loads per loop iteration instead of 1 fills more load buffer slots simultaneously, overlapping more DRAM round-trips. This is not SIMD arithmetic helping — it's more concurrent loads increasing utilization.

---

# Part 7: LLVM and Auto-Vectorization

## 7.1 What is LLVM?

LLVM is a compiler infrastructure framework — a collection of reusable components for building compilers. Originally "Low Level Virtual Machine" but now just "LLVM."

Rust's compilation pipeline:
```
Rust source code
      ↓
   rustc             ← Rust-specific: type checking, borrow checker, trait resolution, MIR generation
      ↓
  LLVM IR            ← platform-independent intermediate representation (like assembly for a virtual machine)
      ↓
   LLVM              ← optimization passes (vectorization, inlining, dead code elimination, loop unrolling...)
      ↓
  AArch64 assembly   ← machine code specific to your M4 Max
```

LLVM is also the backend for Clang (C/C++), Swift, Kotlin Native, and Julia. Rust gets the same optimization quality as C because they use the same LLVM optimization passes.

## 7.2 Auto-Vectorization

LLVM contains an auto-vectorizer that tries to convert scalar loops to SIMD automatically, without you writing a single intrinsic.

For your `seq_read` loop:
```rust
for &x in data { sum += x; }
```
LLVM might automatically emit `vld1q_f32` + `vaddq_f32`. This would be auto-vectorization.

**When it works**:
- Loop body is simple
- No cross-iteration dependencies
- Data is provably contiguous
- No function calls in the hot path

**Why it failed for `seq_read`**: the `sum += x` accumulation creates a serial dependency chain. Each addition writes to `sum`, which the next addition reads. Floating point addition is **not associative** — `(a+b)+c ≠ a+(b+c)` due to rounding. LLVM refuses to reorder FP operations by default because it would change the result, even subtly.

**Evidence from assembly**:
```
seq_read assembly (scalar, unrolled but not vectorized):
fadd    s0, s0, s1   ← scalar single-precision: s registers, not v registers
fadd    s0, s0, s2
fadd    s0, s0, s3
...
```
NEON vector instructions would use `v0.4s` syntax. Seeing `s0`–`s24` confirms scalar-only execution.

**How to check**:
```sh
cargo rustc --release -- --emit=asm 2>/dev/null
# then look for ld1/st1 (NEON) vs ldr/str/fadd (scalar) in the .s file
```

**Conclusion**: never rely on auto-vectorization for the hot path. Use explicit `core::arch` intrinsics so you know exactly what executes.

---

# Part 8: The SIMD seq_read_simd Implementation

## 8.1 The Dependency Chain Problem

Scalar sum accumulation has a serial dependency chain:
```asm
fadd s0, s0, s1   ← takes 3–4 cycles, writes s0
fadd s0, s0, s2   ← must wait for previous fadd to finish (reads s0)
fadd s0, s0, s3   ← must wait...
```
Each instruction depends on the result of the previous. The CPU can't overlap them. Maximum throughput: 1 addition per 3–4 cycles.

## 8.2 Breaking the Chain with Multiple Accumulators

Four independent accumulators have no dependency between them:
```asm
vaddq.f32 v0, v0, v4   ← writes v0
vaddq.f32 v1, v1, v5   ← writes v1 — independent of v0
vaddq.f32 v2, v2, v6   ← writes v2 — independent of v0, v1
vaddq.f32 v3, v3, v7   ← writes v3 — independent of v0, v1, v2
```
The out-of-order engine can issue all four simultaneously. With enough independent units, this runs at 4× the throughput of the single-accumulator version.

## 8.3 The Full Implementation

```rust
#[cfg(target_arch = "aarch64")]
fn seq_read_simd(data: &[f32]) {
    use core::arch::aarch64::*;
    unsafe {
        // 4 independent accumulators — each holds [f32; 4]
        let mut acc0 = vdupq_n_f32(0.0);
        let mut acc1 = vdupq_n_f32(0.0);
        let mut acc2 = vdupq_n_f32(0.0);
        let mut acc3 = vdupq_n_f32(0.0);

        // Process 16 floats per iteration (4 groups of 4)
        for chunk in data.chunks_exact(16) {
            let ptr = chunk.as_ptr();
            // Load 4 vectors of 4 floats each
            let v0 = vld1q_f32(ptr);          // floats 0–3
            let v1 = vld1q_f32(ptr.add(4));   // floats 4–7
            let v2 = vld1q_f32(ptr.add(8));   // floats 8–11
            let v3 = vld1q_f32(ptr.add(12));  // floats 12–15
            // Accumulate independently
            acc0 = vaddq_f32(acc0, v0);
            acc1 = vaddq_f32(acc1, v1);
            acc2 = vaddq_f32(acc2, v2);
            acc3 = vaddq_f32(acc3, v3);
        }

        // Combine 4 accumulators into 1
        let sum01 = vaddq_f32(acc0, acc1);
        let sum23 = vaddq_f32(acc2, acc3);
        let total = vaddq_f32(sum01, sum23);

        // Horizontal reduce: [a, b, c, d] → a+b+c+d
        let sum = vaddvq_f32(total);

        // Handle remainder (elements not divisible by 16)
        let remainder: f32 = data.chunks_exact(16).remainder().iter().sum();

        std::hint::black_box(sum + remainder);
    }
}
```

**`chunks_exact(16)`**: iterator over non-overlapping `&[f32]` slices of exactly 16 elements. If `data.len()` is not divisible by 16, the leftover elements are available via `.remainder()`.

**`ptr.add(4)`**: advances the pointer by 4 elements (16 bytes), not 4 bytes. Rust's pointer arithmetic is type-aware.

## 8.4 Results and Analysis

| | Scalar | SIMD | Improvement |
|---|---|---|---|
| seq_read | 5.7–6.7 GB/s | 21.8–37 GB/s | ~6.5× |

Expected improvement from SIMD alone: 4× (4 floats per instruction).
Actual: 6.5×. The extra 1.6× comes from:
- Multiple accumulators breaking the dependency chain
- Out-of-order engine overlapping independent `vaddq_f32` and `vld1q_f32` operations

Still only 4–7% of the M4 Max's 546 GB/s peak. Remaining gap:
- Single memory stream: one thread can only sustain so many in-flight prefetch requests
- To approach 546 GB/s: multiple threads (`rayon`) with multiple independent streams (Phase 2)

---

# Part 9: Final Benchmark Summary

## 9.1 All Results (256 MB / 64M f32 elements, M4 Max)

| Pattern | GB/s | Bottleneck | % of 546 GB/s peak |
|---|---|---|---|
| seq_read scalar | 5.7–6.7 | 1 load/iter, scalar loop | 1.1% |
| seq_read SIMD | 21.8–37 | single stream, 16 loads/iter | 4–7% |
| seq_write scalar | 8.0–13.6 | store buffer (fire-and-forget) | 1.5–2.5% |
| random_read scalar | 0.6 | DRAM latency, ~15 MLP | 0.1% |
| random_read SIMD | 1.4 | DRAM latency, ~30+ MLP | 0.25% |
| random_write scalar | 0.5 | DRAM latency | 0.09% |
| **Theoretical peak** | **546** | — | 100% |

## 9.2 What These Numbers Mean for the Project

**Sequential vs random ratio (11–16×)**: this single number justifies every tiling decision in Phase 1. A 4000×4000 heightmap processed naively (row by row, column access patterns) hits random-access territory. Tiling keeps the working set in L1/L2 and preserves sequential-access speeds.

**SIMD 4–7% of peak**: with rayon parallelism (10 threads) × SIMD (4 lanes) × multiple streams, approaching 50–100 GB/s for sequential reads is realistic in Phase 2.

**Latency-bound random access**: no SIMD or parallelism trick overcomes 100 ns DRAM latency per miss. The only fix is better memory layout (avoid misses entirely).

---

# Part 10: Key Takeaways

1. **Layout enables SIMD, not the other way around.** Fix the data layout first, then vectorize. `vld1q_f32` needs adjacent data. SoA > AoS for SIMD.

2. **Identify the bottleneck before choosing the optimization.** Compute-bound → SIMD helps. Bandwidth-bound sequential → SIMD + prefetcher. Latency-bound random → fix the access pattern.

3. **The compiler may or may not vectorize. Check the assembly.** LLVM's auto-vectorizer fails silently on FP reductions. Never assume — verify with `--emit=asm`.

4. **Always measure frequencies empirically.** `sysctl hw.tbfrequency` was wrong. `Instant::now()` was the ground truth.

5. **`--release` always for benchmarks.** Debug mode is 10–50× slower with bounds checks everywhere.

6. **Sequential/random ratio = 11–16×.** This is the cost of a cache miss. Internalize this number — every architectural decision in Phases 1–4 is justified by it.

7. **Single thread = ~4–7% of M4 Max peak.** The hardware can do much more. Phase 2 + rayon unlocks it.

8. **MLP is distinct from SIMD arithmetic.** More concurrent loads = more DRAM utilization. This is why random_read_simd beat scalar random_read — not from arithmetic throughput.
