//! What the Android app puts around the shared interface. Plain egui, so it is built and tested on
//! the host.
//!
//! **A tablet or a screen as big (600 dp and up on the short side, like the Galaxy Tab S9 Ultra)
//! gets the desktop interface**: the same header with menus, docked panels, workspaces and
//! shortcuts as on a computer, for a mouse and keyboard (touch still works: see [`crate::touch`]).
//! **A phone gets the [phone layout](crate::phone)**, as a desktop layout does not fit its screen.
//! The choice is made every frame from the screen's size, so folding, split screen or a window
//! resize switch with it. The interface also stays above the soft keyboard.

use egui::Rect;
use filmcraft_ui_egui::FilmcraftApp;

/// The short side of the screen, in dp, from which the desktop interface is used.
pub const DESKTOP_MIN_DP: f32 = 600.0;
/// The desktop interface is laid out for at least this many points; a screen that is smaller
/// zooms it out while Settings ▸ Appearance ▸ UI Scale is Auto, down to [`MIN_AUTO_ZOOM`].
pub const MIN_LOGICAL_SIZE: (f32, f32) = (1000.0, 520.0);
pub const MIN_AUTO_ZOOM: f32 = 0.6;

#[derive(Default)]
pub struct Chrome {
    /// How much of the screen's bottom the soft keyboard covers, in points (0: hidden). The
    /// interface keeps above it, and the phone's tool strip steps aside, so a field being typed
    /// in is seen.
    pub keyboard: f32,
    /// The screen size (physical pixels), native scale and UI Scale setting the automatic zoom was
    /// last decided for.
    auto_zoom: Option<(egui::Vec2, f32, String)>,
}

/// Whether a screen of `dp` (width, height) gets the phone layout.
pub fn is_phone(dp: egui::Vec2) -> bool {
    dp.x.min(dp.y) < DESKTOP_MIN_DP
}

impl Chrome {
    /// Draw the interface in the screen above the soft keyboard.
    pub fn ui(&mut self, app: &mut FilmcraftApp, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let native = ctx.native_pixels_per_point().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(1.0);
        let screen = ui.max_rect();
        let dp = ctx.input(|i| i.viewport_rect().size()) * ctx.pixels_per_point() / native;
        let phone = is_phone(dp);
        if phone {
            crate::phone::install(app);
        } else {
            app.phone_layout = None;
            app.touch_ui = false;
        }
        let keyboard = if self.keyboard.is_finite() { self.keyboard.clamp(0.0, screen.height() * 0.7) } else { 0.0 };
        crate::phone::set_keyboard(&ctx, keyboard > 0.0);
        let above = Rect::from_min_max(screen.min, egui::pos2(screen.max.x, screen.max.y - keyboard));
        ui.scope_builder(egui::UiBuilder::new().max_rect(above), |ui| eframe::App::ui(app, ui, frame));
        self.apply_auto_zoom(app, &ctx, phone);
    }

    /// Settings ▸ Appearance ▸ UI Scale ▸ Auto on a screen smaller than the desktop interface is
    /// laid out for: zoom it out so the panels fit. The phone layout is sized in dp and is never
    /// zoomed. Decided again when the screen size or the setting changes, after the app's own UI
    /// Scale handling in the same pass (going back to Auto resets the zoom there); a fixed UI Scale
    /// is left to the app.
    fn apply_auto_zoom(&mut self, app: &FilmcraftApp, ctx: &egui::Context, phone: bool) {
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
        let zoom = if phone { 1.0 } else { auto_zoom(physical / native) };
        log::info!("screen {}x{} px at {native}x: {} layout, interface zoom {zoom:.2}", physical.x, physical.y, if phone { "phone" } else { "desktop" });
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
    fn tablets_get_the_desktop_and_phones_the_phone_layout() {
        // Galaxy Tab S9 Ultra: 2960 x 1848 px at 2x
        assert!(!is_phone(egui::vec2(1480.0, 924.0)) && !is_phone(egui::vec2(924.0, 1480.0)));
        // a phone, both ways up, and a 7-inch tablet is still the phone layout (it is 600 dp)
        assert!(is_phone(egui::vec2(393.0, 873.0)) && is_phone(egui::vec2(873.0, 393.0)));
        assert!(is_phone(egui::vec2(599.0, 960.0)) && !is_phone(egui::vec2(600.0, 960.0)));
    }

    #[test]
    fn a_big_screen_is_not_zoomed_and_a_small_one_is_zoomed_out_to_the_limit() {
        assert_eq!(auto_zoom(egui::vec2(1480.0, 924.0)), 1.0);
        assert_eq!(auto_zoom(egui::vec2(1600.0, 900.0)), 1.0);
        assert!((auto_zoom(egui::vec2(800.0, 600.0)) - 0.8).abs() < 1e-6);
        assert_eq!(auto_zoom(egui::vec2(300.0, 100.0)), MIN_AUTO_ZOOM);
    }
}
