mod background;
mod config;
mod renderer;
mod screens;
mod style;
mod widgets;

pub use config::{LauncherOutcome, LauncherSettings};

use crate::consts::{DEFAULT_CAM_LAT, DEFAULT_CAM_LON, DEFAULT_TILE_5M_PATH, WINDOW_H, WINDOW_W};

use std::sync::{mpsc, Arc};

use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::ActiveEventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowAttributes},
};

use background::{
    draw_background, draw_corner_marks, draw_gradient_shade, draw_metadata_labels, draw_vignette,
};
use renderer::EguiRenderer;
use screens::{
    loading,
    main_menu::{self, MainMenuAnim},
    select_dem::{self, SelectDemAnim},
};
use widgets::{brand_block, breadcrumb, hairline_rule, menu_row, status_footer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Main,
    SelectDem,
    Settings,
    Loading,
}

struct LoadProgress {
    frac: f32,
    label: String,
    prepared: Option<crate::viewer::PreparedScene>,
}

pub struct LauncherApp {
    // Window / egui (initialised in resumed())
    window: Option<Arc<Window>>,
    egui: Option<EguiRenderer>,
    // Kept alive so device can be cloned for the background loading thread.
    gpu_ctx: Option<render_gpu::GpuContext>,

    // Navigation
    screen: Screen,
    modal_open: bool,

    // Per-screen animation state
    main_anim: MainMenuAnim,
    select_anim: SelectDemAnim,

    // Settings
    settings: LauncherSettings,

    // Loading phase
    load_rx: Option<mpsc::Receiver<LoadProgress>>,
    load_progress: f32,
    load_status: String,
    prepared: Option<crate::viewer::PreparedScene>,

    // Final outcome — set when user confirms start or exit
    pub outcome: Option<LauncherOutcome>,
}

impl LauncherApp {
    pub fn new() -> Self {
        LauncherApp {
            window: None,
            egui: None,
            gpu_ctx: None,
            screen: Screen::Main,
            modal_open: false,
            main_anim: MainMenuAnim::default(),
            select_anim: SelectDemAnim::default(),
            settings: LauncherSettings::load(),
            load_rx: None,
            load_progress: 0.0,
            load_status: String::from("Preparing…"),
            prepared: None,
            outcome: None,
        }
    }

    fn handle_main_event(&mut self, event: main_menu::MainMenuEvent, el: &ActiveEventLoop) {
        use main_menu::MainMenuEvent::*;
        match event {
            SelectDem => self.screen = Screen::SelectDem,
            Settings => self.screen = Screen::Settings,
            Start => {
                self.settings.save();
                self.begin_loading(el);
            }
            Exit => {
                self.settings.save();
                self.outcome = Some(LauncherOutcome::Exit);
                el.exit();
            }
        }
    }

