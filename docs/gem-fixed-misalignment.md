# 5m Tier Misalignment Fix

This document explains the root causes and fixes for the persistent misalignment between the geographic WGS84 (EPSG:4326) 30m base tier and the projected Lambert Conformal Conic (EPSG:31287) 5m/1m BEV detail tiers.

The issue manifested in two ways:
1. A constant ~260m X/Y geographical translation offset.
2. A rotation mismatch of ~2.0° that made the tile corners drift out of alignment.

Here are the four specific bugs that compounded to cause this and how they were resolved:

## 1. Silent Datum Shift Failure (`crates/dem_io/src/crs.rs`)
**The Bug:** The EPSG:31287 projection uses the Bessel 1841 ellipsoid, which requires a 7-parameter Helmert transformation (`+towgs84`) to properly project coordinates into the WGS84 base world. We had code to manually inject this shift, but it checked for an exact string match of `+towgs84=0,0,0,0,0,0,0`. The `proj4wkt` crate sometimes returns a truncated default string (`+towgs84=0,0,0`). Because the string match failed, the Helmert shift was silently ignored, resulting in the ~260m geocentric offset.
**The Fix:** Updated the parsing logic to split the `+towgs84` value and recognize any representation where all parameters evaluate to `0.0`. If so, the correct 7-parameter shift for Austria is injected.

## 2. Missing Meridian Convergence Rotation (`src/viewer/tiers.rs`)
**The Bug:** In the Lambert Conformal Conic projection, the grid North only aligns perfectly with geographic (WGS84) North at the central meridian. Everywhere else, the grid is tilted (meridian convergence). At the edges of Austria, this tilt is around 2.0°. The renderer was placing the 5m tile exactly at its top-left anchor but failing to rotate it to match the geographic grid, causing up to 350m of drift at the far corners of a 20 km tile.
**The Fix:** Updated `cross_crs_world_origin_and_extent` to compute the actual WGS84 positions of both the Top-Left and Top-Right corners. By calculating the angle of the vector between them (`(tr_wy - oy).atan2(ex)`), we dynamically determine the convergence rotation (`rot_rad`) and pass it to the shader.

## 3. Incorrect Rotation Pivot (`crates/render_gpu/src/shader_texture.wgsl`)
**The Bug:** When the rotation was initially passed to the shader, the shader attempted to apply it by rotating the UV coordinates around the *center* of the tile. However, the `origin_x`/`origin_y` uniforms computed by the CPU define the exact position of the tile's *top-left corner*. Rotating around the center physically moved the top-left anchor point by hundreds of meters, making the misalignment much worse.
**The Fix:** Changed the `align5m` and `align1m` functions in the WGSL shader to rotate strictly around `(0, 0)` in tile-local space. This locks the top-left anchor perfectly in place while fanning the rest of the tile out at the correct convergence angle.

## 4. Extent Scaling/UV Squash (`src/viewer/tiers.rs`)
**The Bug:** When computing the dimensions of the projected tile in the base WGS84 world, the code used just the X-axis difference (`ex`) of the rotated top edge as the `hm5m_extent_x` uniform. Because the edge was rotated by ~1.3° to 2.0°, its X-component was shorter than its true physical length. This caused the shader to map the UVs over a slightly squashed area.
**The Fix:** Updated the extent calculation to use the hypotenuse (`true_extent_x = ex.hypot(tr_wy - oy)` and `true_extent_y = (bl_wy - oy).hypot(bl_wx - ox)`). This ensures the physical length of the rotated tile edge is passed to the shader, allowing the UVs to map perfectly to `1.0` across the quadrilateral.

---

## Accompanying Groundwork and Optimizations

Alongside the mathematical bug fixes above, several architectural and performance features were integrated to support the new alignment logic and improve viewer stability:

### 1. The Core Alignment Groundwork
* **Base Scale Fix (`crates/dem_io/src/grid.rs`):** Changed `stitch_windows_geographic` to use the Northwest corner's latitude (`out_lat1`) instead of the camera's center latitude for calculating `dx_meters`, ensuring the base WGS84 grid doesn't warp and stretch dynamically as the camera flies across it.
* **The Extent Function (`src/viewer/tiers.rs`):** Introduced the `cross_crs_world_origin_and_extent` function to compute the exact boundaries of the tile when projected onto the base map.

### 2. Manual Alignment Controls (`src/viewer/mod.rs` & `src/launcher/config.rs`)
* Added `Ctrl` + `I/J/K/L` and `[/]` keyboard shortcuts to allow for manual nudging of the 5m and 1m tiers (both translation and rotation) for debugging and fine-tuning.
* Added the ability to press `Ctrl + S` to save these manual alignment tweaks to the persistent `config.toml` file.
* Added a visualizer mode (toggled with `V`) to show the three tiers as distinct colored surfaces.

### 3. Flight Speed Optimization (`src/viewer/mod.rs`)
* **Speed Gate:** Introduced a mechanism that suppresses the loading of the 5m and 1m tiers while the camera is flying faster than 2500 m/s. This prevents the engine from constantly thrashing the background workers to load 20km windows that the camera will outrun before they even finish rendering. 
* Once the camera slows down (< 2500 m/s) for 400 continuous milliseconds, high-detail data loading resumes.

### 4. Safety Snapping (`src/viewer/mod.rs`)
* If the camera moves fast enough to completely escape the bounds of a newly-loading tile, the viewer now detects this and gracefully snaps the camera to the center of the tile. This prevents the renderer from crashing or showing a completely blank blue screen on the very first frame.