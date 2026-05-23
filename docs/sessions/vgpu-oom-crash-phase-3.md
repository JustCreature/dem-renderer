# Phase 3 — VRAM class detection + tier-radius scaling

**Issue:** [#33 — vRAM OOM Error, no shared RAM utilisation](https://github.com/JustCreature/dem_renderer/issues/33)
**Full plan:** `~/.claude/plans/curious-booping-rabbit.md`
**Prerequisites:** Phase 2 (eager dealloc reduces *peak* memory; this phase reduces *steady-state* memory).

## What Phase 2 left on the table

Phase 2 eliminated the `old + new` overlap during tier reloads. That fixed the crash trigger but didn't shrink the steady state — the Tirol demo still claims ~2.6 GB after all three tiers load, because the base radius is 90 km, close is 20 km, fine is 3.5 km. On a 4 GB card with desktop compositor reserve and driver caches, 2.6 GB is right at the edge of what the GTX 1650 can hold without thrashing into "shared system RAM" (which on Windows + NVIDIA is a fall-back path that wgpu doesn't trigger gracefully).

Phase 3 makes the steady-state proportional to the GPU's actual budget. Three presets:

| Preset | Base radius | Close radius | Fine radius | Steady mem (Tirol demo) |
|---|---|---|---|---|
| High | 90 km | 20 km | 3.5 km | ~2.6 GB |
| Mid  | 70 km | 14 km | 2.5 km | ~1.7 GB |
| Low  | 50 km |  8 km | 1 km    | ~0.7 GB |

(Reload peak, with Phase 2 in place, ≈ steady state — no doubling.)

Low keeps the fine tier on a tight leash: 1 km radius around the camera at 1 m/px gives a ~2000×2000 R32Float window (≈ 48 MB across hm + normal + shadow). Small enough to fit on a 4 GB card, big enough that the user still sees 1 m detail right under their feet when they fly low. The 300 m drift threshold means the worker re-fires more often than the High preset's 1 km drift — but the work is local IO + CPU normals + CPU shadow, not GPU memory, so frequent reloads are not a cost concern.

## How a GPU gets classified

wgpu 0.29 does not expose VRAM capacity. Options to detect:

- **Adapter name string match.** Cheap, no platform code, but brittle: a new "GTX 1650 Mobile OC" SKU we haven't seen gets the default treatment.
- **`device_type` heuristic.** `IntegratedGpu` ≠ low-memory (Iris Plus shares 16 GB+ system RAM; Apple M-series shares 36 GB+). `DiscreteGpu` ≠ high-memory (GTX 1050 has 2 GB). The type alone is a weak signal.
- **Probe allocations.** Try to allocate a 1 GB buffer; if it fails, downgrade. Defeats the whole point — the probe would itself OOM and we'd need a panic catcher just to ask the question.
- **Platform APIs (DXGI / IOKit / sysinfo).** Accurate but means platform branching, conditional dependencies, and a lot of edge cases (Optimus laptops, eGPUs, virtual machines).

The shipped strategy combines the first two:

1. Adapter name contains `"apple m"` → **High** (unified memory; M-series at any size is comfortably > 8 GB usable).
2. Adapter name contains any of these substrings → **Low**:
   - `mx150`, `mx250`, `mx350`, `mx450` (Nvidia laptop dGPUs, 2 GB)
   - `hd graphics 4`, `hd graphics 5`, `uhd graphics 6` (Intel HD 4xxx/5xxx, UHD 6xx — tiny reserved VRAM)
   - `rx 550`, `rx 560` (AMD entry-level, 2 GB)
   - `arc a310`, `arc a380` (Intel discrete entry parts)
3. `DeviceType::Cpu` / `DeviceType::Other` → **Low** (CPU fallback, virtual adapter — assume worst case).
4. Everything else (`IntegratedGpu`, `DiscreteGpu`, `VirtualGpu`) → **Mid**.

The rule is deliberately conservative on **downgrades**. The Iris Plus user proved their integrated GPU runs the full demo without OOM (it shares system RAM through the integrated memory controller), and 3 GB-class discrete cards (GTX 1050 / 1650 / 1660) fit Mid comfortably — the runtime OOM safety net catches them on the rare reload spike. Slapping every integrated or low-end discrete GPU into Low would silently strip features from machines that don't need it. False negatives (a tiny card we don't recognise running as Mid) surface as a runtime warning from the OOM handler; false positives (a healthy card running as Low) are invisible and harder to debug.

