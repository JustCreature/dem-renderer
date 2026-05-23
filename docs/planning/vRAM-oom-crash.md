# Fix vRAM OOM on low-VRAM discrete GPUs (issue #33)

## Context

On a Windows ACER laptop with an NVIDIA GTX 1650 (4 GB vRAM), the demo view consumes ~3.8 GB of vRAM at startup and crashes with `wgpu error: Out of Memory` (wgpu_core.rs:1614) after flying around for a short while. The same machine has 20 GB of shared RAM available, but wgpu never falls back to it. The fix has to land on this hardware without regressing the M4 Max experience.

Three independent root causes compound:

1. **`set_hm5m_inactive` / `set_hm1m_inactive` are cosmetic.** They only set `extent_x = 0.0`; the underlying textures / buffers stay GPU-resident forever (`crates/render_gpu/src/scene/tiers.rs:143-145`, `:269-271`).
2. **Grow-only reload doubles peak memory.** A tier resize creates the new resource *before* releasing the old one; the BindGroup keeps the old `TextureInner` Arc-alive until `rebuild_bind_group()` lands. Peak ≈ `sizeof(old) + sizeof(new)` (`crates/render_gpu/src/scene/tiers.rs:23-81`, `:165-263`; `mod.rs:987-1057`).
3. **No VRAM-aware sizing.** Tier radii are hardcoded (`src/viewer/tiers.rs:12-17`). `GpuContext` detects `device_type` but never branches on it (`crates/render_gpu/src/context.rs:44-45`). No `on_uncaptured_error` handler exists.

Measured budget at demo start (Tirol):

| Tier         | Texture     | Normals     | Shadow      | AO        | Total     |
|--------------|-------------|-------------|-------------|-----------|-----------|
| Base (R16F)  | 72 + 72 MB  | 144 MB      | 144 MB      | 36 MB     | ~468 MB   |
| Close (R32F) | 268 MB      | 268 MB      | 268 MB      | —         | ~804 MB   |
| Fine  (R32F) | 196 MB      | 196 MB      | 196 MB      | —         | ~588 MB   |

Steady-state with all three loaded: ~1.86 GB. Reload peak (old + new for close + fine): ~3.25 GB — matches the user's 3.8 GB reading right before the crash.

**Intended outcome:** the demo runs without crashing on the GTX 1650, the M4 Max behavior stays unchanged (high-VRAM path), and reload peaks drop for every system.

**Out of scope (deferred):** R32Float → R16Float for the close tier (visual quantization risk on high-altitude camera views), upgrading wgpu past 0.29.x, any platform-specific VRAM querying.

---

## Approach

Five layered phases, each independently verifiable on the M4 Max. After Phase 2 the immediate crash is fixed; phases 3–5 polish and harden.

### Phase 1 — Instrumentation (do first, zero behavior change)

**Goal:** make GPU allocations observable on every system so the rest of the work can be validated without a GTX 1650.

- **`crates/render_gpu/src/scene/mod.rs`, `scene/tiers.rs`:** thin wrappers `create_texture_tracked()` / `create_buffer_tracked()` around `device.create_texture` / `device.create_buffer` that take a `label: &str`, estimate bytes from the descriptor, and update module-level `AtomicU64` counters (`GPU_TEXTURE_BYTES`, `GPU_BUFFER_BYTES`). Log `alloc <label>: NN MB (total tex: NN MB, buf: NN MB)` to stderr. Gate behind `RUST_LOG=dem_renderer::vram=debug` once it's chatty.
- **`src/viewer/mod.rs:657` (keyboard handler):** add a Shift+R hotkey that calls `bev_base.close.invalidate()` and `bev_base.fine.as_mut().map(|f| f.invalidate())` so reloads can be forced without flying. Add `invalidate()` on `StreamingTier` to reset `last_cx`/`last_cy` to a sentinel that always trips `needs_reload`.

### Phase 2 — Eager deallocation on reload (the crash fix)

**Goal:** peak vRAM during a tier swap drops from `old + new` to roughly `max(old, new)`.

The mechanism: in wgpu, `BindGroup` holds an internal Arc on each `TextureInner` / `BufferInner`. Dropping the Rust handle is not enough — the destruction is queued for "after the last submission that used it retires". Three things have to happen, in order:

1. Replace the Rust field with a 1×1 placeholder so the Arc count on the old resource can fall to one.
2. Call `rebuild_bind_group()` to release the old BindGroup's reference.
3. Call `device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None })` to force the wgpu hub's destroy-after-submission list to drain.

Files & changes:

