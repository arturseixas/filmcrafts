//! The touch bar: the edits a phone user needs most, as large buttons along the bottom of the
//! screen. The desktop interface asks for a mouse and a keyboard (a drag to put a clip on the
//! timeline, `,` to insert, Delete to remove); here each is one tap.
//!
//! Sizes are in dp (the system's density-independent pixels) and divided by the interface zoom, so
//! the buttons stay finger-sized however far the rest of the interface is zoomed out to fit a small
//! screen. The bar takes its own strip of the screen ([`TouchBar::reserved`]); the interface lays
//! itself out in what is left.
//!
//! Every button runs a command by id, like the menus (`menus::invoke`), except Add, Split and
//! Delete, which [`crate::edit_actions`] implements.

use egui::{Align, Align2, Color32, Id, Layout, Rect, RichText, Sense, UiBuilder, vec2};
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::i18n::t;
use filmcraft_ui_egui::menus;
use serde_json::json;

use crate::edit_actions::{self, Place};

/// Height of the bar, and of its buttons, in dp. Android's guideline for touch targets is 48 dp.
const BAR_DP: f32 = 60.0;
const BUTTON_DP: f32 = 48.0;
const GAP_DP: f32 = 6.0;
const TEXT_DP: f32 = 14.0;
/// How long a message stays above the bar, in seconds.
const TOAST_SECONDS: f64 = 4.0;

/// One tap on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Import,
    Add(Place),
    Play,
    StepBack,
    StepForward,
    Split,
    Delete,
    Undo,
    Redo,
    ZoomIn,
    ZoomOut,
    Fit,
    Save,
    Export,
    Larger,
    Smaller,
    Hide,
}

#[derive(Default)]
pub struct TouchBar {
    hidden: bool,
    /// The last message and the time (egui seconds) it is shown until; `true`: an error.
    toast: Option<(String, f64, bool)>,
}

/// `dp` density-independent pixels, in points at the current interface zoom.
fn dp(ctx: &egui::Context, v: f32) -> f32 {
    v / ctx.zoom_factor().clamp(0.1, 10.0)
}

impl TouchBar {
    /// Height of the strip the bar takes at the bottom of the screen (none while it is hidden).
    pub fn reserved(&self, ctx: &egui::Context) -> f32 {
        if self.hidden { 0.0 } else { dp(ctx, BAR_DP) }
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Draw the bar (or the small tab that brings it back) in the bottom `bar` strip of `screen`
    /// and run what was tapped.
    pub fn show(&mut self, app: &mut FilmcraftApp, ui: &mut egui::Ui, screen: Rect) {
        let ctx = ui.ctx().clone();
        let act = if self.hidden { self.tab(app, ui, screen) } else { self.bar(app, ui, screen) };
        if let Some(act) = act {
            self.run(app, &ctx, act);
        }
        self.show_toast(app, &ctx);
    }

    fn tab(&mut self, app: &mut FilmcraftApp, ui: &mut egui::Ui, screen: Rect) -> Option<Act> {
        let size = vec2(dp(ui.ctx(), 56.0), dp(ui.ctx(), 28.0));
        let rect = Rect::from_min_size(screen.right_bottom() - size, size);
        let tk = app.tokens;
        let resp = ui.interact(rect, Id::unique("touch-bar-show"), Sense::click());
        ui.painter().rect_filled(rect, dp(ui.ctx(), 6.0), tk.header_bg);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "▴", egui::FontId::proportional(dp(ui.ctx(), 18.0)), tk.text);
        app.auto.add("touchbar.show", rect, "Show the touch bar");
        resp.clicked().then_some(Act::Hide)
    }

