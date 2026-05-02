mod system_info;
mod utils;
mod viewer;

// Tell the NVIDIA Optimus and AMD Hybrid driver to route this process through the discrete GPU.
// The driver checks for these exported symbols at process load time, before any D3D12/Vulkan
// calls are made.  Without this, Optimus may route compute dispatches through the iGPU even
// when the correct wgpu adapter is selected in software.
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
#[used]
pub static NvOptimusEnablement: u32 = 1;

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
#[used]
pub static AmdPowerXpressRequestHighPerformance: u32 = 1;

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // let tile_path = Path::new("n47_e011_1arc_v3_bil/n47_e011_1arc_v3.bil");
    // let tile_path = Path::new("tiles/Copernicus_DSM_COG_10_N47_00_E011_00_DEM/Copernicus_DSM_COG_10_N47_00_E011_00_DEM.tif");
    // let tile_path = Path::new("tiles/big_size/hintertux_5m.tif");
    // let tile_path = Path::new("tiles/big_size/hintertux_18km_5m.tif");
    // let tile_path = Path::new("tiles/big_size/hintertux_3km_1m.tif");
    // let tile_path = Path::new("tiles/big_size/hintertux_8km_1m.tif");
    // let tile_path = Path::new("tiles/big_size/salz_east_to_tux_base_8km_1m.tif");
    let tile_path = Path::new("tiles/big_size/5m_whole_austria/DGM_R5.tif");
    const WIDTH: u32 = 1600;
    const HEIGHT: u32 = 533;
    // const WIDTH: u32 = 8000;
    // const HEIGHT: u32 = 2667;
    let mut vsync: bool = false;
    if args.contains(&"--vsync".to_string()) {
        vsync = true;
    }
    let tiles_1m_dir: std::path::PathBuf = args
        .windows(2)
        .find(|w| w[0] == "--1m-tiles-dir")
        .map(|w| std::path::PathBuf::from(&w[1]))
        .unwrap_or_else(|| std::path::PathBuf::from("tiles/big_size/"));
    viewer::run(
        tile_path,
        WIDTH,
        HEIGHT,
        vsync,
        Some(tiles_1m_dir.as_path()),
    );
}
