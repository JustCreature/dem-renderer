# Build

build_arm:
	RUSTFLAGS="-C target-cpu=native" cargo build --release

build_x86:
	RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --target x86_64-apple-darwin

# Run

run: view

view:
	RUSTFLAGS="-C target-cpu=native" cargo run --release

view-vsync:
	RUSTFLAGS="-C target-cpu=native" cargo run --release -- --vsync

view-1m:
	RUSTFLAGS="-C target-cpu=native" cargo run --release

# Config

CONFIG_FILE := $(HOME)/Library/Application\ Support/dem_renderer/config.toml

config:
	@mkdir -p "$(HOME)/Library/Application Support/dem_renderer"
	@touch "$(HOME)/Library/Application Support/dem_renderer/config.toml"
	vim "$(HOME)/Library/Application Support/dem_renderer/config.toml"

# Data

download-tiles:
	bash download_copernicus_tiles_30m.sh

generate_mac_icon:
	# 1. Create the required directory
	rm -rf myapp.iconset
	mkdir myapp.iconset

	# 2. Use the built-in 'sips' tool to generate all required resolutions
	sips -z 16 16     assets/icon_source.png --out myapp.iconset/icon_16x16.png
	sips -z 32 32     assets/icon_source.png --out myapp.iconset/icon_16x16@2x.png
	sips -z 32 32     assets/icon_source.png --out myapp.iconset/icon_32x32.png
	sips -z 64 64     assets/icon_source.png --out myapp.iconset/icon_32x32@2x.png
	sips -z 128 128   assets/icon_source.png --out myapp.iconset/icon_128x128.png
	sips -z 256 256   assets/icon_source.png --out myapp.iconset/icon_128x128@2x.png
	sips -z 256 256   assets/icon_source.png --out myapp.iconset/icon_256x256.png
	sips -z 512 512   assets/icon_source.png --out myapp.iconset/icon_256x256@2x.png
	sips -z 512 512   assets/icon_source.png --out myapp.iconset/icon_512x512.png
	sips -z 1024 1024 assets/icon_source.png --out myapp.iconset/icon_512x512@2x.png

	# 3. Compile the folder into a valid .icns file
	iconutil -c icns myapp.iconset -o assets/icon.icns

	# 4. Clean up the temporary folder
	rm -rf myapp.iconset

generate_windows_icon:
	# brew install imagemagick
	magick assets/icon_source.png -define icon:auto-resize=256,128,64,48,32,16 assets/icon.ico

# Coverage
# One-time local setup: the LLVM instrumentation tools, the coverage driver,
# and jq (used to derive the workspace package list below). cargo-llvm-cov is
# compiled from crates.io; rustup ships llvm-tools as a component.
setup-local:
	rustup component add llvm-tools-preview
	rustup toolchain install nightly -c rust-src -t wasm32-unknown-unknown
	cargo install cargo-llvm-cov trunk
	@command -v jq >/dev/null 2>&1 || brew install jq

# Mirrors the `coverage` job in .github/workflows/main.yml. The `report`
# subcommand rejects --workspace and defaults to the root package only, which
# would drop dem_io / terrain / render_gpu — so derive the member list from
# cargo metadata and pass it as -p flags. Tests run once (--no-report keeps the
# raw profile data); lcov + HTML + summary all read that one dataset.
# Paths excluded from the coverage report — intentionally untested code (the
# egui/winit launcher glue). Matched against the source path by llvm-cov.
COV_IGNORE := 'src/launcher/'

test-generate-report:
	cargo llvm-cov --workspace --no-report
	@PKGS=$$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | sed 's/^/-p /' | tr '\n' ' '); \
	cargo llvm-cov report $$PKGS --ignore-filename-regex $(COV_IGNORE) --lcov --output-path lcov.info; \
	cargo llvm-cov report $$PKGS --ignore-filename-regex $(COV_IGNORE) --html --output-dir coverage-html; \
	SUMMARY=$$(cargo llvm-cov report $$PKGS --ignore-filename-regex $(COV_IGNORE) --summary-only); \
	echo "$$SUMMARY"; \
	if [ -n "$$GITHUB_STEP_SUMMARY" ]; then \
		{ echo '## Coverage'; echo '```'; echo "$$SUMMARY"; echo '```'; } >> "$$GITHUB_STEP_SUMMARY"; \
	fi
	@echo ""
	@echo "lcov:  lcov.info"
	@echo "HTML:  coverage-html/html/index.html  (run 'make open-report')"

open-report:
	open coverage-html/html/index.html

# Release
release:
	git tag $(VERSION)
	git push origin $(VERSION)

release_force:
	git tag -d $(VERSION) || true
	git push origin :refs/tags/$(VERSION) || true
	git tag $(VERSION)
	git push origin $(VERSION)

.PHONY: build_arm build_x86 run view view-vsync view-1m config download-tiles setup-local test-generate-report open-report release release_force
