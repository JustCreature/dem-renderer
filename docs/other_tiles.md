# Other Tile Sources — 1m Resolution Global Coverage

## Andes / Santiago de Chile

### The only freely available near-1m dataset for the area

**Las Bayas Catchment — Zenodo (open access)**
- Location: 33.31°S, 70.25°W — elevation 3,218–4,022 m
- Essentially the same ridge as La Parva ski resort (33.33°S, 70.25°W)
- Collected with a Riegl VZ6000 terrestrial long-range scanner, winter 2018
- Contains: bare-earth elevation (Z), snow depth (SD), slope (SLP), northness (NOR), TPI at multiple scales
- Format: **CSV point cloud** — needs interpolation/gridding before use in the renderer
- Coverage: research catchment only, a few km²
- Link: https://zenodo.org/records/3964394
- Paper: Mendoza et al. 2020, Water Resources Research — https://agupubs.onlinelibrary.wiley.com/doi/10.1029/2020WR028480

### What does not exist freely for the Andes

| What you'd want | Reality |
|---|---|
| IGM Chile 1m national LiDAR | Exists, not open — sold or restricted |
| Airborne 1m mosaic of ski resorts | No public version |
| DGA (water authority) LiDAR | River valleys only, not high alpine |
| OpenTopography Chile collection | Coastal Cordillera forests (26–38°S), not the main Andes chain |

---

## Himalaya / Everest

### The best freely available high-resolution DEM for the area

**HMA 8m DEM Mosaic — NSIDC (NASA Earthdata)**
- Dataset: `HMA_DEM8m_MOS` (DOI 10.5067/KXOVQ9L172S2), Polar Geospatial Center, derived from WorldView / GeoEye optical stereo
- Resolution: 8 m
- Coverage: All of High Mountain Asia — Hindu Kush, Karakoram, Himalaya, Tibetan Plateau
- Format: GeoTIFF, 12500×12500 px per mosaic tile (~250–370 MB)
- CRS: Albers Equal Area centred on 36°N, 85°E — encoded **inline** via GeoKey `3072=32767` (user-defined sentinel) + `ProjCoordTransGeoKey` + `GeoDoubleParams`. No WKT, no single EPSG code; the renderer's three-path CRS discovery handles this since PR #55
- Access: Free with NASA Earthdata Login (free registration)
- Portal: https://nsidc.org/data/data-access-tool/HMA_DEM8m_MOS/versions/1/ — draw a bounding box on the map to filter
- For Everest (27.99°N, 86.92°E): a 1°×1° box (W 86.5 / S 27.5 / E 87.5 / N 28.5) returns 4 tiles in a 2×2 grid; **tile-677** (SE quadrant) alone covers Everest, Lhotse, Makalu, Cho Oyu, Khumbu Glacier, Namche, and Lukla
- User guide: https://nsidc.org/sites/default/files/hma_dem8m_mos-v001-userguide_1.pdf
- Sibling products (same DOI family): `HMA_DEM8m_AT` (along-track stereo strips), `HMA_DEM8m_CT` (cross-track strips). Use the **Mosaic** product for a single seamless raster; the strip products are gappy in space but useful for time-series work
- License: NASA DAAC public-domain use, citation required (DOI + producer credit); keep gitignored in `tiles/big_size/`

### What does not exist freely for the Himalaya at sub-8m

