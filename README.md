
.hdr data downloaded from: https://earthexplorer.usgs.gov/
Entity ID: SRTM1N47E011V3
Data Set Search: SRTM 1 Arc-Second Global

camera posistion: 47°04'31.90"N 11°40'56.64"E, 3341, 2624, tilt 80, heading 85

Look for CONFIG_PERFORMANCE comment to change performance related values, 
you can boost performance but it will introdice visual artefacts, especially at narrow places like the top of the ridge ot a thin wall.

get data from copernicus: `aws s3 cp s3://copernicus-dem-30m/Copernicus_DSM_COG_10_N47_00_E011_00_DEM/ ./tiles/Copernicus_DSM_COG_10_N47_00_E011_00_DEM --recursive --no-sign-request`


## Compilation

### macOS (ARM / Apple Silicon)

```sh
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

### macOS (cross-compile for x86_64)

```sh
rustup target add x86_64-apple-darwin
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --target x86_64-apple-darwin
```

The resulting binary will not run on the Mac — copy it to an x86_64 machine.

### Windows

Install Rust from https://rustup.rs, then install the MSVC build tools via the
Visual Studio installer (select "C++ build tools"). Then:

```powershell
cargo build --release
# or with AVX2:
$env:RUSTFLAGS="-C target-cpu=x86-64-v3"
cargo build --release
```

NVIDIA Control Panel → Manage 3D Settings → Program Settings → add dem_renderer.exe → set OpenGL rendering GPU and
  Preferred graphics processor to High-performance NVIDIA processor.

### Linux

```sh
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install C linker and GPU dependencies (Debian/Ubuntu)
sudo apt install build-essential pkg-config libvulkan-dev mesa-vulkan-drivers

# Fedora
sudo dnf install gcc pkg-config vulkan-loader-devel

# Build
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

## Running

Use the Makefile targets:

```sh
make run        # ARM (native, same as run_arm)
make run_arm    # ARM with target-cpu=native
make run_x86    # x86_64 cross-compiled (macOS only)
```

Output is saved to `results_<hostname>_<arch>.txt` via `tee`.

## GPU on Linux — troubleshooting

wgpu defaults to Vulkan on Linux. On old Intel integrated graphics (Atom/Celeron/
Pentium x5, N3xxx, J3xxx series — ~2015 and older) Vulkan may not be available.

Check if Vulkan is working:

```sh
sudo apt install vulkan-tools
vulkaninfo --summary
```

If Vulkan is missing, install the Intel driver:

```sh
sudo apt install intel-media-va-driver mesa-vulkan-drivers
```

If Vulkan still doesn't work (common on Atom x5/N3xxx hardware), force wgpu to use
the OpenGL backend instead:

```sh
WGPU_BACKEND=gl ./dem_renderer
```

Available backend options: `vulkan`, `gl`, `dx12` (Windows), `metal` (macOS).

### GPU TDR (device lost) on weak Intel integrated graphics

On Intel HD 405 / Braswell and similar low-power chips, the large 8000×2667 render
takes ~15 seconds per frame. The Linux kernel GPU scheduler has a hang detection
timeout (typically 10–20 s) and will kill the GPU job if it runs too long, causing
wgpu to report "Parent device is lost". The multi-frame benchmark (10 × 8000×2667)
would take ~150 seconds total and reliably hits this timeout.

**Estimated times on Intel HD 405 (measured):**

| Render | Resolution | Time |
|---|---|---|
| Single frame (buffer/texture/combined) | 8000×2667 | ~15 s |
| Multi-frame benchmark (10 frames) | 8000×2667 | ~150 s (~2.5 min) |
| FPS benchmark (30 frames) | 1600×533 | ~15 s (~2 fps) |

To disable hang detection and let long renders complete:

```sh
# Create or edit /etc/modprobe.d/i915.conf
echo "options i915 enable_hangcheck=0" | sudo tee /etc/modprobe.d/i915.conf
sudo update-initramfs -u
sudo reboot
```

Then run with:

```sh
WGPU_BACKEND=vulkan ./dem_renderer
```

Note: disabling hangcheck means a truly stuck GPU will hang the machine instead of
recovering. Re-enable it (`enable_hangcheck=1`) after benchmarking if desired.

## Cargo git fetch behind proxy

If `cargo build` fails with a network error when
resolving a git dependency, Cargo's default `libgit2` backend doesn't use the system
proxy. Fix by switching to the system `git` CLI:

Add to `~/.cargo/config.toml`:

```toml
[net]
git-fetch-with-cli = true
```
