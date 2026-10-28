//! The phone layout: what the Android app draws instead of the desktop header and docking area.
//!
//! The desktop interface is a dozen panels side by side, built for a mouse. On a phone the screen
//! shows what an edit needs, big enough to touch:
//!
//! ```text
//!  portrait                        landscape
//!  ┌──────────────────────┐        ┌───────────────┬─────────────────────┐
//!  │ ☰  Project   ↶ ↷ Export │      │ ☰ Project ↶ ↷        Export         │
//!  ├──────────────────────┤        ├───────────────┬─────────────────────┤
//!  │       Program        │        │    Program    │                     │
//!  ├──────────────────────┤        ├───────────────┤      Timeline       │
//!  │ 00:00:03:12  ‹ ▶ ›   │        │ 00:00:03:12 ‹ ▶ › │                 │
//!  ├──────────────────────┤        │               │                     │
//!  │       Timeline       │        ├───────────────┴─────────────────────┤
//!  ├──────────────────────┤        │ Import Media Add Split Delete Speed… │
//!  │ Import Media Add …   │        └─────────────────────────────────────┘
//!  └──────────────────────┘
//! ```
//!
//! A tool that needs a panel (Media, Properties, Audio, Effects, Text, Color, the menus) or its own
//! controls (Speed) opens it as a *sheet* in place of the Timeline, under the Program monitor,
//! which stays in view; tapping the tool again, or ✕, brings the Timeline back.
//!
//! Sizes are in dp (the system's density-independent pixels) divided by the interface zoom, so the
//! buttons stay finger-sized however the user scales the interface. Every control registers an
//! automation id (`phone.*`) and runs commands by id, like the menus; Add, Split and Delete are
//! [`crate::edit_actions`].

use egui::{Align2, Color32, Id, Rect, RichText, Sense, Stroke, UiBuilder, pos2, vec2};
use filmcraft_ui_egui::dock::PanelKind;
use filmcraft_ui_egui::i18n::t;
use filmcraft_ui_egui::icons::{self, Icon};
use filmcraft_ui_egui::state::Mode;
use filmcraft_ui_egui::{FilmcraftApp, menus};
use serde_json::json;

use crate::edit_actions::{self, Place};
use crate::speed_sheet;

const TOP_DP: f32 = 52.0;
const TRANSPORT_DP: f32 = 56.0;
const STRIP_DP: f32 = 68.0;
/// Room under the strip for the system's gesture bar.
const GESTURE_DP: f32 = 10.0;
const SHEET_HEAD_DP: f32 = 48.0;
const TOOL_DP: f32 = 72.0;
const TEXT_DP: f32 = 14.0;
const SMALL_DP: f32 = 11.0;
/// How long a message stays above the strip, in seconds.
const TOAST_SECONDS: f64 = 4.0;

/// A sheet: a panel, or the speed controls, in place of the Timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    Media,
    Speed,
    Properties,
    Audio,
    Effects,
    Text,
    Color,
    Menu,
}

impl Sheet {
    fn title(self) -> &'static str {
        match self {
            Sheet::Media => t("Media"),
            Sheet::Speed => t("Speed"),
            Sheet::Properties => t("Properties"),
            Sheet::Audio => t("Audio"),
            Sheet::Effects => t("Effects"),
            Sheet::Text => t("Text"),
            Sheet::Color => t("Color"),
            Sheet::Menu => t("Menu"),
        }
    }
    fn panel(self) -> Option<PanelKind> {
        match self {
            Sheet::Media => Some(PanelKind::Project),
            Sheet::Properties => Some(PanelKind::EffectControls),
            Sheet::Audio => Some(PanelKind::AudioClipMixer),
            Sheet::Effects => Some(PanelKind::Effects),
            Sheet::Text => Some(PanelKind::Text),
            Sheet::Color => Some(PanelKind::LumetriColor),
            Sheet::Speed | Sheet::Menu => None,
        }
    }
    /// The sheet that shows `panel`, for a command that asks for it (`Window ▸ Effects`).
    fn of_panel(panel: PanelKind) -> Option<Sheet> {
        use PanelKind::*;
        match panel {
            Project | MediaBrowser | Libraries | Info | Markers | History => Some(Sheet::Media),
            Effects => Some(Sheet::Effects),
            EffectControls | Properties | Source | Metadata | EssentialGraphics => Some(Sheet::Properties),
            AudioClipMixer | AudioTrackMixer | EssentialSound => Some(Sheet::Audio),
            Text | TextToSpeech => Some(Sheet::Text),
            LumetriColor | LumetriScopes => Some(Sheet::Color),
            _ => None,
        }
    }
}

