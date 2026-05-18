use std::path::PathBuf;
use std::sync::Arc;
use winit::window::Window;

use crate::consts::DEFAULT_TILE_5M_PATH;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DemoViewConfig {
    #[serde(default = "default_demo_cam_lat")]
    pub camera_lat: f64,
    #[serde(default = "default_demo_cam_lon")]
    pub camera_lon: f64,
    #[serde(default = "default_demo_cam_elev")]
    pub camera_elev: f32,
    #[serde(default = "default_fine_paths")]
    pub fine_tile_paths: Vec<PathBuf>,
    #[serde(default = "default_close_paths")]
    pub close_tile_paths: Vec<PathBuf>,
    #[serde(default = "default_base_paths")]
    pub base_tile_paths: Vec<PathBuf>,
}

fn default_demo_cam_lat() -> f64 {
    crate::consts::DEFAULT_CAM_LAT
}
fn default_demo_cam_lon() -> f64 {
    crate::consts::DEFAULT_CAM_LON
}
fn default_demo_cam_elev() -> f32 {
    crate::consts::DEFAULT_CAM_ELEV
}

fn default_fine_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("tiles/big_size/CRS3035RES50000mN2650000E4400000.tif"),
        PathBuf::from("tiles/big_size/CRS3035RES50000mN2650000E4450000.tif"),
    ]
}

fn default_close_paths() -> Vec<PathBuf> {
    vec![PathBuf::from("tiles/big_size/DGM_R5.tif")]
}

fn default_base_paths() -> Vec<PathBuf> {
    (45u32..=49)
        .flat_map(|lat| {
            (9u32..=13).map(move |lon| {
                PathBuf::from(format!(
                    "tiles/Copernicus_DSM_COG_10_N{lat:02}_00_E{lon:03}_00_DEM/\
                     Copernicus_DSM_COG_10_N{lat:02}_00_E{lon:03}_00_DEM.tif"
                ))
            })
        })
        .collect()
}

impl Default for DemoViewConfig {
    fn default() -> Self {
        DemoViewConfig {
            camera_lat: default_demo_cam_lat(),
            camera_lon: default_demo_cam_lon(),
            camera_elev: default_demo_cam_elev(),
            fine_tile_paths: default_fine_paths(),
            close_tile_paths: default_close_paths(),
            base_tile_paths: default_base_paths(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SelectedView {
    #[default]
    None,
    DemoView,
    CustomFile,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct LauncherSettings {
    #[serde(default)]
    pub demo_view: DemoViewConfig,
    #[serde(default)]
    pub skip_launcher: bool,
    #[serde(default = "default_vsync")]
    pub vsync: bool,
    #[serde(default = "default_true_shadows")]
    pub shadows_enabled: bool,
    #[serde(default = "default_true_fog")]
    pub fog_enabled: bool,
    #[serde(default = "default_vat_mode")]
    pub vat_mode: u32, // 0=Ultra 1=High 2=Mid 3=Low
    #[serde(default)]
    pub lod_mode: u32, // 0=Ultra 1=High 2=Mid 3=Low
    #[serde(default = "default_ao_mode")]
    pub ao_mode: u32, // 0=Off 1=SSAO×8 2=SSAO×16 3=HBAO×4 4=HBAO×8 5=True Hemi
    #[serde(default = "default_true_tiles")]
    pub tiles_refinement: bool,
    #[serde(default = "default_tile_5m_path")]
    pub tile_5m_path: PathBuf,
    #[serde(default)]
    pub selected_view: SelectedView,
}

impl Default for LauncherSettings {
    fn default() -> Self {
        LauncherSettings {
            demo_view: DemoViewConfig::default(),
            skip_launcher: false,
            vsync: false,
            shadows_enabled: true,
            fog_enabled: true,
            vat_mode: 1, // High
            lod_mode: 0, // Ultra
            ao_mode: 3,  // HBAO×4
            tiles_refinement: true,
            tile_5m_path: PathBuf::from(DEFAULT_TILE_5M_PATH),
            selected_view: SelectedView::None,
        }
    }
}

fn default_vsync() -> bool {
    LauncherSettings::default().vsync
}
fn default_true_shadows() -> bool {
    LauncherSettings::default().shadows_enabled
}
fn default_true_fog() -> bool {
    LauncherSettings::default().fog_enabled
}
fn default_true_tiles() -> bool {
    LauncherSettings::default().tiles_refinement
}
fn default_vat_mode() -> u32 {
    LauncherSettings::default().vat_mode
}
fn default_ao_mode() -> u32 {
    LauncherSettings::default().ao_mode
}
fn default_tile_5m_path() -> PathBuf {
    LauncherSettings::default().tile_5m_path
}

impl LauncherSettings {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("dem_renderer").join("config.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::config_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }
}

pub enum LauncherOutcome {
    Exit,
    Start {
        window: Arc<Window>,
        settings: LauncherSettings,
        prepared: crate::viewer::PreparedScene,
        /// Launcher's surface, handed to the viewer so it can be reconfigured in-place
        /// (no drop+recreate = no visible flash during the transition).
        surface: wgpu::Surface<'static>,
    },
}
