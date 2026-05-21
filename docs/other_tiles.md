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

---

## Recommended Next Targets for the Renderer

1. **Norway** — most immediately useful; free, no account, same GeoTIFF format, dramatic fjord+mountain terrain in one dataset
2. **New Zealand (Remarkables near Queenstown)** — sharp alpine relief, free, AWS bulk download
3. **Switzerland swissALTI3D at 0.5m** — highest resolution Alps data anywhere, needs EPSG:2056 CRS support
