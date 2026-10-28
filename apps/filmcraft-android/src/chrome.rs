//! What the Android app puts around the shared interface: it swaps the desktop header and docking
//! area for the phone layout ([`crate::phone`]) and keeps the interface above the soft keyboard.
//! Plain egui, so it is built and tested on the host.

use egui::Rect;
use filmcraft_ui_egui::FilmcraftApp;

#[derive(Default)]
pub struct Chrome {
    /// How much of the screen's bottom the soft keyboard covers, in points (0: hidden). The
    /// interface keeps above it, and the tool strip steps aside, so a field being typed in is seen.
    pub keyboard: f32,
}

impl Chrome {
    /// Draw the interface in the screen above the soft keyboard.
    pub fn ui(&mut self, app: &mut FilmcraftApp, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // no mouse or keyboard: the interface says "tap" where it said "click" or "drag", and
        // the phone layout replaces the desktop header and dock
        crate::phone::install(app);
        let screen = ui.max_rect();
        let keyboard = if self.keyboard.is_finite() { self.keyboard.clamp(0.0, screen.height() * 0.7) } else { 0.0 };
        crate::phone::set_keyboard(ui.ctx(), keyboard > 0.0);
        let above = Rect::from_min_max(screen.min, egui::pos2(screen.max.x, screen.max.y - keyboard));
        ui.scope_builder(egui::UiBuilder::new().max_rect(above), |ui| eframe::App::ui(app, ui, frame));
    }
}
