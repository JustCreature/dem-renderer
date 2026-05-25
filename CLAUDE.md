# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## Project Purpose

A real-time, learning-first 3D terrain renderer in Rust. The viewer raymarches real-world Digital Elevation Model data on the GPU, streaming up to three resolution tiers (coarse base / mid close / fine) and blending them in a single WGSL shader. The project doubles as a hardware-deep performance lab — every design decision has a measured backing (cache-line math, SIMD utilisation, TLB behaviour, PCIe readback floor, ROB / store-buffer limits) — but it is now a usable, generic terrain viewer that loads any GeoTIFF / SRTM file the user provides.

The original development used Austrian BEV data (5 m and 1 m) and Copernicus GLO-30 (30 m) because they were the easiest to test against. The CRS pipeline is now generic (proj4rs + proj4wkt + crs-definitions), and any EPSG-registered single GeoTIFF or arbitrary set of overlapping tiles is supported. The "Tirol demo view" preset remains a curated entry point, and its tile paths / camera coordinates can be overridden in the user config file.

---

## Debugging Tools

- **`cargo run --release -p dem_io --example inspect_geo -- <path/to/file.tif>`** — dumps a GeoTIFF's CRS-defining tags (GeoKeyDirectory 34735, GeoAsciiParams 34737, GeoDoubleParams 34736) and the proj4 string `dem_io::crs::tile_proj4` resolves them to. Reach for this first whenever a tile fails to load with a CRS error, or before adding any logic that depends on how a file encodes its CRS — the dump reveals which of the three discovery paths in `crs::proj4_from_keys` (WKT → inline GeoKey-encoded projection → EPSG lookup) will actually fire. Source: `crates/dem_io/examples/inspect_geo.rs`.

---

## Architecture

### Two-Phase Application

`src/main.rs` owns a single `App { phase: Phase, vsync_override: bool }` where `Phase::Launcher(LauncherApp) | Phase::Viewer(Viewer)`. The launcher and viewer share **one window, one GPU device, one wgpu surface** for the whole process lifetime — switching phases never calls `el.exit()` and never drops the surface, so there is no visible flash during the transition. The launcher produces a `LauncherOutcome::Start { window, settings, prepared, surface }` which the main App passes into `Viewer::from_launcher(...)`.

### Launcher (egui-based UI)

| File | Purpose |
|---|---|
| `src/launcher/mod.rs` | `LauncherApp` (ApplicationHandler), screen dispatch, load/download polling, GPU context creation |
| `src/launcher/config.rs` | `LauncherSettings` (TOML-persisted), `DemoViewConfig`, `SelectedView` (None / DemoView / CustomFile), `LauncherOutcome`, `VramBudget` (Low/Mid/High → `VramClass`) |
| `src/launcher/renderer.rs` | `EguiRenderer` — egui-winit + egui-wgpu integration, font registration (Space Grotesk, JetBrains Mono), `mountain-bg.png` texture |
| `src/launcher/background.rs` | Background painters (image, gradient, vignette, corner marks, metadata labels) |
| `src/launcher/style.rs` | Color palette + font helpers (mono, prop, prop_medium) |
| `src/launcher/widgets.rs` | Custom widgets: `menu_row`, `choice_item`, `segmented_control`, `styled_checkbox`, `dropdown`, `info_tooltip_button`, `breadcrumb`, `brand_block`, `status_footer`, `hairline_rule` |
| `src/launcher/downloader.rs` | Background HTTP Range-resume downloader (ureq); ships the demo bundle (4× Copernicus GLO-30 + DGM_R5 5 m + 2× CRS3035 1 m) |
| `src/launcher/screens/main_menu.rs` | Main screen — Select DEM / Settings / Start / Exit |
| `src/launcher/screens/select_dem.rs` | Choose source — file picker (any `.tif`) or "Recommended demo view" with download modal |
| `src/launcher/screens/settings.rs` | Overall Quality, Level of Detail, Shadows, Fog, AO mode, VRAM Budget (writes to `LauncherSettings`) |
| `src/launcher/screens/loading.rs` | Progress bar shown while terrain is being prepared on a background thread |
| `src/launcher/screens/download_card.rs` | Floating download progress card with animated radial ring + speed EMA |

Settings persist to `dirs::config_dir() / dem_renderer / config.toml` (macOS: `~/Library/Application Support/dem_renderer/config.toml`). The `demo_view` sub-table overrides camera position and tile paths for all three tiers, so the demo view does not have to be Tirol — point `fine_tile_paths` / `close_tile_paths` / `base_tile_paths` at any GeoTIFF set and the renderer will stream from it.

### Viewer

