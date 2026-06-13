mod color;
pub mod crs;
mod geotiff;
mod grid;
mod heightmap;
mod lzw_lenient;
mod overview;

pub use color::{
    ColorWindow, MATERIAL_BUILDING, MATERIAL_HIGH_VEG, MATERIAL_MED_VEG, MATERIAL_NONE,
    MATERIAL_WATER, extract_color_window, landcover_histogram,
};
pub use crs::get_tile_epsg;
pub use geotiff::{
    extract_window, geotiff_pixel_scale, ifd_overview_levels, ifd_scales, parse_geotiff_auto,
    tile_bounds_wgs84, tile_centre_crs,
};
pub use grid::{
    assemble_grid, crop, load_grid_from_paths, stitch_windows, stitch_windows_geographic,
};
pub use heightmap::{
    Heightmap, clamp_nodata_to_sea, composite_surface_over, fill_nodata_from_base, parse_bil,
};
pub use overview::{BASE_OVERVIEW_TARGET_M, CLOSE_OVERVIEW_TARGET_M, ensure_overview_cache};

pub(crate) type DemError = Box<dyn std::error::Error>;