/// What the layout remembers between frames.
#[derive(Clone, Default)]
struct State {
    sheet: Option<Sheet>,
    /// The last message and the time (egui seconds) it is shown until; `true`: an error.
    toast: Option<(String, f64, bool)>,
    /// The soft keyboard is up: the strip steps aside so the field being typed in is seen.
    keyboard: bool,
}

fn state_id() -> Id {
    Id::unique("filmcraft-phone-state")
}

fn load(ctx: &egui::Context) -> State {
    ctx.data(|d| d.get_temp::<State>(state_id())).unwrap_or_default()
}

fn store(ctx: &egui::Context, s: State) {
    ctx.data_mut(|d| d.insert_temp(state_id(), s));
}

/// Use the phone layout for `app`.
pub fn install(app: &mut FilmcraftApp) {
    app.touch_ui = true;
    app.phone_layout = Some(layout);
}

/// Say something above the strip for a few seconds (`error`: in red).
pub fn notify(ctx: &egui::Context, text: impl Into<String>, error: bool) {
    let mut s = load(ctx);
    s.toast = Some((text.into(), ctx.input(|i| i.time) + TOAST_SECONDS, error));
    store(ctx, s);
    ctx.request_repaint();
}

/// Say something in whichever interface is in use: a toast on the phone layout, the status bar
/// on the desktop one.
pub fn say(app: &mut FilmcraftApp, ctx: &egui::Context, text: impl Into<String>, error: bool) {
    let text = text.into();
    app.ui.status = text.clone();
    notify(ctx, text, error);
}

/// Tell the layout whether the soft keyboard is up.
pub fn set_keyboard(ctx: &egui::Context, up: bool) {
    let mut s = load(ctx);
    if s.keyboard != up {
        s.keyboard = up;
        store(ctx, s);
    }
}

/// The sheet in use, if any.
pub fn open_sheet(ctx: &egui::Context) -> Option<Sheet> {
    load(ctx).sheet
}

/// `dp` density-independent pixels, in points at the current interface zoom.
pub fn dp(ctx: &egui::Context, v: f32) -> f32 {
    v / ctx.zoom_factor().clamp(0.1, 10.0)
}

/// How the screen is divided.
struct Areas {
    top: Rect,
    monitor: Rect,
    transport: Rect,
    /// The Timeline, or the sheet that replaces it.
    work: Rect,
    strip: Option<Rect>,
}

fn areas(ctx: &egui::Context, full: Rect, keyboard: bool) -> Areas {
    let top = Rect::from_min_size(full.min, vec2(full.width(), dp(ctx, TOP_DP)));
    let strip_h = if keyboard { 0.0 } else { dp(ctx, STRIP_DP + GESTURE_DP) };
    let strip = (!keyboard).then(|| Rect::from_min_max(pos2(full.min.x, full.max.y - strip_h), full.max));
    let body = Rect::from_min_max(pos2(full.min.x, top.max.y), pos2(full.max.x, full.max.y - strip_h));
    let transport_h = dp(ctx, TRANSPORT_DP);
    if body.width() < body.height() * 1.1 {
        // portrait: the picture at 16:9 across the top, at most 40 % of the body
        let monitor_h = (body.width() * 9.0 / 16.0).min(body.height() * 0.4).max(0.0);
        let monitor = Rect::from_min_size(body.min, vec2(body.width(), monitor_h));
        let transport = Rect::from_min_size(monitor.left_bottom(), vec2(body.width(), transport_h));
        let work = Rect::from_min_max(transport.left_bottom(), body.max);
        Areas { top, monitor, transport, work, strip }
    } else {
        // landscape: the picture and its transport on the left, the Timeline beside them
        let left_w = ((body.height() - transport_h).max(0.0) * 16.0 / 9.0).clamp(body.width() * 0.3, body.width() * 0.5);
        let monitor = Rect::from_min_size(body.min, vec2(left_w, (body.height() - transport_h).max(0.0)));
        let transport = Rect::from_min_size(monitor.left_bottom(), vec2(left_w, transport_h));
        let work = Rect::from_min_max(pos2(body.min.x + left_w, body.min.y), body.max);
        Areas { top, monitor, transport, work, strip }
    }
}

