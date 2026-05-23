# Stair-stepping on distant slopes — base tier R16Float quantization

**Reported during:** Phase 2 verification of issue #33.
**Status:** Pre-existing, not a regression. Out of scope for the vRAM OOM fix; documented here so it doesn't get re-discovered later.

## What you're seeing

On Tirol peaks (3000–3500 m), at distances where the camera is too far for the close (5 m) tier to cover (> ~8 km), the slopes show horizontal terraces — "amphitheater stairs" of roughly 2 m vertical spacing. They become much more obvious at glancing-angle views from the air, where each f16 quantization level reads as a flat tier on the silhouette.

## Why

Heightmap → GPU pipeline:

| Tier | Source spacing | Storage format | Quantization step at 3500 m |
|---|---|---|---|
| Base | 30 m (Copernicus GLO-30) or 20 m (cached overview) | **R16Float** | ~2 m |
| Close | 5 m (BEV DGM_R5) | R32Float | ~3×10⁻⁴ m |
| Fine | 1 m (EPSG:3035) | R32Float | ~3×10⁻⁴ m |

f16 = IEEE 754 binary16: 1 sign + 5 exponent + 10 mantissa bits. Step size for values in `[2^n, 2^(n+1))` is `2^(n-10)`.

| Elevation | Range | Step size |
|---|---|---|
| 0–1024 m | `[2^9, 2^10)` ish | 0.5 m |
| 1024–2048 m | `[2^10, 2^11)` | 1.0 m |
| 2048–4096 m | `[2^11, 2^12)` | **2.0 m** |
| 4096–8192 m | `[2^12, 2^13)` | 4.0 m |

Tirol summits sit squarely in the 2 m step band. On a 30 m grid, a 2 m vertical step quantization reads as a ~0.7° slope discontinuity — small but visible against the sun, especially on smoothed Catmull-Rom interpolation between samples.

## Historical context

Commit `95ee2b5` (2026-05-07) fixed this exact problem for the close and fine tiers:

```
crates/render_gpu/src/scene/tiers.rs
-                format: wgpu::TextureFormat::R16Float,
+                format: wgpu::TextureFormat::R32Float,
```

Plus normals went from `Rgba8Snorm` (1/127 precision) to `Rg16Snorm` (1/32767). That commit also added bicubic Catmull-Rom interpolation, which made the close-tier surface look smooth.

The base tier was deliberately not converted to R32Float at that time because it would double the base footprint. On the 10800×10800 Tirol demo grid + 8 mip levels, R16Float = 296 MB; R32Float would be 593 MB. With the close tier already at 800 MB and the fine tier at 600 MB, the base tier was the one place where memory was still saved.

## Why this isn't a Phase 1 / Phase 2 regression

Neither Phase 1 (instrumentation) nor Phase 2 (eager dealloc) touched any texture format. The base tier alloc / re-alloc paths in `update_heightmap` and `GpuScene::new` still use `wgpu::TextureFormat::R16Float`. Verified by grep:

```
scene/mod.rs:236  format: wgpu::TextureFormat::R16Float,   (GpuScene::new base)
scene/mod.rs:366  format: wgpu::TextureFormat::R16Float,   (create_tier_placeholder hm5m/hm1m placeholder)
scene/mod.rs:1185 format: wgpu::TextureFormat::R16Float,   (update_heightmap placeholder)
scene/mod.rs:1256 format: wgpu::TextureFormat::R16Float,   (update_heightmap real)
```

The user is noticing it now because Phase 2 made the base reload smoother (no stalls), so it's easier to fly to vantage points where the artefact is visible.

## Options for a future fix

1. **Switch base to R32Float.** Cleanest. Doubles base memory; on the Iris Plus / unified memory it's free, on a 4 GB discrete card it's the difference between fitting and not. Would need Phase 3's "Low" VRAM preset to compensate by also halving the base radius.

2. **CPU-side dither when packing to f16.** Add a small per-pixel jitter (uniform −0.5 to +0.5 LSBs of the local quantization step) before `half::f16::from_f32`. Breaks the horizontal banding into noise that the visual system filters out. ~0 memory cost, ~5 ms extra per base load on a 10800² grid. Has the downside that successive reloads would produce slightly different jitter and could shimmer at high frame rate; mitigated by seeding the RNG from `(row, col)` so each pixel's jitter is deterministic.

3. **Pre-blur the base before f16 conversion.** Gaussian σ ≈ 0.3 source pixels would smooth the discrete bands but also soften ridgelines — CLAUDE.md already calls this out as a non-starter ("Gaussian smoothing destroys ridgelines").

4. **Make Phase 3's "Low" preset hide it.** If the close-tier radius is large enough that the base is only visible beyond fog (or a fade-to-sky horizon), the user never sees the artefact. Cheapest if it lines up with the other Phase 3 work — but doesn't help users with explicit high-altitude fly-overs.

5. **R11G11B10Float for base.** Three 11-bit floats packed in 32 bits — but `wgpu::TextureFormat::Rg11b10Ufloat` is unsigned-only (no negative values) and the format doesn't really fit a 1-channel use. Skip.

Option **2** (dither) is the most likely sweet spot: zero memory cost, fixes the visual, no destruction of features. The shimmer worry is real but seedable. Worth a separate experiment after the OOM crash is shipped.

## What I'm doing right now

Nothing. Documenting the finding and moving on to Phase 3. The OOM crash is the priority for the GTX 1650 user; the stair pattern has been latent for weeks without anyone filing an issue.