- **`crates/render_gpu/src/scene/tiers.rs:143-145`** (`set_hm5m_inactive`): replace all five fields (`_hm5m_texture`, `_hm5m_view`, `_hm5m_normal_tex`, `_hm5m_normal_view`, `_hm5m_shadow_buf`) with 1×1 placeholders. Reuse the placeholder construction pattern already in `scene/mod.rs:313-330` and `:344-360` — extract it into a private helper `make_hm5m_placeholders(&Device) -> (...)` to avoid duplication. Set `hm5m_cols = 0`, `hm5m_rows = 0`, `hm5m_buf_elems = 1`, `hm5m_extent_x = 0.0`. Call `self.rebuild_bind_group()`, then `self.gpu_ctx.device.poll(...)`.
- **`crates/render_gpu/src/scene/tiers.rs:269-271`** (`set_hm1m_inactive`): same shape, symmetric helper `make_hm1m_placeholders`.
- **`crates/render_gpu/src/scene/tiers.rs:26-66`** (`upload_hm5m` inside `if size_changed`): before allocating the new texture, set `self.hm5m_extent_x = 0.0` (shader-side disable so a render dispatch landing in this window samples a no-op tier), replace the five fields with placeholders, `rebuild_bind_group()`, `device.poll(Wait)`. *Then* allocate the new texture/normal/shadow, assign to `self`, `rebuild_bind_group()` again. Restore `extent_x` after the new BindGroup is installed and the `write_texture` calls land. Two rebuilds per reload is cheap (microseconds).
- **`crates/render_gpu/src/scene/tiers.rs:165-263`** (`upload_hm1m`): same pattern.
- **`crates/render_gpu/src/scene/mod.rs:987-1057`** (`update_heightmap` for base tier): same pattern — base reload is rarer but the doubling still costs ~470 MB during the swap.

The `extent_x = 0.0` safety net handles the sub-frame window where the BindGroup points at a placeholder. The shader-side checks at `scene/mod.rs:774, 898` already early-out when `extent_x == 0`.

**Risk:** `device.poll(Wait)` blocks the calling thread (≤ 1 ms idle, up to ~16 ms if a heavy dispatch is in flight). Acceptable here because reloads already produce a perceptual hitch (CPU-side window extract + normals + shadow). *Never* call it in the steady-state render loop.

### Phase 3 — VRAM class detection + tier-radius scaling

**Goal:** drop the steady-state footprint on low-VRAM cards so reloads can't OOM even with Phase 2 in place.

- **`crates/render_gpu/src/context.rs`:** add `pub enum VramClass { Low, Mid, High }` and `pub vram_class: VramClass` on `GpuContext`. Decision rule (no platform APIs, no probing):
  1. User override (Phase 4) wins.
  2. Adapter name substring match on a static table of known low-VRAM cards (GTX 1050, 1050 Ti, 1650, 1660, MX150–MX450, Iris/UHD, Arc A310/A380, RX 550/560) → `Low`.
  3. `info.name` contains "Apple" → `High` (unified memory; preserves M4 Max behavior).
  4. `IntegratedGpu` → `Low`; `DiscreteGpu` → `Mid`; virtual/CPU → `Low`.