/// The whole phone screen: [`FilmcraftApp::phone_layout`].
pub fn layout(app: &mut FilmcraftApp, ui: &mut egui::Ui, full: Rect) {
    let ctx = ui.ctx().clone();
    app.touch_ui = true;
    let mut st = load(&ctx);
    // a command asked for a panel (double-click on a clip, Window ▸ …)
    if let Some(sheet) = app.phone_request.take().and_then(Sheet::of_panel) {
        st.sheet = Some(sheet);
    }
    // the Import page is the Android file browser, reached by the Import button
    if app.ui.mode == Mode::Import {
        app.ui.mode = Mode::Edit;
    }
    let a = areas(&ctx, full, st.keyboard);
    let mut act = None;
    let tk = app.tokens;
    ui.painter().rect_filled(full, 0.0, tk.app_bg);
    if app.ui.mode == Mode::Export {
        act = top_bar(app, ui, a.top, true);
        let body = Rect::from_min_max(a.top.left_bottom(), full.max);
        app.export_page(ui, body);
    } else {
        act = act.or(top_bar(app, ui, a.top, false));
        // the picture, then its transport
        app.phone_monitor = true;
        app.panel_in(ui, PanelKind::Program, a.monitor);
        app.phone_monitor = false;
        act = act.or(transport(app, ui, a.transport));
        match st.sheet {
            None => {
                app.panel_in(ui, PanelKind::Timeline, a.work);
                act = act.or(zoom_cluster(app, ui, a.work));
            }
            Some(sheet) => act = act.or(sheet_view(app, ui, a.work, sheet)),
        }
        if let Some(strip) = a.strip {
            act = act.or(tool_strip(app, ui, strip, st.sheet));
        }
    }
    if let Some(act) = act {
        run(app, &ctx, &mut st, act);
    }
    show_toast(app, &ctx, &mut st, a.strip.map_or(0.0, |s| s.height()));
    store(&ctx, st);
}

/// One tap on the phone layout.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
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
    Export,
    Edit,
    Sheet(Sheet),
    CloseSheet,
}

fn run(app: &mut FilmcraftApp, ctx: &egui::Context, st: &mut State, act: Act) {
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
        Act::Export => menus::invoke(app, ctx, "mode.export", json!({})).map(|_| None),
        Act::Edit => {
            app.ui.mode = Mode::Edit;
            Ok(None)
        }
        Act::Sheet(s) => {
            st.sheet = if st.sheet == Some(s) { None } else { Some(s) };
            Ok(None)
        }
        Act::CloseSheet => {
            st.sheet = None;
            Ok(None)
        }
    };
    let now = ctx.input(|i| i.time);
    match result {
        Ok(Some(msg)) => st.toast = Some((msg, now + TOAST_SECONDS, false)),
        Ok(None) => {}
        Err(e) => {
            app.ui.status = e.clone();
            st.toast = Some((e, now + TOAST_SECONDS, true));
        }
    }
    ctx.request_repaint();
}

// ------------------------------------------------------------------------------------------ top