| File | Purpose |
|---|---|
| `src/viewer/mod.rs` | `Viewer` (ApplicationHandler), WASD+mouse, sun animation, tile streaming dispatch, key bindings |
| `src/viewer/scene_init.rs` | `prepare_scene_with_ctx` (single-tile / projected-CRS streaming), `prepare_demo_scene_with_ctx` (N×M Copernicus base, camera-centered crop to `GPU_SAFE_PX`), `compute_ao_cropped` |
| `src/viewer/tiers.rs` | `StreamingTier` (drift-detected reload), `BevBaseState` (base/close/fine workers), `TierRadii` + `tier_radii(VramClass)` preset mapping, `cross_crs_world_origin_and_extent`, `select_ifd`, `cap_to_gpu_limit` |
| `src/viewer/tile_index.rs` | `TileEntry` + `TileIndex` — discover WGS84 bounds of multiple tiles per tier; `tiles_overlapping_wgs84` |
| `src/viewer/geo.rs` | `latlon_to_tile_metres` (handles both geographic and projected CRSes), `sun_position` |
| `src/viewer/hud_renderer.rs` | glyphon HUD overlay, sun indicator, settings panel |
| `src/viewer/shader_hud_bg.wgsl` | HUD background shader |
| `src/viewer/shader_sun_hud.wgsl` | SDF season/time circles for the sun HUD |
| `src/consts.rs` | `WINDOW_W/H`, `DEFAULT_CAM_LAT/LON/ELEV`, `DEFAULT_TILE_5M_PATH`, `M_PER_DEG`, `GPU_SAFE_PX = 8192` |

### Workspace Structure

```
dem_renderer/
├── Cargo.toml
├── build.rs
├── Makefile                                # build / view / config / download-tiles
├── README.md
├── CLAUDE.md
├── menu.html                               # initial UI design mock-up (egui menu)
├── ui-extra.md                             # implementation deviations + next steps for launcher UI
├── download_copernicus_tiles_30m.sh        # 3×3 grid script
├── download_copernicus_tiles_30m_5x5.sh    # 5×5 grid script
├── assets/
│   ├── mountain-bg.png                     # launcher background photo
│   └── fonts/                              # SpaceGrotesk-Regular, JetBrainsMono-{Light,Regular}.ttf
├── src/
│   ├── main.rs                             # Phase state machine (Launcher ↔ Viewer)
│   ├── consts.rs
│   ├── system_info.rs
│   ├── utils.rs
│   ├── launcher/                           # See "Launcher" table above
│   └── viewer/                             # See "Viewer" table above
├── crates/
│   ├── dem_io/src/
│   │   ├── lib.rs
│   │   ├── heightmap.rs                    # Heightmap (f32 data), parse_bil, fill_nodata, fill_nodata_from_base
│   │   ├── geotiff.rs                      # parse_geotiff_auto, extract_window, ifd_scales, tile_bounds_wgs84, tile_centre_crs
│   │   ├── grid.rs                         # assemble_grid (N×M), load_grid_from_paths, crop, stitch_windows[_geographic]
│   │   ├── crs.rs                          # tile_proj4 / to_wgs84 / from_wgs84 / is_geographic / epsg_towgs84 / read_raw_crs_tags (proj4rs + proj4wkt + crs-definitions; WKT → inline-GeoKey → EPSG fallback chain)
│   │   ├── overview.rs                     # ensure_overview_cache: build .tmp_dem_pre_calc_*.tif from large single-IFD tiles (copies source CRS tags verbatim so cache is self-describing)
│   │   └── examples/inspect_geo.rs         # debug tool: dump a tile's GeoKey tags + resolved proj4 (see "Debugging Tools" above)
│   ├── terrain/src/
│   │   ├── lib.rs                          # Platform dispatchers (#[cfg(target_arch)] guards for AVX2 / NEON)
│   │   ├── row_major.rs                    # scalar + NEON normals
│   │   ├── row_major_avx2.rs               # AVX2 normals (x86_64 only)
│   │   ├── shadow.rs                       # scalar + NEON shadow DDA
│   │   └── shadow_avx2.rs                  # AVX2 shadow DDA (x86_64 only)
│   ├── render_gpu/src/
│   │   ├── lib.rs                          # public exports + CPU-side helpers (hm_to_f16_bytes, gen_hm_mip_bytes, pack_normals_*, pack_ao_u8)
│   │   ├── context.rs                      # GpuContext (Arc-backed Device/Queue, Instance, Adapter, VramClass); OOM atomics + on_uncaptured_error
│   │   ├── vram.rs                         # AtomicU64 alloc/drop accounting; create_*_tracked wrappers around wgpu allocations
│   │   ├── camera.rs                       # CameraUniforms — std140-aligned struct mirrored in WGSL
│   │   ├── vector_utils.rs
│   │   ├── render_rexture.rs
│   │   ├── shader_texture.wgsl             # main compute raymarcher (3-tier blend, AO, fog, LOD, bicubic Catmull-Rom)
│   │   └── scene/
│   │       ├── mod.rs                      # GpuScene::{new, resize, update_heightmap, update_shadow, update_ao, dispatch_frame}; make_tier_size_placeholders
│   │       ├── bind_group.rs               # rebuild_bind_group — 20-entry canonical BG (binding 0–19)
│   │       └── tiers.rs                    # upload_hm5m / upload_hm1m / set_hm5m_inactive / set_hm1m_inactive (drop-first eager dealloc + device.poll(Wait))
│   └── profiling/src/lib.rs                # cntvct_el0 / rdtsc cycle counters, CSV emit
├── tiles/                                  # gitignored; user-supplied DEM tiles
│   ├── Copernicus_DSM_COG_10_N*_00_E*_00_DEM/
│   └── big_size/                           # large user-downloaded GeoTIFFs (5 m / 1 m / Norway / NZ / …)
├── n47_e011_1arc_v3_bil/                   # gitignored; SRTM BIL legacy source
└── docs/
    ├── DEM Renderer.zip                    # design ZIP (high-level project archive)
    ├── screenshot.png
    ├── benchmarks_report.html
    ├── menu.html / time_season_hud_concept.png / gem-fixed-misalignment.md / misalignment.md / other_tiles.md
    ├── planning/                           # active plans: performace-improvements.md, tile-processing-overlapping.md, ui-downloader.md
    ├── gems/                               # design notes
    ├── learnings/
    ├── improvements/
    └── sessions/                           # per-phase session restore points
                                            #   vgpu-oom-crash-phase-{1..5}.md — issue #33 OOM fix walkthrough
                                            #   base-tier-r16float-stairs.md — known artefact (R16Float quantisation)
```

