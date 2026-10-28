//! What the Android app puts around the shared interface: the touch bar along the bottom, and the
//! interface zoom that fits a desktop layout on a small screen. Plain egui, so it is built and
//! tested on the host.

use egui::Rect;
use filmcraft_ui_egui::FilmcraftApp;

use crate::touch_bar::TouchBar;

/// The interface is laid out for at least this many points; smaller screens (phones) zoom it out
/// while Settings ▸ Appearance ▸ UI Scale is Auto, down to [`MIN_AUTO_ZOOM`].
pub const MIN_LOGICAL_SIZE: (f32, f32) = (1000.0, 520.0);
pub const MIN_AUTO_ZOOM: f32 = 0.6;

#[derive(Default)]
pub struct Chrome {
    pub bar: TouchBar,
    /// How much of the screen's bottom the soft keyboard covers, in points (0: hidden). The
    /// interface keeps above it, and the touch bar steps aside, so a field being typed in is seen.
    pub keyboard: f32,
    /// The screen size (physical pixels), native scale and UI Scale setting the automatic zoom was
    /// last decided for.
    auto_zoom: Option<(egui::Vec2, f32, String)>,
}

impl Chrome {
    /// Draw the interface in the screen above the touch bar, then the bar.
    pub fn ui(&mut self, app: &mut FilmcraftApp, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // no mouse or keyboard: the interface says "tap" where it said "click" or "drag"
        app.touch_ui = true;
        let screen = ui.max_rect();
        let keyboard = if self.keyboard.is_finite() { self.keyboard.clamp(0.0, screen.height() * 0.7) } else { 0.0 };
        let bar_h = if keyboard > 0.0 { 0.0 } else { self.bar.reserved(ui.ctx()).min(screen.height() * 0.4) };
        let above = Rect::from_min_max(screen.min, egui::pos2(screen.max.x, screen.max.y - bar_h - keyboard));
        ui.scope_builder(egui::UiBuilder::new().max_rect(above), |ui| eframe::App::ui(app, ui, frame));
        if keyboard == 0.0 {
            self.bar.show(app, ui, screen);
        }
        let ctx = ui.ctx().clone();
        self.apply_auto_zoom(app, &ctx);
    }

    /// Settings ▸ Appearance ▸ UI Scale ▸ Auto on a small screen: zoom the interface out so the
    /// panels fit (a phone is about 870 × 390 points in landscape). Decided again when the screen
    /// size or the setting changes, after the app's own UI Scale handling in the same pass (going
    /// back to Auto resets the zoom there); a fixed UI Scale is left to the app.
    fn apply_auto_zoom(&mut self, app: &FilmcraftApp, ctx: &egui::Context) {
        let setting = &app.session.prefs.appearance.ui_scale;
        let native = ctx.native_pixels_per_point().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(1.0);
        let physical = ctx.input(|i| i.viewport_rect().size()) * ctx.pixels_per_point();
        if physical.x < 1.0 || physical.y < 1.0 || self.auto_zoom.as_ref().is_some_and(|(p, n, s)| *p == physical && *n == native && s == setting) {
            return;
        }
        self.auto_zoom = Some((physical, native, setting.clone()));
        if app.session.prefs.appearance.ui_scale_factor().is_some() {
            return;
        }
        let points = physical / native;
        let zoom = auto_zoom(points);
        log::info!("screen {}x{} px at {native}x: interface zoom {zoom:.2}", physical.x, physical.y);
        ctx.set_zoom_factor(zoom);
    }
}

/// The interface zoom that fits the desktop layout in a screen of `points` (at the system's scale).
pub fn auto_zoom(points: egui::Vec2) -> f32 {
    (points.x / MIN_LOGICAL_SIZE.0).min(points.y / MIN_LOGICAL_SIZE.1).clamp(MIN_AUTO_ZOOM, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_screen_is_not_zoomed_and_a_phone_is_zoomed_out_to_the_limit() {
        assert_eq!(auto_zoom(egui::vec2(1600.0, 900.0)), 1.0);
        assert!((auto_zoom(egui::vec2(873.0, 393.0)) - 393.0 / 520.0).abs() < 1e-6);
        assert_eq!(auto_zoom(egui::vec2(300.0, 100.0)), MIN_AUTO_ZOOM);
        assert!((auto_zoom(egui::vec2(800.0, 600.0)) - 0.8).abs() < 1e-6);
    }
}
