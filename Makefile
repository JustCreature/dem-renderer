TILES_1M_DIR ?= tiles/big_size/

# ── Build ─────────────────────────────────────────────────────────────────────

build_arm:
	RUSTFLAGS="-C target-cpu=native" cargo build --release

build_x86:
	RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --target x86_64-apple-darwin

# ── Run ───────────────────────────────────────────────────────────────────────

run: view

view:
	RUSTFLAGS="-C target-cpu=native" cargo run --release -- --1m-tiles-dir $(TILES_1M_DIR)

view-vsync:
	RUSTFLAGS="-C target-cpu=native" cargo run --release -- --vsync --1m-tiles-dir $(TILES_1M_DIR)

view-1m:
	RUSTFLAGS="-C target-cpu=native" cargo run --release -- --1m-tiles-dir $(TILES_1M_DIR)

# ── Data ──────────────────────────────────────────────────────────────────────

download-tiles:
	bash download_copernicus_tiles_30m.sh

.PHONY: build_arm build_x86 run view view-vsync view-1m download-tiles
