
.hdr data downloaded from: https://earthexplorer.usgs.gov/
Entity ID: SRTM1N47E011V3
Data Set Search: SRTM 1 Arc-Second Global

camera posistion: 47°04'31.90"N 11°40'56.64"E, 3341, 2624, tilt 80, heading 85



## Compilation 

On the Windows machine:                                                                                                                                        
# Install Rust from https://rustup.rs, then:
cargo build --release
# or with AVX2:
$env:RUSTFLAGS="-C target-cpu=x86-64-v3"
cargo build --release

On the Linux machine:
# Install Rust:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Then:
RUSTFLAGS="-C target-cpu=native" cargo build --release

You'll also need a C linker on both — on Linux build-essential (Debian/Ubuntu) or gcc (Fedora), on Windows the MSVC build tools (installed via Visual Studio
installer, selecting "C++ build tools").


RUSTFLAGS="-C target-cpu=native" cargo run --release 2>&1 | tee results_$(shell hostname)_arm.txt
RUSTFLAGS="-C target-cpu=x86-64-v3" cargo run --release --target x86_64-apple-darwin 2>&1 | tee results_$(shell hostname)_x86.txt