fn top_bar(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, exporting: bool) -> Option<Act> {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    ui.painter().rect_filled(rect, 0.0, tk.header_bg);
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, tk.separator));
    let side = dp(&ctx, 8.0);
    let b = dp(&ctx, 44.0);
    let mut x = rect.min.x + side;
    let mut chosen = None;
    let slot = |x: &mut f32, w: f32| {
        let r = Rect::from_min_size(pos2(*x, rect.center().y - b / 2.0), vec2(w, b));
        *x += w;
        r
    };
    if exporting {
        let r = slot(&mut x, dp(&ctx, 96.0));
        if pill(ui, app, "phone.back", r, t("‹ Edit"), false) {
            chosen = Some(Act::Edit);
        }
    } else {
        let r = slot(&mut x, b);
        if icon_button(ui, app, "phone.menu", r, Icon::Hamburger, t("Menu"), load(&ctx).sheet == Some(Sheet::Menu)) {
            chosen = Some(Act::Sheet(Sheet::Menu));
        }
    }
    // the title, between the menu and the buttons on the right
    let mut right = rect.max.x - side;
    let export_w = dp(&ctx, 96.0);
    if !exporting {
        let r = Rect::from_min_size(pos2(right - export_w, rect.center().y - b / 2.0), vec2(export_w, b));
        right = r.min.x - dp(&ctx, 4.0);
        if pill(ui, app, "phone.export", r, t("Export"), true) {
            chosen = Some(Act::Export);
        }
        for (id, icon, label, act) in [("phone.redo", Icon::Redo, t("Redo"), Act::Redo), ("phone.undo", Icon::Undo, t("Undo"), Act::Undo)] {
            let r = Rect::from_min_size(pos2(right - b, rect.center().y - b / 2.0), vec2(b, b));
            right = r.min.x;
            if icon_button(ui, app, id, r, icon, label, false) {
                chosen = Some(act);
            }
        }
    }
    let title = if app.session.is_dirty() { format!("{} •", app.session.project.name) } else { app.session.project.name.clone() };
    let title_rect = Rect::from_min_max(pos2(x + dp(&ctx, 4.0), rect.min.y), pos2(right - dp(&ctx, 4.0), rect.max.y));
    if title_rect.width() > 10.0 {
        let mut job = egui::text::LayoutJob::simple_singleline(title, egui::FontId::proportional(dp(&ctx, 16.0)), tk.tab_text_active);
        job.wrap.max_width = title_rect.width();
        job.wrap.max_rows = 1;
        let galley = ui.painter().layout_job(job);
        ui.painter().with_clip_rect(title_rect).galley(pos2(title_rect.min.x, rect.center().y - galley.size().y / 2.0), galley, tk.tab_text_active);
    }
    chosen
}

// ------------------------------------------------------------------------------------ transport

fn transport(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) -> Option<Act> {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    ui.painter().rect_filled(rect, 0.0, tk.panel_bg);
    let mut chosen = None;
    let (rate, drop) =
        app.session.active_sequence().map(|q| (q.settings.frame_rate, q.settings.drop_frame)).unwrap_or((filmcraft_time::FrameRate::FPS_24, false));
    let now = filmcraft_time::format_time(app.session.playhead(), rate, drop, filmcraft_time::TimeDisplay::Timecode, 48_000);
    let total = app.session.active_sequence().map(|q| q.duration()).unwrap_or_default();
    let total = filmcraft_time::format_time(total, rate, drop, filmcraft_time::TimeDisplay::Timecode, 48_000);
    let pad = dp(&ctx, 12.0);
    let btn = dp(&ctx, 48.0);
    // the playhead time, with the length under it
    let tc = Rect::from_min_size(pos2(rect.min.x + pad, rect.min.y), vec2(dp(&ctx, 112.0), rect.height()));
    ui.painter().text(pos2(tc.min.x, tc.center().y - dp(&ctx, 8.0)), Align2::LEFT_CENTER, now, egui::FontId::monospace(dp(&ctx, 16.0)), tk.timecode);
    ui.painter().text(
        pos2(tc.min.x, tc.center().y + dp(&ctx, 11.0)),
        Align2::LEFT_CENTER,
        format!("/ {total}"),
        egui::FontId::monospace(dp(&ctx, 11.0)),
        tk.text_dim,
    );
    app.auto.add("phone.timecode", tc, "Timecode");
    // ‹ play ›, centred in what is left
    let rest = Rect::from_min_max(pos2(tc.max.x, rect.min.y), pos2(rect.max.x - pad, rect.max.y));
    let play_w = dp(&ctx, 60.0);
    let group_w = btn + play_w + btn + 2.0 * dp(&ctx, 6.0);
    let mut x = (rest.center().x - group_w / 2.0).max(rest.min.x);
    let y = rect.center().y;
    let r = Rect::from_center_size(pos2(x + btn / 2.0, y), vec2(btn, btn));
    if icon_button(ui, app, "phone.stepback", r, Icon::StepBack, t("Back one frame"), false) {
        chosen = Some(Act::StepBack);
    }
    x = r.max.x + dp(&ctx, 6.0);
    let r = Rect::from_center_size(pos2(x + play_w / 2.0, y), vec2(play_w, btn));
    let playing = app.playback.playing;
    let resp = ui.interact(r, Id::unique("phone.play"), Sense::click());
    let fill = if resp.is_pointer_button_down_on() { tk.accent_hover } else { tk.accent };
    ui.painter().rect_filled(r, r.height() / 2.0, fill);
    icons::paint(
        ui.painter(),
        Rect::from_center_size(r.center(), vec2(dp(&ctx, 24.0), dp(&ctx, 24.0))),
        if playing { Icon::Pause } else { Icon::Play },
        Color32::WHITE,
    );
    app.auto.add("phone.play", r, if playing { t("Pause") } else { t("Play") });
    if resp.clicked() {
        chosen = Some(Act::Play);
    }
    x = r.max.x + dp(&ctx, 6.0);
    let r = Rect::from_center_size(pos2(x + btn / 2.0, y), vec2(btn, btn));
    if icon_button(ui, app, "phone.stepforward", r, Icon::StepFwd, t("Forward one frame"), false) {
        chosen = Some(Act::StepForward);
    }
    chosen
}

