use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Launcher version line
    // The canonical version lives ONLY in Cargo.toml. Release CI passes the tag via
    // DEM_RENDERER_RELEASE_VERSION (and verifies it equals the Cargo.toml version), so
    // the launcher UI string is derived, never a second literal that can drift. Local
    // / dev builds show "latest". The build date is UTC, formatted YYYY.MM.DD.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEM_RENDERER_RELEASE_VERSION");
    let version = std::env::var("DEM_RENDERER_RELEASE_VERSION").unwrap_or_else(|_| "latest".into());
    println!(
        "cargo:rustc-env=APP_VERSION_LINE=v {version} · build {}",
        build_date()
    );

    // On Windows, the NVIDIA Optimus and AMD Hybrid drivers use GetProcAddress() to look up
    // NvOptimusEnablement / AmdPowerXpressRequestHighPerformance in the PE export table of the
    // running executable.  #[no_mangle] alone is not enough with the MSVC linker — the symbol
    // must be explicitly added to the export directory via /EXPORT linker flags.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rustc-link-arg=/EXPORT:NvOptimusEnablement");
        println!("cargo:rustc-link-arg=/EXPORT:AmdPowerXpressRequestHighPerformance");
        let mut res = winres::WindowsResource::new();
        // Point this to your actual .ico file
        res.set_icon("assets/icon.ico");
        res.compile().unwrap();
    }
}

/// Today's UTC date as `YYYY.MM.DD`, with no external dependencies.
fn build_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}.{m:02}.{d:02}")
}

/// Days since the Unix epoch (1970-01-01) → (year, month, day).
/// Howard Hinnant's `civil_from_days`, valid for any proleptic-Gregorian date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