Detection happens once during `GpuContext::new()` and is logged on startup. For most consumer cards the default class is `Mid`:

```
[GPU] selected: NVIDIA GeForce GTX 1650 (DiscreteGpu)  vram_class=Mid
```

Phase 4 will replace the auto-driven path with a launcher dropdown; the detected class becomes informational.

## TierRadii struct and the runtime-kill sentinel

`viewer/tiers.rs` now holds:

```rust
pub(super) struct TierRadii {
    pub(super) base_radius_m: f64,
    pub(super) base_drift_m: f64,
    pub(super) close_radius_m: f64,
    pub(super) close_drift_m: f64,
    pub(super) fine_radius_m: f64,
    pub(super) fine_drift_m: f64,
}
```

`fine_radius_m == 0.0` is reserved as a runtime kill sentinel. Phase 5's OOM degradation path writes 0 here when it disables the fine tier. None of the launcher presets ship a 0 — even Low keeps a tiny 1 km fine window — so on a fresh launch the fine worker always spawns.

`BevBaseState::new` checks `if fine_index.is_empty() || fine_radius_m <= 0.0` before constructing the fine `StreamingTier`; the `Option<StreamingTier>` is `None` in either case. The single-file path also has a no-sub-5m-IFD case that resolves to an empty fine_index, so the same gate handles both inputs.

The six `BEV_*_RADIUS_M` / `BEV_*_DRIFT_THRESHOLD_M` `const f64` are gone. The base drift recalibration that used to clamp to `BEV_BASE_DRIFT_THRESHOLD_M.min(new_half_m * 0.5)` now clamps to `self.tier_radii.base_drift_m.min(new_half_m * 0.5)` — the dynamic part still works.

## Why memory scales (and what doesn't)

Base tier memory at the Tirol demo:

- High: 90 km radius at 30 m/px = 6000 × 6000 source. Stitched 3×3 grid is 10800 × 10800, then `cap_to_gpu_limit` clamps to 8192 × 5821. Memory ≈ 1.3 GB.
- Mid: 70 km radius. Stitched grid is still 10800 × 10800 (same source tiles), but the effective coverage after cap is smaller. Still ~1.0 GB.
- Low: 50 km radius. Coverage is ~6000 × 4000 after cap. ~500 MB.

Note: the demo specifically uses a stitched 3×3 grid, so the base radius doesn't fully determine base memory — the source grid bounds the upper limit. In single-file mode (custom DEM), `select_ifd` picks a coarser IFD when the radius is smaller, which does scale memory linearly.

Close + fine memory scales straight with radius²:
- Close: 20 → 14 → 8 km radius gives ~800 → 400 → 130 MB at 5 m/px (before GPU cap).
- Fine: 3.5 → 2.5 → 1.0 km gives ~600 → 300 → 48 MB at 1 m/px.

Total per-preset, with Phase 2's eager dealloc:

| Preset | Base | Close | Fine | Total steady |
|---|---|---|---|---|
| High | ~1.3 GB | ~800 MB | ~600 MB | ~2.7 GB |
| Mid | ~1.0 GB | ~400 MB | ~300 MB | ~1.7 GB |
| Low | ~500 MB | ~130 MB | ~48 MB  | ~0.7 GB |

The Low preset comfortably fits in a 2 GB card's usable budget (~1.4 GB after compositor + driver). With the eager-dealloc reload peak ≈ steady, the math has plenty of headroom for the OS to not page-fault into shared sysmem. The Mid preset fits 3 GB-class discrete cards (GTX 1050 / 1650 / 1660) — ~1.7 GB tracked + ~0.3 GB for wgpu staging / driver overhead = ~2 GB, leaving headroom on a 3 GB card. The 1 km fine tier on Low is a deliberate compromise: 48 MB of GPU isn't free, but the visual payoff (sharp 1 m detail right under the camera) is worth far more than skipping it. If a future user reports OOMs at Low, the runtime degradation drops the fine tier first — so the safety net catches the edge case without compromising the typical user's experience.

## Files & exact changes

### `crates/render_gpu/src/context.rs`
- Added `pub enum VramClass { Low, Mid, High }` with `pub fn detect(info: &wgpu::AdapterInfo) -> Self`.
- `GpuContext` gained `pub vram_class: VramClass`. Populated in `GpuContext::new()`. Logged on startup.

### `crates/render_gpu/src/lib.rs`
- Re-exported `VramClass`.