/// Timeline zoom: a small vertical pill in the Timeline's bottom-right corner.
fn zoom_cluster(app: &mut FilmcraftApp, ui: &mut egui::Ui, work: Rect) -> Option<Act> {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    let b = dp(&ctx, 44.0);
    let gap = dp(&ctx, 8.0);
    let pill = Rect::from_min_size(pos2(work.max.x - b - gap, work.max.y - 3.0 * b - 2.0 * gap), vec2(b, 3.0 * b));
    if pill.min.y < work.min.y + dp(&ctx, 60.0) {
        return None;
    }
    ui.painter().rect_filled(pill, b / 2.0, tk.header_bg.gamma_multiply(0.92));
    ui.painter().rect_stroke(pill, b / 2.0, Stroke::new(1.0, tk.separator), egui::StrokeKind::Inside);
    let mut chosen = None;
    for (i, (id, label, act)) in
        [("phone.zoomin", "+", Act::ZoomIn), ("phone.zoomout", "−", Act::ZoomOut), ("phone.fit", "⤢", Act::Fit)].into_iter().enumerate()
    {
        let r = Rect::from_min_size(pos2(pill.min.x, pill.min.y + i as f32 * b), vec2(b, b));
        let resp = ui.interact(r, Id::unique(id), Sense::click());
        if resp.is_pointer_button_down_on() {
            ui.painter().rect_filled(r.shrink(2.0), b / 2.0, tk.hover);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, egui::FontId::proportional(dp(&ctx, 20.0)), tk.text);
        app.auto.add(id, r, label);
        if resp.clicked() {
            chosen = Some(act);
        }
    }
    chosen
}

// ------------------------------------------------------------------------------------- the strip

fn tool_strip(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, open: Option<Sheet>) -> Option<Act> {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    ui.painter().rect_filled(rect, 0.0, tk.header_bg);
    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, tk.separator));
    let strip = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - dp(&ctx, GESTURE_DP)));
    let tools: [(&str, Icon, &str, Act, bool); 11] = [
        ("import", Icon::Import, t("Import"), Act::Import, false),
        ("media", Icon::Folder, t("Media"), Act::Sheet(Sheet::Media), open == Some(Sheet::Media)),
        ("add", Icon::Plus, t("Add"), Act::Add(Place::End), false),
        ("split", Icon::Razor, t("Split"), Act::Split, false),
        ("delete", Icon::Trash, t("Delete"), Act::Delete, false),
        ("speed", Icon::Stopwatch, t("Speed"), Act::Sheet(Sheet::Speed), open == Some(Sheet::Speed)),
        ("properties", Icon::Wrench, t("Properties"), Act::Sheet(Sheet::Properties), open == Some(Sheet::Properties)),
        ("audio", Icon::Speaker, t("Audio"), Act::Sheet(Sheet::Audio), open == Some(Sheet::Audio)),
        ("effects", Icon::Fx, t("Effects"), Act::Sheet(Sheet::Effects), open == Some(Sheet::Effects)),
        ("text", Icon::Type, t("Text"), Act::Sheet(Sheet::Text), open == Some(Sheet::Text)),
        ("color", Icon::Adjust, t("Color"), Act::Sheet(Sheet::Color), open == Some(Sheet::Color)),
    ];
    let mut chosen = None;
    let mut inner = ui.new_child(UiBuilder::new().max_rect(strip).layout(egui::Layout::left_to_right(egui::Align::Center)));
    egui::ScrollArea::horizontal().id_salt("phone-strip").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(&mut inner, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (id, icon, label, act, active) in tools {
            let (r, _) = ui.allocate_exact_size(vec2(dp(&ctx, TOOL_DP), strip.height()), Sense::hover());
            if icon_button_labeled(ui, app, &format!("touchbar.{id}"), r.shrink2(vec2(dp(&ctx, 2.0), dp(&ctx, 4.0))), icon, label, active) {
                chosen = Some(act);
            }
        }
    });
    chosen
}

