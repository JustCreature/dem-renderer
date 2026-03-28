use crate::Heightmap;
use crate::aligned::AlignedBuffer;

pub struct TiledHeightmap {
    tiles: AlignedBuffer, // tile-organised data (all tiles concatenated)
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

impl TiledHeightmap {
    pub fn tiles(&self) -> &[i16] {
        &self.tiles
    }

    pub fn from_heightmap(hm: &Heightmap, tile_size: usize) -> TiledHeightmap {
        let tile_rows = (hm.rows + tile_size - 1) / tile_size; // ceil division
        let tile_cols = (hm.cols + tile_size - 1) / tile_size;

        let total_elements = tile_rows * tile_cols * tile_size * tile_size;
        // let mut tiles = vec![0i16; total_elements];
        let mut tiles = AlignedBuffer::new(total_elements, 4096);

        for tr in 0..tile_rows {
            for tc in 0..tile_cols {
                for r in 0..tile_size {
                    for c in 0..tile_size {
                        // compute src_r, src_c (with clamping)
                        // compute dst index
                        // tiles[dst] = hm.data[src]

                        // implement

                        let src_r = (tr * tile_size + r).min(hm.rows - 1);
                        let src_c = (tc * tile_size + c).min(hm.cols - 1);
                        let src = src_r * hm.cols + src_c;

                        let tile_idx = tr * tile_cols + tc;
                        let dst = tile_idx * tile_size * tile_size + r * tile_size + c;

                        tiles[dst] = hm.data[src];
                    }
                }
            }
        }

        TiledHeightmap {
            tiles,
            rows: hm.rows,
            cols: hm.cols,
            tile_size,
            tile_rows,
            tile_cols,
            origin_lat: hm.origin_lat,
            origin_lon: hm.origin_lon,
            dx_deg: hm.dx_deg,
            dy_deg: hm.dy_deg,
            dx_meters: hm.dx_meters,
            dy_meters: hm.dy_meters,
        }
    }

    #[inline(always)]
    pub fn get(&self, row: usize, col: usize) -> i16 {
        let tile_r = row / self.tile_size;
        let tile_c = col / self.tile_size;
        let local_r = row % self.tile_size;
        let local_c = col % self.tile_size;

        let tile_idx = tile_r * self.tile_cols + tile_c;
        let dst = tile_idx * self.tile_size * self.tile_size + local_r * self.tile_size + local_c;

        self.tiles[dst]
    }
}