    fn bar(&mut self, app: &mut FilmcraftApp, ui: &mut egui::Ui, screen: Rect) -> Option<Act> {
        let ctx = ui.ctx().clone();
        let h = dp(&ctx, BAR_DP);
        let strip = Rect::from_min_max(egui::pos2(screen.min.x, screen.max.y - h), screen.max);
        let tk = app.tokens;
        ui.painter().rect_filled(strip, 0.0, tk.header_bg);
        ui.painter().line_segment([strip.left_top(), strip.right_top()], egui::Stroke::new(1.0, tk.separator));
        let playing = app.playback.playing;
        let button = vec2(dp(&ctx, 64.0), dp(&ctx, BUTTON_DP));
        let mut chosen = None;
        let mut inner =
            ui.new_child(UiBuilder::new().max_rect(strip.shrink2(vec2(dp(&ctx, 6.0), (h - button.y) * 0.5))).layout(Layout::left_to_right(Align::Center)));
        egui::ScrollArea::horizontal().id_salt("touch-bar-scroll").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
            &mut inner,
            |ui| {
                ui.spacing_mut().item_spacing.x = dp(&ctx, GAP_DP);
                ui.spacing_mut().button_padding = vec2(dp(&ctx, 10.0), 0.0);
                put(ui, app, &mut chosen, "import", t("Import"), Act::Import, false, button);
                // Add: to the end of the timeline; the arrow offers the other two places
                put(ui, app, &mut chosen, "add", t("Add"), Act::Add(Place::End), true, button);
                let more_w = dp(&ctx, 28.0);
                let arrow = bar_button(ui, "▾", vec2(more_w, button.y), Some(tk.accent), tk.text, tk.hover);
                app.auto.add("touchbar.add.more", arrow.rect, "More ways to add");
                egui::Popup::menu(&arrow).width(dp(&ctx, 240.0)).show(|ui| {
                    let row = vec2(ui.available_width().max(dp(&ctx, 220.0)), dp(&ctx, BUTTON_DP));
                    for (id, label, place) in
                        [("insert", t("Insert at the playhead"), Place::Insert), ("overwrite", t("Overwrite at the playhead"), Place::Overwrite)]
                    {
                        let r = ui.add(egui::Button::new(RichText::new(label).size(dp(&ctx, TEXT_DP))).min_size(row));
                        app.auto.add(&format!("touchbar.add.{id}"), r.rect, label);
                        if r.clicked() {
                            chosen = Some(Act::Add(place));
                            ui.close();
                        }
                    }
                });
                group(ui, &ctx);
                put(ui, app, &mut chosen, "play", if playing { t("Pause") } else { t("Play") }, Act::Play, false, button);
                let step = vec2(dp(&ctx, 48.0), button.y);
                put(ui, app, &mut chosen, "stepback", "‹", Act::StepBack, false, step);
                put(ui, app, &mut chosen, "stepforward", "›", Act::StepForward, false, step);

                group(ui, &ctx);
                put(ui, app, &mut chosen, "split", t("Split"), Act::Split, false, button);
                put(ui, app, &mut chosen, "delete", t("Delete"), Act::Delete, false, button);
                group(ui, &ctx);
                put(ui, app, &mut chosen, "undo", t("Undo"), Act::Undo, false, button);
                put(ui, app, &mut chosen, "redo", t("Redo"), Act::Redo, false, button);
                group(ui, &ctx);
                let small = vec2(dp(&ctx, 48.0), button.y);
                for (id, label, act) in [("zoomout", t("Zoom −"), Act::ZoomOut), ("zoomin", t("Zoom +"), Act::ZoomIn), ("fit", t("Fit"), Act::Fit)] {
                    let w = if id == "fit" { small } else { button };
                    let r = bar_button(ui, label, w, Some(tk.field_bg), tk.text, tk.hover);
                    app.auto.add(&format!("touchbar.{id}"), r.rect, label);
                    if r.clicked() {
                        chosen = Some(act);
                    }
                }
                // everything else
                let more = bar_button(ui, "⋯", small, Some(tk.field_bg), tk.text, tk.hover);
                app.auto.add("touchbar.menu", more.rect, "More");
                egui::Popup::menu(&more).width(dp(&ctx, 260.0)).show(|ui| {
                    let row = vec2(ui.available_width().max(dp(&ctx, 240.0)), dp(&ctx, BUTTON_DP));
                    for (id, label, act) in [
                        ("save", t("Save project"), Act::Save),
                        ("export", t("Export…"), Act::Export),
                        ("larger", t("Interface larger"), Act::Larger),
                        ("smaller", t("Interface smaller"), Act::Smaller),
                        ("hide", t("Hide this bar"), Act::Hide),
                    ] {
                        let r = ui.add(egui::Button::new(RichText::new(label).size(dp(&ctx, TEXT_DP))).min_size(row));
                        app.auto.add(&format!("touchbar.menu.{id}"), r.rect, label);
                        if r.clicked() {
                            chosen = Some(act);
                            ui.close();
                        }
                    }
                });
            },
        );
        chosen
    }

    /// Say something above the bar for a few seconds (`error`: in red).
    pub fn notify(&mut self, ctx: &egui::Context, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), ctx.input(|i| i.time) + TOAST_SECONDS, error));
        ctx.request_repaint();
    }

    fn run(&mut self, app: &mut FilmcraftApp, ctx: &egui::Context, act: Act) {
        let result: Result<Option<String>, String> = match act {
            Act::Import => menus::invoke(app, ctx, "file.import", json!({})).map(|_| None),
            Act::Add(how) => edit_actions::add_to_timeline(&mut app.session, how).map(Some),
            Act::Split => edit_actions::split_at_playhead(&mut app.session).map(Some),
            Act::Delete => edit_actions::delete_selected(&mut app.session).map(Some),
            Act::Play => menus::invoke(app, ctx, "playback.toggle", json!({})).map(|_| None),
            Act::StepBack => menus::invoke(app, ctx, "playhead.stepBack", json!({})).map(|_| None),
            Act::StepForward => menus::invoke(app, ctx, "playhead.stepForward", json!({})).map(|_| None),
            Act::Undo => menus::invoke(app, ctx, "edit.undo", json!({})).map(|_| None),
            Act::Redo => menus::invoke(app, ctx, "edit.redo", json!({})).map(|_| None),
            Act::ZoomIn => menus::invoke(app, ctx, "view.zoomIn", json!({})).map(|_| None),
            Act::ZoomOut => menus::invoke(app, ctx, "view.zoomOut", json!({})).map(|_| None),
            Act::Fit => menus::invoke(app, ctx, "view.zoomToSequence", json!({})).map(|_| None),
            Act::Save => menus::invoke(app, ctx, "file.save", json!({})).map(|_| Some(t("Saved").to_string())),
            Act::Export => menus::invoke(app, ctx, "mode.export", json!({})).map(|_| None),
            Act::Larger => step_interface_size(app, ctx, true).map(Some),
            Act::Smaller => step_interface_size(app, ctx, false).map(Some),
            Act::Hide => {
                self.hidden = !self.hidden;
                Ok(None)
            }
        };
        let now = ctx.input(|i| i.time);
        match result {
            Ok(Some(msg)) => self.toast = Some((msg, now + TOAST_SECONDS, false)),
            Ok(None) => {}
            Err(e) => {
                app.ui.status = e.clone();
                self.toast = Some((e, now + TOAST_SECONDS, true));
            }
        }
        ctx.request_repaint();
    }

    /// The result of the last tap, above the bar for a few seconds.
    fn show_toast(&mut self, app: &FilmcraftApp, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let Some((text, until, error)) = self.toast.clone() else { return };
        if now >= until {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let tk = app.tokens;
        let lift = self.reserved(ctx) + dp(ctx, 12.0);
        egui::Area::new(Id::unique("touch-bar-toast")).order(egui::Order::Tooltip).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -lift)).interactable(false).show(
            ctx,
            |ui| {
                let fill = if error { Color32::from_rgb(120, 40, 40) } else { tk.accent };
                egui::Frame::new().fill(fill).corner_radius(dp(ctx, 8.0)).inner_margin(vec2(dp(ctx, 14.0), dp(ctx, 8.0))).show(ui, |ui| {
                    ui.label(RichText::new(text).size(dp(ctx, TEXT_DP)).color(Color32::WHITE));
                });
            },
        );
    }
}