// ---------------------------------------------------------------------------------------- sheets

fn sheet_view(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, sheet: Sheet) -> Option<Act> {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    ui.painter().rect_filled(rect, 0.0, tk.panel_bg);
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), dp(&ctx, SHEET_HEAD_DP)));
    ui.painter().rect_filled(head, 0.0, tk.header_bg);
    ui.painter().line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, tk.separator));
    ui.painter().text(
        pos2(head.min.x + dp(&ctx, 16.0), head.center().y),
        Align2::LEFT_CENTER,
        sheet.title(),
        egui::FontId::proportional(dp(&ctx, 16.0)),
        tk.tab_text_active,
    );
    let mut chosen = None;
    let b = dp(&ctx, 44.0);
    let close = Rect::from_center_size(pos2(head.max.x - b / 2.0 - dp(&ctx, 4.0), head.center().y), vec2(b, b));
    if icon_button(ui, app, "phone.sheet.close", close, Icon::Close, t("Close"), false) {
        chosen = Some(Act::CloseSheet);
    }
    let body = Rect::from_min_max(head.left_bottom(), rect.max);
    match sheet {
        Sheet::Speed => speed_sheet::show(app, ui, body),
        Sheet::Menu => menu_sheet(app, ui, body),
        Sheet::Media => {
            // the Project panel, and the ways to put what is chosen on the timeline
            let foot_h = dp(&ctx, 60.0);
            let foot = Rect::from_min_max(pos2(body.min.x, body.max.y - foot_h), body.max);
            app.panel_in(ui, PanelKind::Project, Rect::from_min_max(body.min, pos2(body.max.x, foot.min.y)));
            ui.painter().rect_filled(foot, 0.0, tk.header_bg);
            ui.painter().line_segment([foot.left_top(), foot.right_top()], Stroke::new(1.0, tk.separator));
            let w = (foot.width() - 5.0 * dp(&ctx, 8.0)) / 4.0;
            for (i, (id, label, act)) in [
                ("import", t("Import"), Act::Import),
                ("add", t("Add"), Act::Add(Place::End)),
                ("insert", t("Insert"), Act::Add(Place::Insert)),
                ("overwrite", t("Overwrite"), Act::Add(Place::Overwrite)),
            ]
            .into_iter()
            .enumerate()
            {
                let r = Rect::from_min_size(
                    pos2(foot.min.x + dp(&ctx, 8.0) + i as f32 * (w + dp(&ctx, 8.0)), foot.min.y + dp(&ctx, 8.0)),
                    vec2(w, foot_h - dp(&ctx, 16.0)),
                );
                if pill(ui, app, &format!("phone.media.{id}"), r, label, id == "add") {
                    chosen = Some(act);
                }
            }
        }
        other => {
            if let Some(panel) = other.panel() {
                app.panel_in(ui, panel, body);
            }
        }
    }
    chosen
}

