# Viewer Refactor Plan

## Problem

Every new resolution tier duplicates 5 fields and a poll block:
`hm5m_tx`, `hm5m_rx`, `hm5m_computing`, `last_5m_cx`, `last_5m_cy`.
Three tiers (30m / 5m / 1m) = 15 near-identical fields + 3 near-identical poll loops.
`src/viewer/mod.rs` is already ~1300 lines with only two tiers.

---

## Step 1 — Introduce `StreamingTier`

Replace all per-tier duplicated fields with a single struct:

```rust
struct StreamingTier {
    tx: mpsc::SyncSender<(f64, f64)>,
    rx: mpsc::Receiver<TierBundle>,
    computing: bool,
    last_cx: f64,
    last_cy: f64,
    drift_threshold_m: f64,
    radius_m: f64,
    ifd_level: u32,
}
```

`BevBaseState` holds `Vec<StreamingTier>` (or `[StreamingTier; N]`).
The poll+dispatch loop iterates it — no tier-specific branches.
Adding 1m = push one more `StreamingTier` with the right parameters.

---

## Step 2 — Split `mod.rs` into four modules

| Module | Contents | Rationale |
|--------|----------|-----------|
| `viewer/geo.rs` | `lcc_epsg31287`, `laea_epsg3035`, `latlon_to_tile_metres`, `sun_position` | Pure math, no state, no wgpu |
| `viewer/scene_init.rs` | `prepare_scene`, `compute_ao_cropped` | One-time startup: loads data, builds `GpuScene` |
| `viewer/tiers.rs` | `StreamingTier`, bundle structs, `spawn_tier_worker()`, poll helper | All dynamic tier streaming logic |
| `viewer/mod.rs` | `Viewer` struct + `ApplicationHandler` + `run()` | Thin event-loop wiring only (~300 lines) |

---

## Step 3 — Tier-border blend zones (shader only)

The shader currently hard-switches between tiers: inside `hm5m_extent` → 5m texture, outside → base.
A visible seam is expected at the boundary.

Fix: add a blend zone in WGSL.
```wgsl
let t = smoothstep(inner_radius, outer_radius, dist_from_5m_centre);
let h = mix(h_5m, h_base, t);
// same for normals
```

No Rust changes needed — shader-only.
Do this at the same time as adding the 1m tier (no point blending two tiers if a third is imminent).

---

## Order of work

1. Refactor `mod.rs` into the four modules above.
2. Introduce `StreamingTier` abstraction inside `tiers.rs`.
3. Add 1m tier as a third `StreamingTier` (low code delta after steps 1-2).
4. Add blend zones in `shader_texture.wgsl` for all tier boundaries simultaneously.