`docs/vram-limitation.md` at the repo root documents why Windows "Shared GPU Memory" can't be used as a wgpu allocation target — useful reference whenever someone asks why we don't just spill the fine tier into system RAM.

### Dependency DAG

```
profiling (leaf)
    ↑
dem_io  ─ proj4rs · proj4wkt · crs-definitions · tiff · image
    ↑
terrain ─ rayon
    ↑
render_gpu ─ wgpu · half · bytemuck
    ↑
  main.rs / src/launcher / src/viewer
        ─ winit · wgpu · egui (+ egui-wgpu + egui-winit) · glyphon · rfd · ureq · dirs · serde · toml · sysinfo · rayon · image · bytemuck
```

Types are defined in the crate that produces them: `Heightmap` in `dem_io`, `NormalMap`/`ShadowMask` in `terrain`, `GpuScene`/`GpuContext`/`CameraUniforms` in `render_gpu`.

### Crate Responsibilities

- **`dem_io`** — Read SRTM `.hgt`/`.bil` and any GeoTIFF (including BigTIFF). `parse_geotiff_auto` resolves the CRS through `tile_proj4`'s three-path discovery (WKT in tag 34737 via proj4wkt → inline GeoKey-encoded projection from `ProjCoordTransGeoKey` + `GeoDoubleParams` → EPSG code in 3072/2048 via crs-definitions) — no hardcoded CRS knowledge, and the inline path covers files like PGC's HMA mosaics where 3072 is the user-defined sentinel 32767 and the projection lives entirely in inline GeoKeys. `extract_window` performs selective COG reads at a chosen IFD level. `assemble_grid` builds an N×M mosaic from `&[Vec<Option<&Heightmap>>]`; `load_grid_from_paths` derives the bounding box `(max_lat-min_lat+1) × (max_lon-min_lon+1)` from whichever tiles the caller supplies and fills missing cells with zeros. `stitch_windows_geographic` and `stitch_windows` stitch BEV / projected windows from multiple overlapping tiles. `ensure_overview_cache` builds a `.tmp_dem_pre_calc_<filename>.tif` next to any large single-IFD tile (box-averaged to ~8 m and ~32 m levels) so subsequent runs and tier reloads stay fast; the cache copies the source's GeoTIFF CRS tags (34735/34736/34737) verbatim via `read_raw_crs_tags`, so it remains self-describing for any source CRS — including the inline-GeoKey ones that have no single EPSG code. `fill_nodata_from_base` smooth-blends a higher-resolution window over a coarser base so seams between tiers disappear.
- **`terrain`** — Surface normals (Sobel SoA, NEON 4-wide and 8-wide on aarch64, AVX2 8-wide on x86_64). DDA shadow sweep with arbitrary azimuth and penumbra (scalar / NEON / AVX2 variants, rayon-parallel). True-hemisphere AO (`compute_ao_true_hemi` — 16-azimuth DDA averaged).
- **`render_gpu`** — wgpu compute-shader raymarcher. `GpuScene` owns all GPU resources persistently; mutable per-frame work is the camera uniform write and an optional shadow/AO/heightmap buffer/texture refresh. Three tiers are blended in WGSL with a 500 m blend margin. Bicubic Catmull-Rom interpolation is enabled within `smooth_radius_m` of the camera (default 2000 m, `B` key cycles 0/500/1000/2000/5000 m). Public helpers `hm_to_f16_bytes`, `gen_hm_mip_bytes`, `pack_normals_u32_bytes`, `pack_normals_rg16_bytes`, `pack_ao_u8` let workers pre-pack bytes off the main thread. Tier reloads follow a drop-first cycle: swap to 1×1 placeholders → `rebuild_bind_group` → `device.poll(PollType::Wait)` to drain wgpu's destroy queue → allocate new → second `rebuild_bind_group`. `vram.rs` tracks every allocation through an `AtomicU64`; `context.rs::on_uncaptured_error` sets an `OOM_OBSERVED` flag that the viewer polls each frame to disable the fine tier (then the close tier) instead of crashing.
- **`profiling`** — `cntvct_el0` (AArch64) / `rdtsc` (x86) cycle counters, CSV timing emitter.