- **`src/viewer/tiers.rs:12-17`:** replace the three `const f64` declarations with a `pub struct TierRadii { base, base_drift, close, close_drift, fine, fine_drift }` and `pub fn tier_radii(class: VramClass) -> TierRadii`. Suggested values:
  - `Low`:  base 50 km / drift 17 km, close 8 km / drift 1.5 km, **fine disabled (0.0)**
  - `Mid`:  base 70 km / drift 23 km, close 14 km / drift 2.5 km, fine 2.5 km / drift 800 m
  - `High`: base 90 km / drift 30 km, close 20 km / drift 3 km, fine 3.5 km / drift 1 km (today's values)
- **`src/viewer/scene_init.rs`, `src/viewer/tiers.rs` (`BevBaseState::new`):** thread `TierRadii` from `GpuContext::vram_class` (or the override) into the worker spawns. When `fine == 0.0`, skip spawning the fine worker entirely (no allocation, no upload path).
- **`src/viewer/mod.rs:520-554` (tier reload dispatch loop):** guard the fine-tier branch on `bev_base.fine.is_some()`.

Low preset memory math: base ≈ 70 MB, close ≈ 120 MB, fine disabled → ~200 MB resident + ~200 MB reload peak. Comfortably fits inside the GTX 1650's usable budget after the desktop compositor and shader caches.

### Phase 4 — Launcher UI override

**Goal:** escape hatch when the heuristic is wrong.

- **`src/launcher/config.rs:78-100`** (`LauncherSettings`): add `pub vram_budget: VramBudget` (enum `Auto | Low | Mid | High`) with serde default `Auto`. Persists to the existing `config.toml`.
- **`src/launcher/screens/settings.rs`:** add a dropdown reusing the same style as `lod_mode`. Label: "VRAM budget". Help text: "Auto-detected from your GPU. Override if the demo crashes or looks unnecessarily low-detail."
- **`src/launcher/config.rs` (`LauncherOutcome::Start`)** and **`src/viewer/mod.rs` (`Viewer::from_launcher`)**: plumb the resolved `VramClass` (heuristic + override) into the viewer.

### Phase 5 — OOM safety net + staggered reloads

**Goal:** parachute for cases the heuristic misclassifies, and reduce inter-reload overlap.

- **`crates/render_gpu/src/context.rs:53-60`** (after `request_device`):
  ```rust
  device.on_uncaptured_error(Box::new(|err| {
      eprintln!("[GPU ERROR] {:?}", err);
      if matches!(err, wgpu::Error::OutOfMemory { .. }) {
          OOM_OBSERVED.store(true, Ordering::SeqCst);
      }
  }));
  ```
  Module-level `static OOM_OBSERVED: AtomicBool`. Public `pub fn oom_observed() -> bool` for the viewer to poll.
- **`src/viewer/mod.rs`** (top of frame loop): on first observation, set a flag that disables fine-tier reloads permanently and calls the eager-dealloc path from Phase 2 to free the fine tier. On second observation, same for close. Print one stderr warning per degradation step.
- **`src/viewer/tiers.rs` (`BevBaseState`):** add `enum ReloadGate { Idle, BaseInFlight, CloseInFlight, FineInFlight }` so the reload dispatch loop in `viewer/mod.rs:520-554` only triggers one detail-tier reload at a time. Drift checks for tier N wait until the gate permits. Base drift always takes priority. This prevents close + fine reloads from stacking destruction queues when the camera slows below the 2500 m/s speed gate.
- **Debug hotkey** (`viewer/mod.rs`): `Ctrl+Shift+O` flips `OOM_OBSERVED` so the degradation path can be tested on the M4 Max.

---

## Files modified

- `crates/render_gpu/src/context.rs` — `VramClass`, OOM handler
- `crates/render_gpu/src/scene/mod.rs` — `update_heightmap` eager dealloc, tracked allocation wrappers
- `crates/render_gpu/src/scene/tiers.rs` — `set_hm*_inactive` real dealloc, `upload_hm*` drop-before-create, placeholder helpers
- `src/viewer/tiers.rs` — `TierRadii` + `tier_radii(class)`, `ReloadGate`, `StreamingTier::invalidate`
- `src/viewer/scene_init.rs` — thread `TierRadii` through scene prep
- `src/viewer/mod.rs` — VRAM class plumb-through, OOM polling, Shift+R / Ctrl+Shift+O debug hotkeys, gated fine-tier branch
- `src/launcher/config.rs` — `VramBudget` field on `LauncherSettings`, `LauncherOutcome` plumbing
- `src/launcher/screens/settings.rs` — VRAM budget dropdown

## Reusable building blocks (don't duplicate)

- `cap_to_gpu_limit` (`src/viewer/tiers.rs:22-42`) — already crops to `GPU_SAFE_PX`; layers correctly on top of the smaller radii.
- `rebuild_bind_group` (`crates/render_gpu/src/scene/bind_group.rs`) — used by Phase 2's "drop, rebuild, poll, allocate, rebuild" sequence.
- 1×1 placeholder construction pattern (`crates/render_gpu/src/scene/mod.rs:313-330`, `:344-360`) — extract to helpers `make_hm5m_placeholders` / `make_hm1m_placeholders`.
- `hm_to_f16_bytes`, `pack_normals_rg16_bytes`, `pack_ao_u8` (worker-side byte packing) — unchanged; Phase 3 still feeds them, just with smaller windows.

## Verification (M4 Max)

1. **Phase 1 only:** run `cargo run --release`, fly the Tirol demo. Confirm allocation log emits a steady-state ≈ 1.8–1.9 GB. Press Shift+R to force close + fine reloads. Confirm the brief peak in the log reaches ≈ 3.2 GB.
2. **After Phase 2:** same flight, same Shift+R. Confirm peak drops to ≈ 1.9–2.0 GB (no more old+new overlap). The `[GPU ERROR]` line never appears.
3. **After Phase 3, with `vram_budget = Low` override:** steady-state ≈ 200 MB, peak ≈ 400 MB. Visually confirm base coverage still surrounds the camera, close tier is detailed within ~8 km, fine tier is absent.
4. **After Phase 4:** verify `config.toml` persists `vram_budget = "Mid"` and the launcher dropdown reflects the saved value across restarts.
5. **After Phase 5:** trigger Ctrl+Shift+O. Confirm fine tier disappears, no crash. Trigger again — close tier disappears. Confirm one warning line per step.
6. **GTX 1650 (separate machine):** run with default `Auto` budget. Confirm `[GPU] selected` prints `... (DiscreteGpu)`, `vram_class = Low` is logged, demo loads, free-flight for ≥ 5 minutes without crash.

## What NOT to do

- **No wgpu version bump.** 0.29 → 0.30 is a breaking change (`Maintain` → `PollType`, Surface lifetime model, `Required` features). Out of scope.
- **No `device.poll(Wait)` inside the steady-state frame loop.** It blocks; one frame stall ≥ 16 ms. Only at reload boundaries.
- **No internal-allocator patches** (`gpu-allocator`, Metal). No public knobs; breaks portability.
- **No `Arc<Mutex<Texture>>` wrappers.** wgpu types are already internally Arc'd.
- **No probe allocations** to detect VRAM. The name + device_type heuristic is good enough and degrades safely (unknown card → `Mid`).
- **No close-tier R32 → R16 change** in this PR. Defer; visible quantization on ridgelines above ~4500 m camera elevation needs careful evaluation.
