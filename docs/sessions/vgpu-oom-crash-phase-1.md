# Phase 1 — GPU allocation instrumentation

**Issue:** [#33 — vRAM OOM Error, no shared RAM utilisation](https://github.com/JustCreature/dem_renderer/issues/33)
**Full plan:** `~/.claude/plans/curious-booping-rabbit.md`

## Goal

Make every GPU texture / buffer allocation visible so the rest of the fix can be validated on a machine that doesn't actually OOM (M4 Max). Zero behaviour change.

## Why this had to come first

wgpu 0.29 does not expose VRAM residency or memory pressure through any public API. `adapter.limits()` returns dimensional caps (max texture size, max binding count) but no memory budget. `wgpu::Limits` has no `max_memory_bytes`. The driver's actual VRAM accounting is opaque.

So to test whether the upcoming Phase 2 fix (drop-before-create) actually reduces reload peaks, we need a CPU-side proxy that mirrors what we tell the device to allocate. That's good enough to compare *before* vs *after* — even if the absolute numbers don't match what the driver internally tracks (which would include staging buffers, descriptor heaps, shader caches, etc. that wgpu hides from us).

## What was added

### `crates/render_gpu/src/vram.rs` (new)

Two `AtomicU64` counters — `GPU_TEXTURE_BYTES`, `GPU_BUFFER_BYTES` — plus thin tracked wrappers:

- `create_texture_tracked(device, desc, label)` — computes bytes from descriptor (per-mip pyramid for textures with `mip_level_count > 1`), increments counter, logs an event, returns the wgpu `Texture`.
- `create_buffer_tracked(device, desc, label)` — same for `create_buffer`.
- `create_buffer_init_tracked(device, desc, label)` — same for `create_buffer_init`.
- `track_texture_drop(t, label)` / `track_buffer_drop(b, label)` — *manual* drop accounting, called at the call sites that are about to overwrite a stored field. The actual wgpu resource isn't freed at that moment — wgpu's hub keeps it alive until the BindGroup that references it is rebuilt *and* the next submission retires. But the call site is the right place to log "intent to release", and after Phase 2 it'll become the right place to log actual release as well (because Phase 2 inserts the rebuild + `device.poll(Wait)` immediately after).

Format-to-bytes-per-pixel lookup covers the formats this project uses: R8/R16/R32 Float and Snorm, Rg16/Rgba8/Rgba16/Rgba32 variants. Unknown formats fall back to 4 bytes (overestimates but never under).

Counter reads in the log line are `Ordering::Relaxed` because we only need eventual consistency for human-readable progress logs, not synchronization. The updates are also `Relaxed` — they never gate any program logic.

### `crates/render_gpu/src/lib.rs`

Added `pub mod vram;` so the module is reachable cross-crate (the viewer will need to read counter values from Phase 5's OOM-handler path).

### `crates/render_gpu/src/scene/mod.rs`

Wrapped every `gpu_ctx.device.create_texture(...)` / `create_buffer(...)` / `create_buffer_init(...)` site:

- `create_tier_placeholder` — 1×1 hm + 1×1 normal + 1-elem shadow buf (allocated twice: once for hm5m, once for hm1m).
- `GpuScene::new` — base hm texture (R16Float, 8 mips), AO texture (R8Unorm), normals_packed buffer, shadow buffer, camera uniform, output buffer, readback buffer.
- `GpuScene::resize` — output + readback re-creation. Now also calls `track_buffer_drop` for the old ones, so the counter goes down then back up on a window resize.
- `GpuScene::update_heightmap` — base hm / AO / normals_packed / shadow re-creation on tile slide. Drops tracked.

### `crates/render_gpu/src/scene/tiers.rs`

Wrapped both grow paths in `upload_hm5m` and `upload_hm1m`. The drops are logged immediately before the matching alloc — which in the current grow-only world is misleading (the actual GPU resource is still held alive by the prior BindGroup). After Phase 2, this same call site will be load-bearing because the drop will be immediately followed by `rebuild_bind_group` + `device.poll(Wait)`, which is the only thing that actually triggers wgpu's deferred-destruction queue to drain.

### `src/viewer/mod.rs` (debug hotkey)

Added an `R` keybinding next to the existing letter shortcuts. On press it calls `bev_base.close.invalidate()` and (if present) `bev_base.fine.as_mut().map(|f| f.invalidate())`. `invalidate()` already existed on `StreamingTier` — it sets `last_cx`/`last_cy` to `0.0`, which is far from any real Austrian CRS coordinate so `needs_reload` returns true on the next frame. That makes reload peaks reproducible without flying — important for testing on the M4 Max where high speed never fires the speed-gate suppression.

I didn't bother gating on Shift because no other binding uses `R` and the hotkey is harmless. The original plan was Shift+R; using bare `R` removes the modifier-tracking dependency.

## Observed startup baseline

From a clean launch into the Tirol demo on the Intel Iris Plus (integrated GPU, unified memory):

| Resource | Size | Format / dims |
|---|---|---|
| `scene_hm_tex` | 296.63 MB | R16Float, 10800×10800, 8 mips |
| `scene_ao_tex` | 111.24 MB | R8Unorm, 10800×10800 |
| `normals_packed` | 444.95 MB | u32 storage buffer, 10800×10800 |
| `shadow` | 444.95 MB | f32 storage buffer, 10800×10800 |
| `output` + `readback` | 3.25 MB × 2 | RGBA32, 1600×533 (grows to 13 MB × 2 on resize) |
| `cam` | <1 KB | UNIFORM, `sizeof(CameraUniforms)` |
| `hm5m_*` placeholder | 0 MB | 1×1 |
| `hm1m_*` placeholder | 0 MB | 1×1 |
| **Initial base footprint** | **~1300 MB tex+buf** | |
| First `hm5m` grow | +732 MB | 7998×7998 R32Float + Rg16Snorm + f32 shadow |
| First `hm1m` grow | +561 MB | 7000×7000 R32Float + Rg16Snorm + f32 shadow |
| **Steady state, all three tiers** | **~2.6 GB** | tex 1269 MB + buf 1346 MB |

This is significantly bigger than the pre-instrumentation estimate (the plan guessed ~1.86 GB). The reason: the Tirol demo uses a **3×3 Copernicus stitched grid at 10800×10800 pixels**, not the 6000×6000 single-tile that the plan modeled. The grow-only base tier only shrinks (to 8192×5821) on the *first reload* after the camera moves away from the demo start — and that's exactly when the user reports OOM on the GTX 1650.

The base reload that's visible in the log (`BEV base reload triggered at lat=47.0872 lon=11.9605`) demonstrates this: the base shrinks from 10800×10800 to 8192×5821, freeing ~960 MB of texture + buffer space. But during the swap, the old + new briefly coexist (which Phase 2 will fix).

## Why the current "drop before alloc" log ordering is misleading (but useful)

Look at the log around a base reload:

```
[vram]  drop tex scene_hm_tex           - 296.63 MB  (tex   973.1 MB, ...)
[vram] alloc tex scene_hm_tex           + 121.26 MB  (tex  1094.4 MB, ...)
```

The counter goes down then up by the diff. This *looks* like drop-first behaviour, but it's only the accounting that's drop-first. The actual wgpu resources behave like this:

1. `track_texture_drop` runs — counter ↓ — but the Rust field still owns the texture, and the BindGroup still references it.
2. `create_texture_tracked` runs — counter ↑ — wgpu now holds two textures: the about-to-be-replaced one and the new one. **Real GPU memory at this moment = old + new.**
3. Rust field reassignment drops the old handle's Rust ownership, but the BindGroup keeps an internal Arc on `TextureInner`.
4. `rebuild_bind_group()` runs — Arc count on old falls to "scheduled for destroy".
5. The next `queue.submit(...)` completes; wgpu's hub processes the destroy-after-submission list; the old texture is actually freed.

So the counter understates the real reload peak. The peak we *can* see in the log is the sum of the current counter plus whatever's pending in wgpu's destroy queue — and that pending amount is exactly what Phase 2 will reduce.

After Phase 2, the same `track_texture_drop` call site will be followed by `rebuild_bind_group()` and `device.poll(PollType::Wait { ... })` *before* the matching `create_texture_tracked`. At that point the counter ordering will match GPU residency: counter ↓ → wgpu drain → counter ↑ → new texture lives in GPU. Peak = `max(old, new)` not `old + new`.

## How to use this for Phase 2 verification

On the M4 Max:
1. Launch the demo.
2. Note the steady-state counter (tex + buf totals at the bottom of each log line).
3. Press `R` to force close + fine reloads.
4. Read the next handful of log lines. Today: counter goes drop → alloc for each of the six tier resources, but each alloc happens *while wgpu still holds the old*. After Phase 2: the counter dip on each drop will be real, and the alloc on top of the dip will leave the steady-state lower.

A coarser but reliable check: watch macOS Activity Monitor → GPU → "Memory Used" before and after `R`. Today the value spikes for ~1 second while the old + new coexist. After Phase 2 the spike should disappear (or be much smaller, bounded by the new allocation only).