### Multi-tier Streaming Model

For projected single-file mode and demo mode, the viewer spawns three background workers that hold WGS84 `(lat, lon)` and translate per-tile. Radii and drift thresholds come from the active `VramClass` preset (set via the launcher's VRAM Budget dropdown — defaults to `Mid`); the table below shows the `High` preset (full radii, used on Apple Silicon / 8 GB+ discrete):

| Tier | Radius (High / Mid / Low) | Drift threshold (High / Mid / Low) | Format on GPU |
|---|---|---|---|
| **base** | 90 / 70 / 50 km | 30 / 23 / 17 km | R16Float texture + 8 mips; packed-u32 normal storage buffer; R8Unorm AO; f32 shadow buffer |
| **close** (≈5 m) | 20 / 14 / 8 km | 3 / 2 / 1.5 km | R32Float texture; Rg16Snorm normal texture; f32 shadow buffer |
| **fine** (≈1 m) | 3.5 / 2.5 / 1 km | 1 / 0.8 / 0.3 km | R32Float texture; Rg16Snorm normal texture; f32 shadow buffer |

`StreamingTier::needs_reload` fires when the camera drifts past the threshold; the worker re-reads via `extract_window`, recomputes normals/shadows on the worker thread, pre-packs all GPU bytes, and sends a `TierData` bundle. The main thread does only `write_texture` / `write_buffer`. Detail tiers are suppressed while the camera moves > 2 500 m/s (400 ms debounce) — a 20 km close window is pointless when the camera leaves it before the load finishes. The base tier also recalibrates its drift threshold to half the actual loaded window after each reload (so GPU-capped windows on 1 m sources stay reload-stable). The runtime OOM handler can zero `fine_radius_m` (and then `close_radius_m`) in place if a reload spike trips wgpu's allocator anyway — the bev_base.fine `Option` becomes `None`, the close worker is gated behind `close_tier_disabled`, and the HUD shows a red warning banner.

### Bind Group Layout (Single Canonical BG, 20 Entries)

| Binding | Resource |
|---|---|
| 0 | `CameraUniforms` (Uniform) |
| 1 / 2 | Base hm `texture_2d<f32>` (R16Float, 8 mips) + filtering sampler |
| 3 | Output buffer (`storage, read_write` u32 array) |
| 4 | Packed normals (`storage, read` u32 array) |
| 7 | Base shadow (`storage, read` f32 array) |
| 8 / 9 | AO texture (R8Unorm) + sampler |
| 10–14 | 5 m close tier — heightmap (R32Float) + samp + normals (Rg16Snorm) + samp + shadow buffer |
| 15–19 | 1 m fine tier — same layout as 5 m |

`scene::bind_group::rebuild_bind_group()` rebuilds the BG whenever a tier's texture / buffer is recreated (size grows). Steady-state reloads do not allocate — `write_texture` / `write_buffer` overwrite in place.

---

## Launcher UI — Development Rules

These rules are derived from real mistakes made during launcher development. Follow them strictly.

### Component-first: extract before writing inline

Before writing inline egui layout code, check whether `src/launcher/widgets.rs` already has a component that fits. If similar inline code already exists in a screen file, extract it into a widget immediately — don't leave a `// NOTE: Extract it to a component later` comment. The available widgets are:

| Widget | Purpose |
|---|---|
| `small_button(ui, label, ButtonVariant)` | Small inline action button (22 px tall), painter-drawn with hover |
| `main_button(ui, label, ButtonVariant)` | Large modal / confirmation button (38 px tall), painter-drawn with hover |
| `copy_icon_button(ui)` | Square icon button with painter-drawn copy icon |
| `text_area(ui, id, text, editable)` | Dark-framed monospace text block; selectable when `editable: false`, persistent TextEdit when `true` |
| `ButtonVariant` | `Primary` / `Secondary` / `Apply` / `Reject` — shared by both button sizes |
| `segmented_control`, `dropdown` | Settings-row controls |
| `choice_item`, `menu_row` | Animated card / row with hover slide |
| `hairline_rule`, `breadcrumb`, `brand_block`, `status_footer` | Structural / chrome widgets |

### Hover: always painter-drawn, never egui::Button

Use `ui.allocate_painter` + `Sense::click()` for any button that needs custom hover colors. `egui::Button` only applies egui's own hover overlay, which does not respect our custom fill colors and produces inconsistent results. Always follow the pattern in `small_button`: allocate → check `response.hovered()` → paint fill, stroke, then galley.

To make text color change on hover, lay out the galley with `Color32::PLACEHOLDER` (not the real color) so `painter.galley(pos, galley, fallback_color)` can switch the color each frame without re-laying-out.

### Preserve existing style exactly when replacing inline code

When replacing inline egui widgets with a shared component, verify the visual output matches before and after. Key differences that break style:
- `egui::Button` defaults to `fill(ui.visuals().widgets.inactive.bg_fill)` unless overridden — always set `.fill()` explicitly
- Transparent-fill buttons (`fill(Color32::TRANSPARENT)`) look different from our dark-bg `small_button` — check which the design calls for
- Font size and weight must match: `prop_medium` ≠ `prop`, `mono` ≠ `prop`

### Uniform panel sizing — no per-screen heights

All launcher screens must use the same `panel_top` and `min_height` values. Per-screen heights cause a visible jump when navigating between screens. If a new settings row makes the panel too small, increase the shared values for all screens together.

### Font coverage — use painter-drawn icons

The loaded fonts are **Space Grotesk** (proportional) and **JetBrains Mono** (monospace). Neither covers all Unicode. Special symbols such as `⎘` (U+2398) or uncommon arrows render as `?`. Use painter-drawn icons (`allocate_painter` + rect/line/circle primitives) for any UI icon that isn't a standard ASCII character or a common arrow (`→`).

### egui state persistence for editable widgets

egui redraws every frame. Any `String` (or other value) computed fresh each frame will overwrite user input. For editable state, store it in egui's temp data under a stable `Id`:

```rust
// Read or initialise
let mut buf = ui.ctx().data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| initial.to_string());
// … show TextEdit against &mut buf …
// Write back every frame
ui.ctx().data_mut(|d| d.insert_temp(id, buf));
```

This is how `text_area(ui, id, text, editable: true)` works internally.

### Long paths — truncate with tooltip

Displaying a full absolute path in a row or label will overflow into adjacent text. Show only the leaf segment (`…/tiles`) as the label and put the full path in `.on_hover_text(full_path)`. See the "Tiles directory" row in `src/launcher/screens/settings.rs` for the reference implementation.

---

## Interaction Mode

- **Guide.** The user is building this to learn. Explain *why* something works at the hardware level, point to the right direction, suggest experiments — but do not write code or execute commands unless explicitly asked.
- **Assume strong technical curiosity.** The user wants full-depth explanations: cache-line math, TLB reach, ROB/store-buffer reasoning, branch predictor behaviour. Don't simplify unless asked.
- **Encourage measurement over intuition.** "Profile it — here's how and what counters to look at" is almost always the right answer to "which is faster?".
- **Build layered mental models.** Start from the hardware constraint (cache size, SIMD width, pipeline depth), derive the software implication, then suggest the experiment to validate.
- **Go full hardware depth.** Store buffers, ROB size, retirement rate, branch predictor internals (TAGE), TLB pressure, prefetcher training, port pressure — not just "use SIMD and cache lines".

---

## Key Measurement Results (M4 Max unless noted)





### Multi-tile loading (10800×10800 assembled GLO-30 grid)
- load_grid (9 × DEFLATE COG from disk): 4.52 s | normals: 185 ms | shadows: 525 ms
- AO full grid (16-azimuth DDA): 7.81 s | AO cropped (20 km radius): 290 ms — **27× speedup**
- `extract_window` (5m BEV DGM, 5 km radius, cold): **18.6 ms** — ~64 tiles read out of ~128,000 (0.05% of file)

### Cross-system (Win Nitro i5+GTX1650 / Mac Intel i7 / Asus Pentium N3700)
- Auto-vec penalty universal: 6.5–10× on every machine (same root cause, different ISA)
- Write/read asymmetry: M4 0.40 | Mac i7 0.26 | Asus 0.33 | Win 0.16 (write-allocate RFO)
- TLB: x86 exhausts at 1 MB (256 × 4 KB); M4 exhausts at 4 MB (256 × 16 KB)
- GTX1650: compute ~20 ms, PCIe readback ~47 ms → fps ceiling is PCIe BW, not shader throughput

---

## Key Lessons Learned

### Vectorization
- A single `continue` in the inner loop cuts throughput 6× regardless of ISA, tile size, or thread count
- Compiler auto-vectorization is powerful but fragile — one control-flow escape gates everything
- Tiling helps input reads but hurts output writes when output layout doesn't match iteration order

### Memory layout
- `get()` abstraction overhead dominates in tight loops — must use direct tile pointer arithmetic to see tiling benefit
- Write path saturates at fewer threads than read path on every machine (RFO + store buffer)
- M4 16 KB pages give 4× TLB reach vs x86 — critical at large working sets (26 MB heightmap)
- Morton ordering needs DRAM pressure to matter; OOO ROB hides the L2 latency difference

### wgpu specifics
- Bind groups store GPU addresses, not CPU-side Arc refs — all referenced resources must be kept alive in the owning struct
- `write_buffer` updates buffer contents in-place; bound bind group sees new data automatically on next dispatch
- Default buffer binding limit 128 MB; fix: `required_limits: adapter.limits()`
- Texture dimension limit 8192 px (hardware max, not wgpu default) — `GPU_SAFE_PX = 8192`, source windows above this are cropped centred on the camera
- wgpu does not expose VkSparseBinding or Metal sparse textures; software indirection is the only option
- Workgroup size (64–256 threads, 8×8 to 32×8): all within ±3% when readback dominates
- Dropping a `wgpu::Texture` Rust handle is **not** enough to free GPU memory — the BindGroup keeps an internal `Arc<TextureInner>` until the BindGroup itself is rebuilt AND the next submission retires. `device.poll(PollType::Wait { submission_index: None, timeout: None })` is the only way to drain the destroy-after-submission queue synchronously; without it, a reload sees `old + new` GPU memory peak instead of `max(old, new)`. Safe to call from reload paths (they already cause a perceptual hitch); never call from the steady-state frame loop (blocks until the next submission completes ≥ 16 ms)
- wgpu 0.29 changed `device.on_uncaptured_error` to take `Arc<dyn UncapturedErrorHandler>` (was `Box<dyn Fn(Error) + Send + Sync>` in 0.20). Handler runs on wgpu's internal thread — must not block or allocate; safe pattern is an `AtomicBool` flag the frame loop polls
- wgpu does not expose VRAM capacity per device. Detection is heuristic: `adapter.get_info().name` substring match for known-tiny SKUs, `device_type` fallback, Apple Silicon detection. The Windows "Shared GPU Memory" pool is a driver-internal eviction target, not an application-visible allocation heap — see `docs/vram-limitation.md`

### GeoTIFF / CRS
- CRS is read from each tile, not assumed: `tile_proj4` has a three-path discovery chain — (1) WKT from tag 34737 via proj4wkt, (2) inline GeoKey-encoded projection synthesised from `ProjCoordTransGeoKey` (3075) + parameter keys (3078–3095) + `GeoDoubleParams` (34736), (3) EPSG code in GeoKey 3072/2048 via crs-definitions. proj4rs handles the transforms in both directions
- GeoKey value 32767 is the GeoTIFF spec sentinel for "user-defined", **not** a real EPSG code — `read_geo_key_data` excludes it from `projected_epsg`/`geographic_epsg`, otherwise the EPSG fallback would call `crs_definitions::from_code(32767)` and fail. Older GeoTIFF encoders (PGC's HMA pipeline, some USGS / NOAA products) sit `3072 = 32767` and put the projection entirely in inline GeoKeys + `GeoDoubleParams`, which is what discovery path 2 above is for
- proj4wkt defaults to `+towgs84=0,0,0,0,0,0,0` for any WKT without an explicit TOWGS84 node; `epsg_towgs84()` overrides that with 7-parameter Helmert shifts for MGI (Austria), DHDN (Germany), OSGB36, ED50, CH1903 (Switzerland), Tokyo, NZGD49 — without these the Austrian 5 m tile sits ~600 m east of the 30 m Copernicus grid
- A geographic tile stores `dx_meters = dx_deg * 111 320 * cos(lat)`; `extract_window` writes `dx_meters = dx_deg` for geographic tiles (degrees as pixel scale), so all viewer/tier code derives m/px from `dx_deg` for geographic CRSes and reads `dx_meters` directly for projected ones
- BEV DGM 5 m NoData sentinel = 0.0 (safe: min Austrian elevation >> 0); GLO-30 NaN/<-1000 sentinel; the extract_window NoData sentinel is -9999
- Pixel-scale tag value distinguishes geographic CRS (<0.1 deg/px) from projected (≥1.0 m/px) at load time
- `tiff` crate default memory limit blocks tiles > 128 MB; fix: `Limits::unlimited()`
- Tile geometry at mid-latitudes is asymmetric: E-W width shrinks with cos(lat)
- GLO-30 tiles: 3600×3600 pixel-is-area, pixel centres at ±0.5/3600° from integer degree boundary; adjacent tiles concatenate directly
- For large single-IFD tiles, `ensure_overview_cache` builds box-averaged pyramids (target ~8 m close + ~32 m base) into a `.tmp_dem_pre_calc_<filename>.tif` next to the source — checked by mtime so subsequent runs hit the cache and base / close tier reloads stay fast. The cache writer copies the source's GeoKeyDirectory (34735), GeoDoubleParams (34736) and GeoAsciiParams (34737) verbatim via `read_raw_crs_tags`, so the cache is self-describing for any CRS encoding — a minimal `3072=<epsg>` directory like the original implementation would discard the inline-GeoKey projection on cache reload and corrupt CRS interpretation

### Multi-resolution tiers
- True Hemisphere AO = sun shadow DDA generalised: 16 azimuths, averaged — baked once, free at render time
- HBAO radial 600m sweep exposes smaller GPU caches (GTX 1650); SSAO fixed-offset samples stay cache-local
- C1 discontinuity (slope jumps at DEM grid lines) is a data floor; bicubic Catmull-Rom inside `smooth_radius_m` softens fine-tier surfaces without destroying ridgelines; Gaussian smoothing destroys ridgelines
- At 47°N: SRTM tiles are 111 km N-S × 76 km E-W; sea-level fog 60 km overshoots E/W edges → 3×3 (or 2×2) tile grid required. Fog far-distance now scales with camera altitude as `exp(alt / 8000)` capped at 6× (≈360 km at ≥14 km), so high-altitude views need a base radius the streamer can actually load — the 6× cap deliberately matches the largest base-tier radius (90 km High preset) so fog never pushes past the loaded grid
- The fine and close tiers carry an explicit per-tier rotation (`cos_rot`/`sin_rot` in `CameraUniforms`) so meridian convergence between projected CRSes (e.g. 1 m EPSG:3035 over a 30 m geographic base) does not produce a visible seam

### Main-thread responsiveness (fix-freeze)
- All CPU-heavy pre-upload work (`hm_to_f16_bytes`, `gen_hm_mip_bytes`, `pack_normals_u32_bytes` / `pack_normals_rg16_bytes`, `pack_ao_u8`) is now performed on tier worker threads; the main thread only calls `write_texture` / `write_buffer`. Removing the f32→f16 pass and the mip generation from the GPU upload path eliminated multi-frame stalls on tile slides
- The base tier reload also respawns shadow and AO workers because both close over `Arc<Heightmap>` — replacing the heightmap requires respawning their senders/receivers, and AO is force-recomputed at the new tile centre by setting `ao_last_x = f64::MAX`

### Latent assumptions in data sizes
- The same shape of bug almost certainly exists elsewhere in the codebase: any other place where a dimension, a count, or a stride is hardcoded to a value that "always works" for the data the author tested with. Worth keeping the smell in mind when reading code in this repo — especially around streaming and GPU upload paths.
- Example precedent: `scene_hm_tex` had `mip_level_count: 8` hardcoded. This held for every Tirol / Copernicus dataset (windows ≥ 128 px on the long axis → `log2(128)+1 = 8`), but crashed wgpu validation the moment a tile like Diamond Head (a 3.6 km × 3.3 km NOAA Oahu LiDAR extract) made the base tier walk to its coarsest overview (115×105 → max 7 mips). Fix: derive the count from actual cols/rows via `render_gpu::hm_mip_count(cols, rows)`, capped at 8.
- Rule of thumb when adding a new GPU resource or upload path: if a literal integer appears next to a `size:` / `mip_level_count:` / `bytes_per_row:` / `rows_per_image:` / `array_layer_count:` field, ask "what shape of input invalidates this?" before committing it.

---

## Open Items

- `fill_nodata` division-by-zero if all 4 directions hit boundary without finding valid data
- Supersampled ray optimization: march 1 reference ray, approximate 3 neighbours via gradient. Breaks at sharp peaks.
- Dynamic tile list in the Select DEM screen (planned `src/launcher/scanner.rs` to discover `tiles/` contents and feed the UI live)
- Custom configure-view UI (let the user point each tier at arbitrary GeoTIFFs without editing the TOML by hand). When this lands, `TierRadii` should become a public serde-derived struct embedded per-view so each saved view carries its own radii instead of inheriting the global `vram_budget`.
- Base-tier R16Float quantisation at high elevations causes visible "amphitheater" stairs above ~2 km — see `docs/sessions/base-tier-r16float-stairs.md`. Most plausible fix: CPU-side dither before f16 conversion.
- Issue #40 — Windows-only failure when reading some LZW-compressed GeoTIFFs (NZ LINZ LiDAR). `tiff-0.11.3`'s LZW reader rejects strips that don't end with the EOI code; macOS reads the same bytes fine. Plan in the issue is a lenient `weezl`-driven reader bypassing `BufReader<File>`.

---

## Build Commands

```sh
cargo build --release                                         # native (target-cpu via Makefile)
RUSTFLAGS="-C target-cpu=native" cargo build --release        # explicit AVX2 / NEON
make build_arm                                                # macOS Apple Silicon
make build_x86                                                # cross-compile to x86_64-apple-darwin (run on an Intel host)
```

**Build profiles** (workspace `Cargo.toml`):
```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
```

Use `#[inline(never)]` during profiling so functions appear as distinct symbols. Switch to `#[inline]` + LTO for final benchmark numbers.

---

## Key Design Decisions

| Decision | Rationale |
|---|---|
| Generic CRS via proj4rs/proj4wkt | Any GeoTIFF with a CRS works; no per-EPSG branches in the load path |
| Three-path CRS discovery (WKT → inline GeoKey → EPSG) | Some encoders (PGC HMA, USGS/NOAA legacy) put the projection inline via ProjCoordTransGeoKey + GeoDoubleParams with 3072=32767 sentinel — neither WKT nor EPSG lookup helps; the inline-synthesis path handles them without per-vendor code |
| Overview cache copies source CRS tags verbatim | Cache stays self-describing for any CRS, including inline-GeoKey projections that have no single EPSG code |
| Built-in epsg_towgs84 Helmert shifts | proj4wkt zero-fills TOWGS84 for WKT without it; manual override prevents 600 m datum offsets on national grids |
| Pre-built overview cache for large single-IFD tiles | One slow build instead of slow tier reloads on every run |
| Two-phase App with shared surface | Launcher and viewer use one window/device/surface — zero visible flash at startup |
| TOML config in `dirs::config_dir()` | Survives reinstalls; demo_view paths can be edited by hand for any region |
| SoA over AoS for normals | Load 8 consecutive nx values in one AVX2 instruction |
| Branchless inner loops | SIMD masks / `cmov` in shadow sweep, ray termination |
| Worker-side byte packing | Main thread never spends time converting f32→f16 / scaling AO / packing normals |
| Single canonical 20-entry bind group | `rebuild_bind_group()` runs only when a tier texture/buffer is resized |
| Drop-first tier reload cycle | Swap fields to 1×1 placeholders → `rebuild_bind_group` → `device.poll(PollType::Wait)` → allocate new → second `rebuild_bind_group`. Reload peak goes from `old + new` to `max(old, new)` because the BindGroup holds an `Arc<TextureInner>` until rebuild + poll |
| `set_hm*_inactive` actually frees | Same drop-first sequence (no real alloc on the other end). Used by the base-reload completion path and by the OOM degradation step — neither of which would ever return VRAM under the previous "set extent_x to 0" approach |
| VRAM budget presets (Low / Mid / High) | `TierRadii` per `VramClass`; launcher dropdown persists the choice. Adapter-detected class is informational only — the user always picks |
| OOM safety net via `on_uncaptured_error` | wgpu panics from a worker thread by default. The handler sets an atomic; the frame loop polls and steps down (disable fine tier → disable close tier → no-op). HUD shows a red banner when the path has fired |
| Allocation tracker in `vram.rs` | `AtomicU64` counters via `create_*_tracked` / `track_*_drop` wrappers. Lets us validate the drop-first ordering on a high-VRAM machine where the bug doesn't manifest naturally |
| `GPU_SAFE_PX = 8192` cap | Hardware texture dimension limit; oversized source windows are cropped around the camera |
| Speed-gated detail loads | At boost speed (5 km/s) a 20 km close window outlives its load time; gate at 2.5 km/s with 400 ms debounce |
| CPU shadow, GPU render | Running-max is serial → CPU wins; raymarching embarrassingly parallel → GPU wins |
| Swap-chain viewer | Eliminates 85 MB readback floor; PCIe was the fps bottleneck, not shader compute |
| Multi-resolution tiers | 30 m / 5 m / 1 m blended in shader with a 500 m feathered margin; bicubic Catmull-Rom near camera |
| Per-tier `cos_rot`/`sin_rot` | Compensates meridian convergence between projected CRSes at tier boundaries |
| Altitude-aware sky + fog | `sky_color(dir.z, cam.origin.z)` in `shader_texture.wgsl` runs a horizon→zenith gradient whose zenith colour is interpolated through two stages (sea level → ~3 km alpine → ~10 km near-space). Fog far-distance is multiplied by `exp(alt / 8000)` capped at 6×, matching atmospheric scale-height physics. Same `sky_color` call drives both the open-sky branch and the distance-fog tint so haze in the distance matches the overhead tone |
| N×M demo grid + crop | `assemble_grid` and `load_grid_from_paths` derive grid shape from the supplied tile set's bounding box (was hardcoded 3×3); `prepare_demo_scene_with_ctx` applies `cap_to_gpu_limit` immediately after assembly so any tile pool exceeding `GPU_SAFE_PX` (e.g. a 5×5 GLO-30 = 18000²) is cropped around the camera before normals/shadow/AO/upload. 3×3 demos are unaffected (cap is a no-op) |

---

## Coding Conventions

- Rust stable, `edition = "2024"`. Nightly only for `std::simd` / `core::arch` items not yet stabilised.
- `unsafe` only for SIMD intrinsics — document the safety invariant inline.
- Prefer `core::arch` over `std::simd` when stable intrinsics cover the operation.
- Name SIMD dispatch functions explicitly: `compute_normals_neon()`, `compute_normals_avx2()`, `compute_normals_vector()` (dispatcher).
- ISA-specific modules go behind `#[cfg(target_arch = "x86_64")]` / `#[cfg(target_arch = "aarch64")]` and the dispatcher logs `[SCALAR FALLBACK]` when neither path is available.

---

## Profiling

**Target hardware**: Apple Intel x86-64 (AVX2, 32–48 KB L1D) and Apple Silicon M4 Max (NEON, 128 KB L1D) and ACER WindowsOS Intel CPU Nvidea GTX 1650 GPU.
