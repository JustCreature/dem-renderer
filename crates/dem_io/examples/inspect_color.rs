//! Inspect a land-cover (and optionally orthophoto) mosaic: dump the mask-aware
//! overview levels and class-value histograms at known Tirol reference spots
//! (reservoir / forest slope / town / glacier). Used to pin the BEV land-cover
//! class → material mapping in `dem_io::color::lc_code_to_material` — the class
//! codes are not documented in the GeoTIFF itself.
//!
//! Usage:
//!     cargo run --release -p dem_io --example inspect_color -- tiles/color/2022470_Mosaik_LC.tif [tiles/color/2019470_Mosaik_RGB.tif]

use std::env;
use std::path::Path;

fn main() {
    let lc_arg = env::args()
        .nth(1)
        .expect("usage: inspect_color <landcover.tif> [ortho_rgb.tif]");
    let lc_path = Path::new(&lc_arg).to_path_buf();
    let rgb_arg = env::args().nth(2);

    let levels = dem_io::ifd_overview_levels(&lc_path).expect("walk IFDs");
    println!("land cover overview levels (ifd, scale):");
    for (ifd, scale) in &levels {
        println!("  ifd {ifd:>2}  {scale:.2} m/px");
    }

    let proj4 = dem_io::crs::tile_proj4(&lc_path).expect("resolve CRS");
    println!("proj4: {proj4}");

    // (label, lat, lon, radius_m) — picked so each window is dominated by one class.
    let spots: &[(&str, f64, f64, f64)] = &[
        ("Achensee centre (deep lake)", 47.4450, 11.7080, 250.0),
        ("Speicher Schlegeis (reservoir)", 47.0285, 11.7090, 250.0),
        ("forest slope near Mayrhofen", 47.1500, 11.8400, 150.0),
        ("valley meadow near Uderns", 47.3120, 11.8740, 100.0),
        ("Mayrhofen town centre", 47.1660, 11.8630, 150.0),
        ("Hintertux glacier / rock", 47.0680, 11.6730, 250.0),
    ];

    // Use a mid overview (~1.6 m) so each window is a few hundred px — enough for
    // a stable histogram without decoding thousands of full-res chunks.
    let (ifd, scale) = levels
        .iter()
        .copied()
        .find(|&(_, s)| s >= 1.5)
        .unwrap_or(levels[0]);
    println!("\nhistograms at ifd {ifd} ({scale:.2} m/px):");

    for &(label, lat, lon, radius) in spots {
        let Ok(centre) = dem_io::crs::from_wgs84(lat, lon, &proj4) else {
            println!("  {label}: from_wgs84 failed");
            continue;
        };
        match dem_io::landcover_histogram(&lc_path, centre, radius, ifd) {
            Ok(hist) => {
                let total: u64 = hist.iter().sum();
                print!("  {label}: ");
                for (v, &n) in hist.iter().enumerate() {
                    if n > 0 {
                        print!("{v}:{:.0}% ", n as f64 / total as f64 * 100.0);
                    }
                }
                println!();
            }
            Err(e) => println!("  {label}: {e}"),
        }
    }

    if let Some(rgb_arg) = rgb_arg {
        let rgb_path = Path::new(&rgb_arg).to_path_buf();
        let rgb_levels = dem_io::ifd_overview_levels(&rgb_path).expect("walk RGB IFDs");
        println!("\northo overview levels (ifd, scale):");
        for (ifd, scale) in &rgb_levels {
            println!("  ifd {ifd:>2}  {scale:.2} m/px");
        }
        let (rgb_ifd, rgb_scale) = rgb_levels
            .iter()
            .copied()
            .find(|&(_, s)| s >= 1.5)
            .unwrap_or(rgb_levels[0]);
        let lc_ifd = levels
            .iter()
            .copied()
            .find(|&(_, s)| (s - rgb_scale).abs() < 0.01)
            .map(|(i, _)| i);
        println!("sampling ortho at ifd {rgb_ifd} ({rgb_scale:.2} m/px), lc ifd {lc_ifd:?}");

        for &(label, lat, lon, radius) in spots {
            let proj4_rgb = dem_io::crs::tile_proj4(&rgb_path).expect("resolve RGB CRS");
            let Ok(centre) = dem_io::crs::from_wgs84(lat, lon, &proj4_rgb) else {
                continue;
            };
            match dem_io::extract_color_window(
                &rgb_path,
                Some(&lc_path),
                centre,
                radius,
                rgb_ifd,
                lc_ifd,
            ) {
                Ok(win) => {
                    let n = win.georef.rows * win.georef.cols;
                    let c = n / 2; // centre-ish pixel
                    let px = &win.rgba[c * 4..c * 4 + 4];
                    let avg: [u64; 4] = win.rgba.chunks_exact(4).fold([0; 4], |mut a, p| {
                        for i in 0..4 {
                            a[i] += p[i] as u64;
                        }
                        a
                    });
                    println!(
                        "  {label}: {}×{}  centre rgba=({},{},{},{})  avg rgba=({},{},{},{})",
                        win.georef.cols,
                        win.georef.rows,
                        px[0],
                        px[1],
                        px[2],
                        px[3],
                        avg[0] / n as u64,
                        avg[1] / n as u64,
                        avg[2] / n as u64,
                        avg[3] / n as u64,
                    );
                }
                Err(e) => println!("  {label}: {e}"),
            }
        }
    }
}
