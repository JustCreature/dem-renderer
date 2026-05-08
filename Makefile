# ── Build ─────────────────────────────────────────────────────────────────────

build_arm:
	RUSTFLAGS="-C target-cpu=native" cargo build --release

build_x86:
	RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --target x86_64-apple-darwin

# ── Run ───────────────────────────────────────────────────────────────────────

run: view

view:
	RUSTFLAGS="-C target-cpu=native" cargo run --release

view-vsync:
	RUSTFLAGS="-C target-cpu=native" cargo run --release -- --vsync

view-1m:
	RUSTFLAGS="-C target-cpu=native" cargo run --release

# ── Config ────────────────────────────────────────────────────────────────────

CONFIG_FILE := $(HOME)/Library/Application\ Support/dem_renderer/config.toml

config:
	@mkdir -p "$(HOME)/Library/Application Support/dem_renderer"
	@touch "$(HOME)/Library/Application Support/dem_renderer/config.toml"
	vim "$(HOME)/Library/Application Support/dem_renderer/config.toml"

# ── Data ──────────────────────────────────────────────────────────────────────

download-tiles:
	bash download_copernicus_tiles_30m.sh

.PHONY: build_arm build_x86 run view view-vsync view-1m config download-tiles