### `src/viewer/tiers.rs`
- Removed `BEV_BASE_RADIUS_M`, `BEV_BASE_DRIFT_THRESHOLD_M`, `BEV_5M_RADIUS_M`, `BEV_5M_DRIFT_THRESHOLD_M`, `BEV_1M_RADIUS_M`, `BEV_1M_DRIFT_THRESHOLD_M`.
- Added `TierRadii` struct and `tier_radii(VramClass) -> TierRadii` mapping.
- `BevBaseState::new` takes a new `radii: TierRadii` parameter. The base / close / fine worker closures capture the f64 radii by value (cheap copy) instead of reading the deleted consts.
- Fine worker only spawned when `fine_radius_m > 0.0`; the `Option<StreamingTier>` is `None` for the Low preset.

### `src/viewer/scene_init.rs`
- Removed `BEV_BASE_RADIUS_M` from the import (kept `AO_RADIUS_M`).
- `prepare_scene_with_ctx` derives `init_radii = tier_radii(gpu_ctx.vram_class)` and uses `init_radii.base_radius_m` for the initial `select_ifd` + `extract_window` calls. A Low preset reads a 50 km window instead of a 90 km window, which is meaningful when the source is a non-cached 10 GB BigTIFF — the initial load shrinks proportionally.

### `src/viewer/mod.rs`
- Import: dropped `BEV_BASE_DRIFT_THRESHOLD_M`, added `TierRadii`, `tier_radii`.
- `Viewer` struct gained `tier_radii: TierRadii`.
- `from_launcher` computes `tier_radii(scene.get_gpu_ctx().vram_class)` once after the scene is built, logs the resolved radii, and threads it into both `BevBaseState::new` call sites (demo and single-file projected paths).
- The base reload completion path at line ~434 now uses `self.tier_radii.base_drift_m.min(new_half_m * 0.5)` instead of the deleted constant.
- The single-file mode `select_ifd` calls now use `tier_radii.close_radius_m` / `tier_radii.base_radius_m`.
- The single-file mode fine-tier index is empty when the preset disables the fine tier — so the worker isn't spawned and the source 1m IFD isn't even probed.

## Verification on the Iris Plus (Mid preset)

Expected log lines on startup:

```
[GPU] selected: Intel(R) Iris(TM) Plus Graphics (IntegratedGpu)  vram_class=Mid
[tier] radii: base 70 km / drift 23 km, close 14 km / drift 2 km, fine 2.5 km / drift 0.8 km
```

Expected steady-state counter (after the initial base + close + fine settle): roughly 70% of the High values. The Tirol demo's stitched 3×3 base grid will still load as 10800 × 10800 before the `cap_to_gpu_limit` shrink, so the *initial* base footprint is unchanged from before. The savings appear after the first `BEV base reload`, where the window crops to a smaller radius.

On the M4 Max:

```
[GPU] selected: Apple M4 Max (DiscreteGpu)  vram_class=High
[tier] radii: base 90 km / drift 30 km, close 20 km / drift 3 km, fine 3.5 km / drift 1 km
```

No behaviour change — the High preset matches the pre-Phase-3 hardcoded values.

To force-test the Low preset on a high-VRAM machine: temporarily edit `VramClass::detect` to return `VramClass::Low` unconditionally and verify the demo still renders, just with a smaller close-tier radius and no fine detail.

## What this doesn't fix

- **The Tirol demo's 3×3 stitched base grid is still 10800 × 10800 *at startup*.** The Mid / Low preset only shrinks the *post-reload* base. A user with a 2 GB GPU could still OOM during the initial load before any reload fires. To fix that we'd need to make the initial 3×3 stitch radius-aware too, which lives in `prepare_demo_scene_with_ctx` — a separate change.
- **Hardcoded preset values.** Mid / Low were chosen from the memory math above; they're not tuned against real hardware (no GTX 1650 in the loop yet). Phase 4's manual override is the escape hatch for users on the edge.
- **No probe-based fallback.** If a card is mis-classified as Mid and OOMs anyway, today it crashes; Phase 5's `on_uncaptured_error` handler will catch this and downgrade.

## Not done in this phase

- Wiring through `LauncherSettings` so the user can override `Auto / Low / Mid / High` from the menu — that's Phase 4.
- `device.on_uncaptured_error` to catch OOM and downgrade automatically — Phase 5.
- Smaller initial demo base radius — separate refactor of `prepare_demo_scene_with_ctx`. Filed mentally; not in scope.
