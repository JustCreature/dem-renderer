mod consts;
mod launcher;
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

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

// ── Phase state machine ──────────────────────────────────────────────────────
// A single ApplicationHandler delegates to the active phase.  Switching from
// Launcher to Viewer never calls el.exit(), so winit never hides the window and
// the surface is transferred in-place — the platform layer (CAMetalLayer, etc.)
// stays alive continuously, eliminating the visible flash.

enum Phase {
    Launcher(launcher::LauncherApp),
    Viewer(viewer::Viewer),
}

struct App {
    phase: Phase,
    vsync_override: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        match &mut self.phase {
            Phase::Launcher(l) => l.resumed(el),
            Phase::Viewer(v) => v.resumed(el),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match &mut self.phase {
            Phase::Launcher(l) => {
                l.window_event(el, id, event);
                self.try_transition(el);
            }
            Phase::Viewer(v) => v.window_event(el, id, event),
        }
    }

    fn device_event(
        &mut self,
        el: &ActiveEventLoop,
        id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let Phase::Viewer(v) = &mut self.phase {
            v.device_event(el, id, event);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        match &mut self.phase {
            Phase::Launcher(l) => l.about_to_wait(el),
            Phase::Viewer(v) => v.about_to_wait(el),
        }
    }
}

impl App {
    /// Called after every launcher window_event.  If the launcher has set an outcome,
    /// handle it: exit the loop (Exit) or switch to Viewer phase without restarting
    /// the loop (Start) — the window stays on-screen the whole time.
    fn try_transition(&mut self, el: &ActiveEventLoop) {
        let Phase::Launcher(l) = &mut self.phase else {
            return;
        };
        let Some(outcome) = l.outcome.take() else {
            return;
        };

        match outcome {
            launcher::LauncherOutcome::Exit => el.exit(),
            launcher::LauncherOutcome::Start {
                window,
                settings,
                prepared,
                surface,
            } => {
                let alignment_key = settings.alignment_key();
                let (align5m, align1m) = settings.current_alignment();
                let viewer = viewer::Viewer::from_launcher(
                    prepared,
                    window,
                    surface,
                    settings.tile_5m_path.as_path(),
                    Some(settings.tiles_1m_dir.as_path()),
                    settings.tiles_refinement,
                    settings.selected_view.clone(),
                    settings.vsync || self.vsync_override,
                    settings.shadows_enabled,
                    settings.fog_enabled,
                    settings.vat_mode,
                    settings.lod_mode,
                    settings.ao_mode,
                    align5m,
                    align1m,
                    alignment_key,
                );
                self.phase = Phase::Viewer(viewer);
                // Drive resumed() manually so the surface and HUD are configured
                // before the next RedrawRequested arrives.
                if let Phase::Viewer(v) = &mut self.phase {
                    v.resumed(el);
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let vsync = args.contains(&"--vsync".to_string());

    let event_loop = winit::event_loop::EventLoop::new().expect("event loop");
    let mut app = App {
        phase: Phase::Launcher(launcher::LauncherApp::new()),
        vsync_override: vsync,
    };
    event_loop.run_app(&mut app).expect("app run failed");
}
