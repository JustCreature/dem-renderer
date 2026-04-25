build_arm:
	RUSTFLAGS="-C target-cpu=native" cargo build --release

build_x86:
	RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release --target x86_64-apple-darwin

run_arm:
	RUSTFLAGS="-C target-cpu=native" cargo run --release

run_x86:
	RUSTFLAGS="-C target-cpu=x86-64-v3" cargo run --release --target x86_64-apple-darwin

run: run_arm

.PHONY: build_arm build_x86 run_arm run_x86 run