| What you'd want | Reality |
|---|---|
| 1 m wall-to-wall Everest | Does not exist publicly |
| NatGeo / Rolex 2019 Khumbu LiDAR | Open via Dryad (https://datadryad.org/dataset/doi:10.5061/dryad.73n5tb2vx) but **point cloud only** — LAS 1.2/1.4, 23 GB, South Col → Dugla strip. Needs gridding via PDAL / LAStools before the renderer can use it. Mandatory NatGeo + Rolex credit |
| EarthDEM 2 m (PGC, WorldView stereo) | Covers HMA wall-to-wall but **access-controlled** via NASA CSDA Satellite Data Explorer — research / education eligibility only |
| Pléiades tri-stereo Khumbu DTM (Lamsal et al. 2015) | ~2–4 m, published research product; CNES Pléiades licensing typically forbids redistribution — contact authors directly |
| 2020 Chinese summit remeasurement DEM | Only the new summit elevation (8848.86 m) was released — no raster product |
| OpenTopography Khumbu high-res | No dedicated collection as of 2026-05; SRTM and ASTER GDEM (30 m) are the only consistently-listed Himalaya products |
| ICIMOD RDS (regional Hindu Kush–Himalaya repo) | Mostly ≥30 m — good for catchment masks, not finer than HMA |

---

## Global 1m Mountain DEM Sources

### Tier 1 — Open, free, immediate download, excellent mountain terrain

**Norway — Høydedata**
- Resolution: 1m nationwide LiDAR DTM
- Coverage: Everything — Jotunheimen, Trolltinden, Hardangervidda, Lofoten
- Format: GeoTIFF tiles
- Access: Free, no account required, commercial use allowed
- Portal: https://hoydedata.no/LaserInnsyn2
- Notes: Best single option for dramatic mountain terrain outside Austria — fjords + 2000m peaks in the same dataset

**USA — USGS 3DEP**
- Resolution: 1m LiDAR
- Coverage: Patchy but expanding fast — Sierra Nevada (confirmed 2022 dataset), Cascades, parts of Rockies, Grand Teton
- Format: GeoTIFF / LAZ point cloud
- Access: Free, no restrictions
- Portal: https://apps.nationalmap.gov/downloader/
- Also on AWS S3 open dataset and Google Earth Engine
- Specific dataset: `CA_SierraNevada_1_2022` on OpenTopography — https://portal.opentopography.org/usgsDataset?dsid=CA_SierraNevada_1_2022

**New Zealand — LINZ**
- Resolution: 1m LiDAR
- Coverage: Southern Alps, Remarkables (near Queenstown), Fiordland
- Format: GeoTIFF, 400+ tiles, NZTM2000 projection (EPSG:2193)
- Access: Free, CC-BY 4.0, also on AWS S3 for bulk download
- Vertical accuracy: ±0.2m (95%)
- Portal: https://data.linz.govt.nz/layer/121859-new-zealand-lidar-1m-dem/
- AWS: https://registry.opendata.aws/nz-elevation/
- Notes: Geologically young terrain — very sharp relief, good for the renderer

**Switzerland — swissALTI3D**
- Resolution: **0.5m** and 2m
- Coverage: Entire Swiss Alps + Liechtenstein
- Format: GeoTIFF, 1 km² tiles
- Projection: CH1903+/LV95 (EPSG:2056) — needs CRS handling
- Access: Open data since ~2021, free
- Vertical accuracy: ±0.3m to ±0.5m below 2000m
- Portal: https://www.swisstopo.admin.ch/en/height-model-swissalti3d
- Also via opendata.swiss: https://opendata.swiss/en/dataset/swissalti3d
- Notes: Highest resolution Alps data available anywhere publicly

### Tier 2 — Open but with caveats

**Canada — HRDEM (Natural Resources Canada)**
- Resolution: 1m–2m LiDAR
- Coverage: 2M+ km² as of 2024, expanding — mountain coverage concentrated in BC/Alberta valleys, high alpine Rockies not fully covered yet
- Access: Open government license, free
- Portal: https://open.canada.ca/data/en/dataset/957782bf-847c-4644-a757-e383c0057995
- Also on AWS: https://registry.opendata.aws/canelevation-dem/
- Also via OpenTopography: https://portal.opentopography.org/datasetMetadata?otCollectionID=OT.062025.3979.1

**France — IGN RGE Alti**
- Resolution: 1m
- Coverage: Much of France including French Alps (Mont Blanc massif, Écrins, Mercantour), Pyrenees
- Format: GeoTIFF, Lambert-93 projection (EPSG:2154)
- Access: Free since 2021
- Portal: https://geoservices.ign.fr/

---

## Summary Table

| Country | Resolution | Mountain coverage | Format | Access |
|---|---|---|---|---|
| Norway | 1m | Full national — Jotunheimen, Lofoten, Trolltinden | GeoTIFF | Free, no account |
| USA (3DEP) | 1m | Sierra Nevada, Cascades, partial Rockies | GeoTIFF / LAZ | Free, National Map |
| New Zealand | 1m | Southern Alps, Remarkables, Fiordland | GeoTIFF | Free, AWS |
| Switzerland | **0.5m** | Entire Swiss Alps | GeoTIFF | Free (open since ~2021) |
| Austria (BEV) | 1m / 5m | Already in use | GeoTIFF | Free |
| Canada | 1m–2m | Valleys, partial Rockies | GeoTIFF | Free, AWS |
| France | 1m | French Alps, Pyrenees | GeoTIFF | Free since 2021 |
| Chile (Las Bayas) | ~1m | La Parva ridge only, ~few km² | CSV point cloud | Free, Zenodo |
| HMA (PGC) | 8m | Everest, Karakoram, all High Mountain Asia | GeoTIFF | Free, NASA Earthdata login |

---

## Recommended Next Targets for the Renderer

1. **Norway** — most immediately useful; free, no account, same GeoTIFF format, dramatic fjord+mountain terrain in one dataset
2. **New Zealand (Remarkables near Queenstown)** — sharp alpine relief, free, AWS bulk download
3. **Switzerland swissALTI3D at 0.5m** — highest resolution Alps data anywhere, needs EPSG:2056 CRS support
