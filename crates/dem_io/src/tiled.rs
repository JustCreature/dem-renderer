struct TiledHeightmap {
    pub tiles: Vec<i16>,  // tile-organised data (all tiles concatenated)
    pub rows: usize,      // total rows (same as Heightmap)
    pub cols: usize,      // total cols
    pub tile_size: usize, // e.g. 128
    pub tile_rows: usize, // number of tile rows = ceil(rows / tile_size)
    pub tile_cols: usize, // number of tile cols = ceil(cols / tile_size)
    pub origin_lat: f64,  // latitude of row 0 (north edge)
    pub origin_lon: f64,  // longitude of col 0 (west edge)
    pub dx_deg: f64,      // degrees per column (east = positive)
    pub dy_deg: f64,      // degrees per row (south = negative, from .blw)
    pub dx_meters: f64,   // real-world cell width (for normals in Phase 2)
    pub dy_meters: f64,   // real-world cell height (for normals in Phase 2)
}