/// The menus of the desktop menu bar as a list, for what the strip does not offer.
fn menu_sheet(app: &mut FilmcraftApp, ui: &mut egui::Ui, body: Rect) {
    let ctx = ui.ctx().clone();
    let mut inner = ui.new_child(UiBuilder::new().max_rect(body.shrink(dp(&ctx, 8.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt("phone-menu").show(&mut inner, |ui| {
        ui.spacing_mut().button_padding = vec2(dp(&ctx, 12.0), dp(&ctx, 12.0));
        ui.spacing_mut().item_spacing.y = dp(&ctx, 6.0);
        ui.set_min_width(body.width() - dp(&ctx, 16.0));
        ui.vertical(|ui| menus::menu_bar(app, ui));
    });
}

// ------------------------------------------------------------------------------------- widgets

/// A square icon button; `true` when tapped.
pub fn icon_button(ui: &mut egui::Ui, app: &mut FilmcraftApp, id: &str, rect: Rect, icon: Icon, label: &str, active: bool) -> bool {
    let tk = app.tokens;
    let ctx = ui.ctx().clone();
    let resp = ui.interact(rect, Id::unique(id), Sense::click());
    if active || resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, dp(&ctx, 10.0), if active { tk.pill_active_bg } else { tk.hover });
    }
    let side = dp(&ctx, 24.0);
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(side, side)), icon, if active { tk.icon_active } else { tk.icon });
    app.auto.add(id, rect, label);
    resp.clicked()
}

/// An icon with its label underneath (the tool strip); `true` when tapped.
fn icon_button_labeled(ui: &mut egui::Ui, app: &mut FilmcraftApp, id: &str, rect: Rect, icon: Icon, label: &str, active: bool) -> bool {
    let tk = app.tokens;
    let ctx = ui.ctx().clone();
    let resp = ui.interact(rect, Id::unique(id), Sense::click());
    if active || resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, dp(&ctx, 12.0), if active { tk.pill_active_bg } else { tk.hover });
    }
    let side = dp(&ctx, 26.0);
    let icon_rect = Rect::from_center_size(pos2(rect.center().x, rect.min.y + dp(&ctx, 8.0) + side / 2.0), vec2(side, side));
    let color = if active { tk.icon_active } else { tk.icon };
    icons::paint(ui.painter(), icon_rect, icon, color);
    ui.painter().text(
        pos2(rect.center().x, icon_rect.max.y + dp(&ctx, 8.0)),
        Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(dp(&ctx, SMALL_DP)),
        if active { tk.tab_text_active } else { tk.text_dim },
    );
    app.auto.add(id, rect, label);
    resp.clicked()
}

/// A rounded text button (`accent`: filled with the accent colour); `true` when tapped.
pub fn pill(ui: &mut egui::Ui, app: &mut FilmcraftApp, id: &str, rect: Rect, label: &str, accent: bool) -> bool {
    let tk = app.tokens;
    let ctx = ui.ctx().clone();
    let resp = ui.interact(rect, Id::unique(id), Sense::click());
    let (fill, text) = match (accent, resp.is_pointer_button_down_on()) {
        (true, true) => (tk.accent_hover, Color32::WHITE),
        (true, false) => (tk.accent, Color32::WHITE),
        (false, true) => (tk.pressed, tk.text),
        (false, false) => (tk.field_bg, tk.text),
    };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, egui::FontId::proportional(dp(&ctx, TEXT_DP)), text);
    app.auto.add(id, rect, label);
    resp.clicked()
}

/// The result of the last tap, above the strip for a few seconds.
fn show_toast(app: &FilmcraftApp, ctx: &egui::Context, st: &mut State, lift: f32) {
    let now = ctx.input(|i| i.time);
    let Some((text, until, error)) = st.toast.clone() else { return };
    if now >= until {
        st.toast = None;
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    let tk = app.tokens;
    let lift = lift + dp(ctx, 12.0);
    egui::Area::new(Id::unique("phone-toast")).order(egui::Order::Tooltip).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -lift)).interactable(false).show(
        ctx,
        |ui| {
            let fill = if error { Color32::from_rgb(120, 40, 40) } else { tk.accent };
            egui::Frame::new().fill(fill).corner_radius(dp(ctx, 8.0)).inner_margin(vec2(dp(ctx, 14.0), dp(ctx, 8.0))).show(ui, |ui| {
                ui.label(RichText::new(text).size(dp(ctx, TEXT_DP)).color(Color32::WHITE));
            });
        },
    );
}