    fn handle_select_event(&mut self, event: select_dem::SelectDemEvent, el: &ActiveEventLoop) {
        use select_dem::SelectDemEvent::*;
        match event {
            ChooseFiles => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("DEM tiles", &["tif", "hgt", "asc"])
                    .pick_file()
                {
                    self.settings.tile_5m_path = path;
                    self.settings.save();
                }
            }
            Reset => {
                self.settings.tile_5m_path = std::path::PathBuf::from(DEFAULT_TILE_5M_PATH);
                self.settings.save();
            }
            DemoView => {
                self.settings.save();
                self.begin_loading(el);
            }
        }
    }

    fn begin_loading(&mut self, _el: &ActiveEventLoop) {
        self.screen = Screen::Loading;
        self.load_progress = 0.0;
        self.load_status = "Reading terrain data…".to_string();

        let (tx, rx) = mpsc::channel::<LoadProgress>();
        self.load_rx = Some(rx);

        // Clone the GPU context so the background thread builds the GpuScene on the same
        // underlying wgpu device as the launcher — this lets us hand the surface over to
        // the viewer without a drop+recreate (and the visual flash that comes with it).
        let ctx = self
            .gpu_ctx
            .as_ref()
            .expect("gpu_ctx must be initialised before begin_loading")
            .clone();
        let tile_path = self.settings.tile_5m_path.clone();

        std::thread::spawn(move || {
            let prepared = crate::viewer::scene_init::prepare_scene_with_ctx(
                ctx,
                &tile_path,
                WINDOW_W,
                WINDOW_H,
                DEFAULT_CAM_LAT,
                DEFAULT_CAM_LON,
                |frac, label| {
                    let _ = tx.send(LoadProgress {
                        frac,
                        label: label.to_string(),
                        prepared: None,
                    });
                },
            );
            let _ = tx.send(LoadProgress {
                frac: 1.0,
                label: "Ready".to_string(),
                prepared: Some(prepared),
            });
        });
    }

    fn poll_loading(&mut self, el: &ActiveEventLoop) {
        let Some(rx) = &self.load_rx else { return };
        let mut latest = None;
        while let Ok(msg) = rx.try_recv() {
            latest = Some(msg);
        }
        if let Some(msg) = latest {
            self.load_progress = msg.frac;
            self.load_status = msg.label.clone();
            if let Some(p) = msg.prepared {
                self.prepared = Some(p);
            }
            if msg.frac >= 1.0 {
                self.finish_start(el);
            }
        }
    }

    fn finish_start(&mut self, _el: &ActiveEventLoop) {
        let prepared = self
            .prepared
            .take()
            .expect("terrain not loaded before finish_start");
        // Extract the surface BEFORE dropping EguiRenderer so the platform surface
        // (CAMetalLayer / VkSurfaceKHR) stays alive for the viewer to reconfigure.
        let surface = self
            .egui
            .take()
            .expect("egui must exist at finish_start")
            .take_surface();
        let window = self.window.clone().expect("window must exist");
        self.outcome = Some(LauncherOutcome::Start {
            window,
            settings: self.settings.clone(),
            prepared,
            surface,
        });
        // Do NOT call el.exit() — the combined App handler in main.rs observes outcome
        // and switches Phase without exiting the event loop, so winit never hides the window.
    }

    fn handle_key(&mut self, el: &ActiveEventLoop, key: PhysicalKey) {
        let PhysicalKey::Code(code) = key else { return };

        if self.modal_open {
            if code == KeyCode::Escape {
                self.modal_open = false;
            }
            return;
        }

        match self.screen {
            Screen::Main => match code {
                KeyCode::Enter | KeyCode::NumpadEnter => self.begin_loading(el),
                KeyCode::Escape => {
                    self.outcome = Some(LauncherOutcome::Exit);
                    el.exit();
                }
                KeyCode::Digit1 | KeyCode::Numpad1 => self.screen = Screen::SelectDem,
                KeyCode::Digit2 | KeyCode::Numpad2 => self.screen = Screen::Settings,
                _ => {}
            },
            Screen::SelectDem | Screen::Settings => {
                if code == KeyCode::Escape {
                    self.screen = Screen::Main;
                }
            }
            Screen::Loading => {}
        }

        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl ApplicationHandler for LauncherApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        } // already created

        let window: Arc<Window> = Arc::new(
            el.create_window(
                WindowAttributes::default()
                    .with_title("DEM Renderer")
                    .with_inner_size(LogicalSize::new(WINDOW_W, WINDOW_H)),
            )
            .expect("failed to create launcher window"),
        );

        // One GPU context for the entire process lifetime — launcher and viewer share the same
        // underlying wgpu device via Arc-backed clones so the surface can be handed over
        // without being dropped and recreated.
        let gpu_ctx = render_gpu::GpuContext::new();

        let surface: wgpu::Surface<'static> = gpu_ctx
            .instance
            .create_surface(window.clone())
            .expect("failed to create surface");

        let caps = surface.get_capabilities(&gpu_ctx.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Bgra8Unorm)
            .unwrap_or(caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window.inner_size().width.max(1),
            height: window.inner_size().height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&gpu_ctx.device, &surface_config);

        self.egui = Some(EguiRenderer::new(
            gpu_ctx.device.clone(), // clone — Arc-backed, shares same underlying device
            gpu_ctx.queue.clone(),
            surface,
            format,
            surface_config,
            &window,
        ));
        self.gpu_ctx = Some(gpu_ctx);
        self.window = Some(window);

        if self.settings.skip_launcher {
            self.begin_loading(el);
        }
    }

    fn window_event(
        &mut self,
        el: &ActiveEventLoop,
        _id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if let Some(egui) = &mut self.egui {
            let resp = egui
                .state
                .on_window_event(self.window.as_ref().unwrap(), &event);
            if resp.consumed {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.settings.save();
                self.outcome = Some(LauncherOutcome::Exit);
                el.exit();
            }

            WindowEvent::Resized(size) => {
                if let Some(egui) = &mut self.egui {
                    egui.resize(size.width, size.height);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if key_event.state == winit::event::ElementState::Pressed {
                    self.handle_key(el, key_event.physical_key);
                }
            }

            WindowEvent::RedrawRequested => {
                if self.screen == Screen::Loading {
                    self.poll_loading(el);
                }

                let Some(egui_renderer) = &mut self.egui else {
                    return;
                };
                let Some(window) = &self.window else { return };

                let bg_tid = egui_renderer.bg_texture.id();
                let bg_w = egui_renderer.bg_w;
                let bg_h = egui_renderer.bg_h;

                // Capture fields before the closure because the closure borrows
                // `egui_renderer` mutably, preventing any further borrows of `self`.
                let screen = self.screen;
                let modal_open = &mut self.modal_open;
                let main_anim = &mut self.main_anim;
                let sel_anim = &mut self.select_anim;
                let settings = &mut self.settings;
                let load_progress = self.load_progress;
                let load_status = self.load_status.clone();
                // For the SelectDem screen: show the chosen filename and a reset button
                // only when the user has picked something other than the bundled default.
                let tile_5m_is_custom =
                    settings.tile_5m_path != std::path::PathBuf::from(DEFAULT_TILE_5M_PATH);
                let tile_5m_display = settings
                    .tile_5m_path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| settings.tile_5m_path.to_string_lossy().into_owned());

                let mut main_evt: Option<main_menu::MainMenuEvent> = None;
                let mut sel_evt: Option<select_dem::SelectDemEvent> = None;
                let mut back_clicked = false;

                egui_renderer.render(window, |ctx| {
                    // Background layers
                    let bg_layer = ctx.layer_painter(egui::LayerId::background());
                    draw_background(&bg_layer, bg_tid, bg_w, bg_h);
                    draw_gradient_shade(&bg_layer);
                    draw_vignette(&bg_layer);
                    draw_corner_marks(&bg_layer);
                    draw_metadata_labels(&bg_layer, screen_title(screen), 47.17, 11.44, None);

                    // Panel
                    let screen_rect = ctx.content_rect();
                    let panel_w = 540.0_f32;
                    let panel_right_margin = 56.0_f32;
                    let panel_x = screen_rect.max.x - panel_right_margin - panel_w;
                    let panel_top = (screen_rect.center().y - 220.0).max(screen_rect.min.y + 20.0);

                    egui::Area::new(egui::Id::new("launcher_panel"))
                        .fixed_pos(egui::pos2(panel_x, panel_top))
                        .order(egui::Order::Middle)
                        .show(ctx, |ui| {
                            egui::Frame::NONE
                                .fill(style::PANEL_BG)
                                .stroke(egui::Stroke::new(1.0, style::PANEL_BORDER))
                                .inner_margin(egui::Margin {
                                    left: 40,
                                    right: 40,
                                    top: 38,
                                    bottom: 32,
                                })
                                .show(ui, |ui| {
                                    ui.set_width(panel_w - 80.0);
                                    ui.set_min_height(440.0);

                                    // ── Header — identical layout for all screens ──────────
                                    // Sub-screens show a back breadcrumb; main/loading get an
                                    // equivalent spacer so the brand block always starts at the
                                    // same y-position.
                                    match screen {
                                        Screen::Main | Screen::Loading => {
                                            ui.add_space(26.0);
                                        }
                                        Screen::Settings => {
                                            if breadcrumb(ui, "← Back", "Settings") {
                                                back_clicked = true;
                                            }
                                            ui.add_space(12.0);
                                        }
                                        Screen::SelectDem => {
                                            if breadcrumb(ui, "← Back", "Select DEM Files") {
                                                back_clicked = true;
                                            }
                                            ui.add_space(12.0);
                                        }
                                    }
                                    match screen {
                                        Screen::Main => brand_block(
                                            ui,
                                            "Digital Elevation Model · Renderer",
                                            "DEM Renderer",
                                            ".",
                                            "v 0.4.2 · build 2026.05",
                                        ),
                                        Screen::Settings => brand_block(
                                            ui,
                                            "Render · View · Performance",
                                            "Settings",
                                            ".",
                                            "tune output for your hardware",
                                        ),
                                        Screen::SelectDem => brand_block(
                                            ui,
                                            "Source · Step 01 of 02",
                                            "Choose Source",
                                            ".",
                                            "how do you want to load terrain data",
                                        ),
                                        Screen::Loading => brand_block(
                                            ui,
                                            "DEM · RENDERER",
                                            "Loading",
                                            ".",
                                            "preparing terrain data",
                                        ),
                                    }
                                    ui.add_space(14.0);
                                    hairline_rule(ui);
                                    ui.add_space(8.0);

                                    // ── Content — screen-specific rows only ────────────────
                                    match screen {
                                        Screen::Main => {
                                            main_evt = screens::main_menu::show(ui, main_anim);
                                        }
                                        Screen::SelectDem => {
                                            sel_evt = screens::select_dem::show(
                                                ui,
                                                sel_anim,
                                                modal_open,
                                                &tile_5m_display,
                                                tile_5m_is_custom,
                                            );
                                        }
                                        Screen::Settings => {
                                            screens::settings::show(ui, settings);
                                        }
                                        Screen::Loading => {
                                            loading::show(ui, load_progress, &load_status);
                                        }
                                    }

                                    // ── Footer — always bottom-pinned via bottom_up ────────
                                    // Compute strings after show() so SelectDem's free-space
                                    // cache is already populated.
                                    let (fstatus, fright) = screen_footer(screen, ui.ctx());

                                    ui.with_layout(
                                        egui::Layout::bottom_up(egui::Align::LEFT),
                                        |ui| {
                                            status_footer(ui, fstatus, &fright);

                                            let (_, p) = ui.allocate_painter(
                                                egui::vec2(ui.available_width(), 1.0),
                                                // egui::Sense::hover() — egui requires you to declare what interaction the region participates in.
                                                // hover() is the lightest option: detect hover, nothing else.
                                                // Sense::empty() would also work here since we never look at the response.
                                                // hover() is just idiomatic for "I need a region to paint into, not a real widget."
                                                egui::Sense::hover(),
                                            );
                                            p.line_segment(
                                                [
                                                    p.clip_rect().left_top(),
                                                    p.clip_rect().right_top(),
                                                ],
                                                egui::Stroke::new(1.0, style::HAIRLINE),
                                            );

                                            // Main menu: Start and Exit live in the footer zone so
                                            // they pin to the bottom while rows 01/02 stay at top.
                                            if screen == Screen::Main {
                                                if menu_row(
                                                    ui,
                                                    "Esc",
                                                    "Exit",
                                                    "",
                                                    false,
                                                    true,
                                                    &mut main_anim.row[3],
                                                ) {
                                                    main_evt = Some(main_menu::MainMenuEvent::Exit);
                                                }
                                                if menu_row(
                                                    ui,
                                                    "Enter",
                                                    "Start",
                                                    "load & render",
                                                    true,
                                                    false,
                                                    &mut main_anim.row[2],
                                                ) {
                                                    main_evt =
                                                        Some(main_menu::MainMenuEvent::Start);
                                                }
                                            }
                                        },
                                    );
                                });
                        });
                });

                if back_clicked {
                    if self.screen == Screen::Settings {
                        self.settings.save();
                    }
                    self.screen = Screen::Main;
                }
                if let Some(e) = main_evt {
                    self.handle_main_event(e, el);
                }
                if let Some(e) = sel_evt {
                    self.handle_select_event(e, el);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

fn screen_title(s: Screen) -> &'static str {
    match s {
        Screen::Main => "DEM · MAIN MENU",
        Screen::SelectDem => "DEM · SOURCE · SD-01",
        Screen::Settings => "DEM · SETTINGS · ST-01",
        Screen::Loading => "DEM · LOADING",
    }
}

fn screen_footer(s: Screen, ctx: &egui::Context) -> (&'static str, String) {
    match s {
        Screen::Main => ("SYSTEM READY", "47.17°N · 11.44°E".to_string()),
        Screen::Settings => ("PRESET · CUSTOM", "EST. 2.4 GB VRAM".to_string()),
        Screen::SelectDem => {
            let free = ctx.data(|d| {
                d.get_temp::<String>(egui::Id::new("free_space_cache"))
                    .unwrap_or_else(|| "—".to_string())
            });
            ("SYSTEM READY", format!("FREE  {free}"))
        }
        Screen::Loading => ("LOADING", String::new()),
    }
}