/// A gap between groups of buttons.
fn group(ui: &mut egui::Ui, ctx: &egui::Context) {
    ui.add_space(dp(ctx, 10.0));
}

/// A labelled bar button that registers its automation id and records its tap.
#[allow(clippy::too_many_arguments)]
fn put(ui: &mut egui::Ui, app: &mut FilmcraftApp, chosen: &mut Option<Act>, id: &str, label: &str, act: Act, accent: bool, size: egui::Vec2) {
    let tk = app.tokens;
    let resp = bar_button(ui, label, size, Some(if accent { tk.accent } else { tk.field_bg }), tk.text, tk.hover);
    app.auto.add(&format!("touchbar.{id}"), resp.rect, label);
    if resp.clicked() {
        *chosen = Some(act);
    }
}

/// A bar button: text on a rounded fill, at least `size` big.
fn bar_button(ui: &mut egui::Ui, label: &str, size: egui::Vec2, fill: Option<Color32>, text: Color32, hover: Color32) -> egui::Response {
    let ctx = ui.ctx().clone();
    let font = egui::FontId::proportional(dp(&ctx, TEXT_DP));
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), text);
    let pad = dp(&ctx, 10.0);
    let w = size.x.max(galley.size().x + 2.0 * pad);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, size.y), Sense::click());
    let bg = match (fill, resp.is_pointer_button_down_on(), resp.hovered()) {
        (Some(f), true, _) => f.gamma_multiply(0.7),
        (Some(f), _, _) => f,
        (None, true, _) => hover.gamma_multiply(1.6),
        (None, _, true) => hover,
        _ => Color32::TRANSPARENT,
    };
    ui.painter().rect_filled(rect, dp(&ctx, 8.0), bg);
    ui.painter().galley(rect.center() - galley.size() * 0.5, galley, text);
    resp
}

/// Settings ▸ Appearance ▸ UI Scale one step up or down from the scale in use now.
fn step_interface_size(app: &mut FilmcraftApp, ctx: &egui::Context, larger: bool) -> Result<String, String> {
    let now = (ctx.pixels_per_point() * 100.0).round() as i32;
    let steps: Vec<i32> = filmcraft_engine::settings::UI_SCALES.iter().filter_map(|(k, _)| k.parse().ok()).collect();
    let next = if larger { steps.iter().copied().find(|v| *v > now + 2) } else { steps.iter().rev().copied().find(|v| *v < now - 2) };
    let Some(next) = next else {
        return Err(t(if larger { "The interface is as large as it gets" } else { "The interface is as small as it gets" }).to_string());
    };
    app.session.execute("prefs.set", json!({"key": "appearance.uiScale", "value": next.to_string()})).map_err(|e| e.to_string())?;
    Ok(format!("{} {next}%", t("Interface size")))
}
