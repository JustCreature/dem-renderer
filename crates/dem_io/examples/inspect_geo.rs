//! Dump the CRS-related GeoTIFF tags of a single file: GeoKeyDirectory (34735),
//! GeoAsciiParams (34737), GeoDoubleParams (34736), and the proj4 string that
//! `dem_io::crs::tile_proj4` resolves it to.
//!
//! Reach for this first whenever a tile fails to load with a CRS error, or when
//! you need to understand how an unfamiliar GeoTIFF encodes its CRS — inline
//! GeoKeys (PGC's HMA mosaics, older USGS/NOAA data), WKT in GeoAsciiParams
//! (most modern files), or just an EPSG code in GeoKey 3072. The dump reveals
//! which of the three discovery paths in `crs::proj4_from_keys` will fire,
//! which is usually enough to pinpoint why a file isn't loading.
//!
//! Usage:
//!     cargo run --release -p dem_io --example inspect_geo -- path/to/file.tif

use std::env;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use tiff::decoder::Decoder;
use tiff::tags::Tag;

fn main() {
    let path = env::args().nth(1).expect("usage: inspect_geo <tif>");
    let path = Path::new(&path);

    let f = File::open(path).expect("open");
    let mut dec = Decoder::new(BufReader::new(f)).expect("decode");

    let raw = dec
        .get_tag(Tag::Unknown(34735))
        .expect("no 34735")
        .into_u32_vec()
        .expect("u32");
    let n_keys = raw[3] as usize;
    println!(
        "GeoKeyDirectory (34735): version=({},{},{}) n_keys={}",
        raw[0], raw[1], raw[2], n_keys
    );
    for i in 0..n_keys {
        let b = 4 + i * 4;
        println!(
            "  key={:5} loc={:6} count={:4} val_or_off={}",
            raw[b],
            raw[b + 1],
            raw[b + 2],
            raw[b + 3]
        );
    }

    match dec.get_tag(Tag::Unknown(34737)) {
        Ok(tiff::decoder::ifd::Value::Ascii(s)) => {
            println!("\nGeoAsciiParams (34737): len={}", s.len());
            println!("{:?}", s);
        }
        Ok(other) => println!("\n34737 non-ascii: {:?}", other),
        Err(e) => println!("\n34737 missing: {}", e),
    }

    // Tag 34736 — GeoDoubleParamsTag (projection parameters as doubles)
    match dec.get_tag(Tag::Unknown(34736)) {
        Ok(v) => {
            if let Ok(dv) = v.into_f64_vec() {
                println!("\nGeoDoubleParams (34736): {:?}", dv);
            }
        }
        Err(_) => println!("\n34736 missing"),
    }

    println!("\n=== resolved by dem_io::crs::tile_proj4 ===");
    match dem_io::crs::tile_proj4(path) {
        Ok(p4) => println!("{p4}"),
        Err(e) => println!("ERROR: {e}"),
    }
}
