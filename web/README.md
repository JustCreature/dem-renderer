# dem-renderer-web — browser PoC

Viewer-only WebAssembly build of the DEM renderer. Click the canvas, pick a single
geographic GeoTIFF (≤ 6144 px per side), and it renders via the existing wgpu compute
raymarcher. No launcher, no streaming tiers, no native I/O.

This crate is **outside** the Cargo workspace (`exclude = ["web"]` in the root manifest) so
it builds standalone under its own nightly toolchain + `build-std` config without touching
native builds or CI. It reuses `dem_io` / `terrain` / `render_gpu` unchanged.

## One-time setup

```sh
rustup toolchain install nightly -c rust-src -t wasm32-unknown-unknown   # see rust-toolchain.toml
cargo install trunk                                                       # or a prebuilt binary
```

`trunk` auto-downloads a matching `wasm-bindgen-cli` on first build.

## Run locally

```sh
cd web
trunk serve --release      # http://localhost:8080
```

`Trunk.toml` sets the COOP/COEP headers the dev server needs; `coi-serviceworker.js` is the
fallback that provides cross-origin isolation on hosts that can't (see below). Open the
console and confirm `crossOriginIsolated === true` and `rayon thread pool ready (...)`.
Click the canvas → pick a tile → terrain renders, and the tab stays responsive during the
load because parse + normals run on a worker.

## Deploy to GitHub Pages

GitHub Pages cannot send COOP/COEP headers, so `coi-serviceworker.js` re-injects them and
reloads once. Build with the project subpath, then publish `dist/`:

```sh
cd web
trunk build --release --public-url /<repo-name>/
# publish ./dist to the gh-pages branch (e.g. `gh-pages` action or manual push)
```

## Architecture notes

- **Threads:** SharedArrayBuffer + `wasm-bindgen-rayon` (with the **`no-bundler`** feature
  — required, since Trunk has no JS bundler; the default worker does `import('../../..')`
  which resolves to the site root and 404s as `text/html`). `initializer.js` calls
  `initThreadPool()` once the module is ready (Trunk `data-initializer` hook). The
  `#[wasm_bindgen(start)]` fn bails out in worker contexts (`window().is_none()`) so the
  re-instantiated module doesn't spawn a second event loop / GPU device.
- **Threading link flags (`.cargo/config.toml`):** this toolchain does *not* auto-derive
  the threading layout from `+atomics`, so the flags are explicit and all required:
  `--shared-memory --import-memory --max-memory=…` give one SharedArrayBuffer every worker
  imports, and `--export=__heap_base/__wasm_init_tls/__tls_size/__tls_align/__tls_base`
  expose the symbols wasm-bindgen's threading transform needs (each missing one aborts the
  `wasm-bindgen` step with `failed to find <symbol>`). Symptoms of an incomplete set:
  `Atomics.waitAsync … not a shared typed array` and `postMessage … Memory could not be
  cloned` (memory wasn't shared), or `failed to prepare module for threading` at build time
  (a TLS/heap export was missing). Verify with: the generated JS should contain
  `new WebAssembly.Memory({…, shared:true})`.
- **GPU:** `GpuContext::new_async()` (added to `render_gpu`) — the wasm main thread can't
  block on `pollster`. WebGPU-default limits instead of `adapter.limits()`.
- **File → scene:** `dem_io::parse_geotiff_auto_reader(Cursor<Vec<u8>>)` (added to
  `dem_io`) parses the in-memory bytes; `terrain::compute_normals_vector` runs on the
  worker. Shadow + AO are uniform placeholders in this PoC.
- **Limits:** fixed 1024×768 render target (width ×4 is 256-byte aligned for the
  buffer→swapchain blit); tiles > 6144 px are rejected to stay under WebGPU's defaults.

## Verified / not verified

- ✅ Native workspace build unaffected (`cargo build --workspace`).
- ✅ `cargo +nightly build --target wasm32-unknown-unknown` (with `build-std` + atomics).
- ⏳ `trunk build`/`serve` and in-browser run (COOP/COEP isolation, thread pool, file pick,
  render) — needs `trunk` + a WebGPU browser; not runnable in the dev sandbox.

## Out of scope (deferred)

HTTP Range / COG streaming, multi-tier streaming, camera/input controls, real shadow/AO,
wasm SIMD (`v128`).
