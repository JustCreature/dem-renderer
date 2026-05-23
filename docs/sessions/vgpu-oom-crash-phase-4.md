# Phase 4 — Launcher UI for the VRAM budget override

**Issue:** [#33 — vRAM OOM Error, no shared RAM utilisation](https://github.com/JustCreature/dem_renderer/issues/33)
**Full plan:** `~/.claude/plans/curious-booping-rabbit.md`
**Prerequisite:** Phase 3 (`render_gpu::VramClass`, `viewer::tier_radii`).

## Purpose

Phase 3 added `VramClass::detect`, but a name-substring heuristic can't possibly stay current with every new SKU, and silent auto-downgrades are hard to debug ("why is my card running at Low?"). The shipped policy: **the user picks**, with `Mid` as the safe default. The auto-detected class is logged for context but never overrides the choice.

Reasons the user might pick each preset:

- **`High`** — Apple Silicon, 8 GB+ discrete: full radii, all three tiers.
- **`Mid`** (default) — most modern hardware, including high-end integrated (Iris Plus, Apple M-series, Vega 8) and 4–8 GB discrete: reduced radii with fine tier on.
- **`Low`** — 2 GB cards (MX150/250/350/450, old HD Graphics, RX 550/560, Arc A310/A380), 3 GB cards still tight after driver overhead (GTX 1050 / 1650 etc.), or "I want a smaller working set so my browser/Discord/IDE don't fight for VRAM": small base + small close + tiny 1 km fine right under the camera.

Single UI control. Persists across launches.

## Design

A 6th row in the launcher Settings screen, mirroring the existing pattern:

```
01 Overall Quality   [Ultra | High | Mid | Low]
02 Level of Detail   [Ultra | High | Mid | Low]
03 Shadows           [Off   | On  ]
04 Fog               [Off   | On  ]
05 Ambient Occlusion [Off ▾ ]
06 VRAM Budget       [Mid ▾]   ← new (default Mid)
```

Dropdown options: `Low (≤4 GB)`, `Mid (4–8 GB)`, `High (8 GB+)`. Labels include the approximate VRAM tier so the user has context for the choice without needing to read docs. Each variant maps 1:1 onto `render_gpu::VramClass::{Low, Mid, High}` via `VramBudget::to_class()`. The same value drives both load sites (`prepare_scene_with_ctx` initial extract, `Viewer::from_launcher` tier streaming) so there's no risk of one half running Low while the other thinks it's Mid.

## How the budget propagates

```
LauncherSettings.vram_budget: VramBudget        ← persisted in config.toml
        │
        ├─→ prepare_scene_with_ctx(..., vram_budget, ...)   (single-file mode init)
        │       vram_budget.to_class()
        │       → init_radii.base_radius_m
        │
        └─→ Viewer::from_launcher(..., vram_budget)
                vram_budget.to_class()
                → tier_radii(class) → self.tier_radii
                → BevBaseState::new(radii, ...) for both demo and single-file paths
```

`VramBudget::to_class() -> VramClass` is the central join point: one function, two callers, no conditional logic. The viewer never sees `VramBudget` again after `from_launcher`; everything downstream operates on `TierRadii`.

## Persistence behaviour

`LauncherSettings::save()` already writes the struct to `dirs::config_dir()/dem_renderer/config.toml` via `toml::to_string_pretty`. Adding `pub vram_budget: VramBudget` with `#[serde(default)]` and a `Default` impl that returns `VramBudget::Mid` means:

- Fresh install / missing config: `vram_budget = "Mid"` — safe default.
- Existing config from before Phase 4: the key is absent → `default()` fills in `Mid` → demo runs at the middle preset (a small downgrade from the High that pre-Phase-4 used, but the eager-dealloc work in Phase 2 makes it imperceptible).
- User changes the dropdown → settings struct updated → `save()` on launcher exit serialises the new value → next launch reads it back.

The TOML output looks like:

```toml
vram_budget = "Mid"
```

No magic numbers; the on-disk format is human-readable and editable.

## Why the budget had to thread through both load paths

There are two separate places where the radii matter:

1. **`prepare_scene_with_ctx` (single-file projected mode).** The initial `extract_window` reads a `base_radius` chunk of the source file. With a 90 km radius on a non-cached 10 GB BigTIFF this is 200 ms+ of IO; with 50 km it's 80 ms. Crucially, the extracted window determines the initial CPU-side `Heightmap` size, which is what gets uploaded as the first base texture. If we used Mid here but Low in the streaming workers, the first frame would have a 70 km base in memory, then immediately reload to 50 km — wasting the initial upload's IO.

2. **`Viewer::from_launcher` (both demo and single-file).** Every subsequent reload uses `self.tier_radii`. The close worker's `extract_window` reads `close_radius_m`; the fine worker is gated on `fine_radius_m > 0.0`; the base reload's drift threshold recalibration uses `self.tier_radii.base_drift_m`.

If only one of these saw the budget, the system would self-correct on the first reload but waste startup time. Threading the budget into both means the user sees the chosen preset from frame zero.

## Files & exact changes

### `src/launcher/config.rs`
- New `pub enum VramBudget { Low, #[default] Mid, High }` with serde derives.
- `impl VramBudget { pub fn to_class(self) -> VramClass }` — direct 1:1 mapping.
- New field `pub vram_budget: VramBudget` on `LauncherSettings`, `#[serde(default)]`.
- `LauncherSettings::default()` adds `vram_budget: VramBudget::Mid`.

### `src/launcher/screens/settings.rs`
- Import `VramBudget`.
- New `opt_row` "06 VRAM Budget" with a `dropdown` widget. The widget operates on `&mut u32` indices, so a small `match` converts to/from `VramBudget` around the call. The three labels include the approximate VRAM tier in parens.

### `src/launcher/mod.rs`
- `begin_loading` captures `self.settings.vram_budget` into the load thread.
- Passes it as a new argument to `prepare_scene_with_ctx`. (`prepare_demo_scene_with_ctx` doesn't take it because the demo's initial 3×3 base load is a fixed `load_grid_from_paths` regardless of radius — the radii only matter for streaming reloads.)

### `src/viewer/scene_init.rs`
- `prepare_scene_with_ctx` signature gains `vram_budget: VramBudget`.
- The initial `let init_radii = tier_radii(gpu_ctx.vram_class)` becomes `tier_radii(vram_budget.to_class())`. The adapter-derived `gpu_ctx.vram_class` is no longer consulted here — it's informational only.

### `src/viewer/mod.rs`
- `Viewer::from_launcher` signature gains `vram_budget: VramBudget`.
- The Phase 3 line `let tier_radii = tier_radii(scene.get_gpu_ctx().vram_class);` becomes `let tier_radii = tier_radii(vram_budget.to_class());`. The log line still prints the adapter-detected class alongside the chosen budget — useful for users filing bugs to see both at once.

### `src/main.rs`
- The `Viewer::from_launcher(...)` call in the `LauncherOutcome::Start` handler passes `settings.vram_budget`.

## Verification

1. **First launch on the M4 Max:** dropdown shows `Mid` (default). Log says `vram_budget=Mid (adapter detected=High)`. Demo runs at the Mid preset.
2. **Change dropdown to `Low`, quit, relaunch:** config.toml contains `vram_budget = "Low"`. Log says `vram_budget=Low (adapter detected=High)`. Base extracts at 50 km, close at 8 km, fine spawns at 1 km — visible as a small island of 1 m detail right under the camera.
3. **Change dropdown to `High`:** log says `vram_budget=High`; full 90/20/3.5 km radii. Matches the pre-Phase-3 behaviour on Apple Silicon.
4. **Delete config.toml entirely:** next launch defaults to `Mid`. No crash on missing field (the serde default catches it).
5. **Edit config.toml by hand to `vram_budget = "High"`:** the launcher reads it, the dropdown reflects High, the runtime applies the High preset.
6. **3 GB-class user (GTX 1050 / 1650 / 1660) accepting the `Mid` default:** the adapter detector doesn't downgrade these any more — they're tagged `Mid` and run at the Mid preset. If the OS / driver eats enough VRAM to push reload peak over the budget, the runtime OOM safety net disables the fine tier; the user can switch the dropdown to `Low` for the next launch.

## Not done in this phase

- **No hot-apply.** Changing the dropdown mid-flight does nothing; the budget is read once when the viewer is constructed. A future tweak could update `self.tier_radii` live and let the next reload pick it up. Not worth the complexity for now — every other quality setting in the launcher works the same way.
- **No tooltip.** The three label strings (with VRAM tier hints) are clear enough that an info-button tooltip would be redundant. The `opt_row_with_info` helper exists if a future change wants one.
- **No "recommended" badge based on detection.** The launcher could highlight the row matching the adapter-detected class to nudge the user toward a sane choice. Skipped — the Mid default is fine for almost everything, and bug-filers can include the stderr log line for context anyway.
