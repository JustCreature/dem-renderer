use crate::launcher::widgets::menu_row;
use egui::Ui;

#[derive(Default)]
pub struct MainMenuAnim {
    pub row: [f32; 4],
}

pub enum MainMenuEvent {
    SelectDem,
    Settings,
    Start,
    Exit,
}

/// Renders only the navigation rows (01 Select DEM, 02 Settings).
/// Start / Exit and the footer are rendered by mod.rs in the bottom-up zone.
pub fn show(ui: &mut Ui, anim: &mut MainMenuAnim) -> Option<MainMenuEvent> {
    let mut event = None;

    if menu_row(
        ui,
        "01",
        "Select DEM Files",
        ".tif · .hgt · .asc",
        false,
        false,
        &mut anim.row[0],
    ) {
        event = Some(MainMenuEvent::SelectDem);
    }
    if menu_row(
        ui,
        "02",
        "Settings",
        "render · view · export",
        false,
        false,
        &mut anim.row[1],
    ) {
        event = Some(MainMenuEvent::Settings);
    }

    event
}
