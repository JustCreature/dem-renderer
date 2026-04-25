fn main() {
    // On Windows, the NVIDIA Optimus and AMD Hybrid drivers use GetProcAddress() to look up
    // NvOptimusEnablement / AmdPowerXpressRequestHighPerformance in the PE export table of the
    // running executable.  #[no_mangle] alone is not enough with the MSVC linker — the symbol
    // must be explicitly added to the export directory via /EXPORT linker flags.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        println!("cargo:rustc-link-arg=/EXPORT:NvOptimusEnablement");
        println!("cargo:rustc-link-arg=/EXPORT:AmdPowerXpressRequestHighPerformance");
    }
}
