//! The touch interface on a phone-sized screen: the shared interface above the touch bar, driven
//! by taps on the bar. Renders to PNG when a (software) GPU exists and `FILMCRAFT_SCREENSHOT_DIR`
//! names a folder; without a GPU the checks that need pixels are skipped.

use egui_kittest::Harness;
use filmcraft::chrome::Chrome;
use filmcraft_engine::Session;
use filmcraft_project::ItemId;
use filmcraft_ui_egui::FilmcraftApp;
use serde_json::json;

/// A 2400 x 1080 px phone screen at 2.75x (873 x 393 dp), with the interface zoomed out to fit.
const PHONE_PX: (f32, f32) = (2400.0, 1080.0);
const DENSITY: f32 = 2.75;

struct Phone {
    app: FilmcraftApp,
    chrome: Chrome,
    /// Touch -> mouse translation, as the Android shell applies it to the window's events.
    gestures: filmcraft::touch::Gestures,
    /// Pass pointer events through (a mouse), instead of translating touches.
    mouse: bool,
}

impl eframe::App for Phone {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.mouse {
            self.gestures.process(&mut raw.events, std::time::Instant::now());
        }
        self.app.raw_input_hook(ctx, raw);
    }
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.chrome.ui(&mut self.app, ui, frame);
    }
}

fn phone(project: impl FnOnce(&mut Session)) -> Harness<'static, Phone> {
    let mut s = Session::default();
    project(&mut s);
    // Settings > Appearance > UI Scale 200%: 2 pixels per point, so the 2400 x 1080 px screen is
    // 1200 x 540 points (what the phone's own automatic zoom comes to, 2.75 x 0.73)
    s.execute("prefs.set", json!({"key": "appearance.uiScale", "value": "200"})).unwrap();
    let size = egui::vec2(PHONE_PX.0, PHONE_PX.1) / 2.0;
    let mut h = Harness::builder().with_size(size).with_pixels_per_point(DENSITY).with_max_steps(10_000).wgpu().build_eframe(move |_cc| Phone {
        app: FilmcraftApp::new(s),
        chrome: Chrome::default(),
        gestures: Default::default(),
        mouse: false,
    });
    step(&mut h, 6);
    h
}

fn step(h: &mut Harness<'static, Phone>, n: usize) {
    for _ in 0..n {
        let ctx = h.ctx.clone();
        let mut raw = std::mem::take(h.input_mut());
        eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
        *h.input_mut() = raw;
        ctx.request_repaint();
        h.step();
    }
}

fn control_rect(h: &Harness<'static, Phone>, id: &str) -> egui::Rect {
    let [x, y, w, height] =
        h.state().app.auto.elements.iter().find(|e| e.id == id).or_else(|| h.state().app.auto.find(id)).unwrap_or_else(|| panic!("no control {id}")).rect;
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, height))
}

/// A tap with a finger on the named control: touch down, up.
fn tap(h: &mut Harness<'static, Phone>, id: &str) {
    let at = control_rect(h, id).center();
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(7), phase, pos: at, force: None };
    h.input_mut().events.push(touch(egui::TouchPhase::Start));
    step(h, 1);
    h.input_mut().events.push(touch(egui::TouchPhase::End));
    step(h, 4);
}

fn shot(h: &mut Harness<'static, Phone>, name: &str) {
    let Ok(dir) = std::env::var("FILMCRAFT_SCREENSHOT_DIR") else { return };
    step(h, 2);
    match h.render() {
        Ok(img) => {
            std::fs::create_dir_all(&dir).unwrap();
            let path = std::path::Path::new(&dir).join(format!("{name}.png"));
            img.save(&path).unwrap();
            eprintln!("snapshot: {} ({}x{})", path.display(), img.width(), img.height());
        }
        Err(e) => eprintln!("snapshot {name} skipped: {e}"),
    }
}

fn with_clips(s: &mut Session) -> Vec<ItemId> {
    s.execute("file.newProject", json!({"name": "Phone"})).unwrap();
    let mut ids = Vec::new();
    for scene in ["OceanSunset", "Aurora", "CityNight"] {
        let r = s.execute("file.importDemoFootage", json!({"scene": scene})).unwrap();
        ids.extend(r["items"].as_array().cloned().unwrap_or_default().into_iter().filter_map(|v| v.as_u64().map(ItemId)));
    }
    ids
}

fn video_clips(h: &Harness<'static, Phone>) -> usize {
    h.state().app.session.active_sequence().map_or(0, |q| q.video_tracks.iter().map(|t| t.items.len()).sum())
}

#[test]
fn tapping_add_puts_the_chosen_clips_on_the_timeline() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    shot(&mut h, "01-imported");
    assert_eq!(video_clips(&h), 0);
    // tapping Add with nothing chosen says what to do
    tap(&mut h, "touchbar.add");
    assert!(h.state().app.ui.status.contains("Project panel"), "status: {}", h.state().app.ui.status);
    // choose a clip, as a tap in the Project panel does, then Add: a timeline is made from it
    h.state_mut().app.session.state.project_selection = vec![ids[0]];
    tap(&mut h, "touchbar.add");
    assert_eq!(video_clips(&h), 1);
    shot(&mut h, "02-first-clip-added");
    // more clips go back to back after it
    h.state_mut().app.session.state.project_selection = vec![ids[1], ids[2]];
    tap(&mut h, "touchbar.add");
    assert_eq!(video_clips(&h), 3);
    shot(&mut h, "03-three-clips");
    // Undo takes the last Add back as one step
    tap(&mut h, "touchbar.undo");
    assert_eq!(video_clips(&h), 1);
    tap(&mut h, "touchbar.redo");
    assert_eq!(video_clips(&h), 3);
}

#[test]
fn the_buttons_are_finger_sized_and_fit_the_screen() {
    let mut h = phone(|s| {
        with_clips(s);
    });
    let screen = h.ctx.content_rect();
    let zoom = h.ctx.zoom_factor();
    for id in [
        "touchbar.import",
        "touchbar.add",
        "touchbar.play",
        "touchbar.stepback",
        "touchbar.stepforward",
        "touchbar.split",
        "touchbar.delete",
        "touchbar.undo",
        "touchbar.redo",
    ] {
        let r = control_rect(&h, id);
        // at least 44 dp each way (48 dp is the aim): points x zoom = dp
        assert!(r.height() * zoom >= 44.0, "{id} is {} dp tall", r.height() * zoom);
        assert!(r.width() * zoom >= 44.0, "{id} is {} dp wide", r.width() * zoom);
        assert!(r.min.y >= screen.min.y && r.max.y <= screen.max.y + 0.5, "{id} {r:?} is off the screen {screen:?}");
    }
    tap(&mut h, "touchbar.add.more");
    shot(&mut h, "04b-add-menu");
    tap(&mut h, "touchbar.add.more");
    tap(&mut h, "touchbar.menu");
    shot(&mut h, "04c-more-menu");
    shot(&mut h, "04-bar");
}

#[test]
fn the_bar_can_be_hidden_and_brought_back() {
    let mut h = phone(|s| {
        with_clips(s);
    });
    tap(&mut h, "touchbar.menu");
    tap(&mut h, "touchbar.menu.hide");
    assert!(h.state().app.auto.find("touchbar.add").is_none() || h.state().app.auto.find("touchbar.show").is_some());
    shot(&mut h, "05-hidden");
    tap(&mut h, "touchbar.show");
    assert!(control_rect(&h, "touchbar.add").height() > 0.0);
}

/// One finger on the screen: touch events as winit delivers them (egui-winit also makes the
/// mouse events the gesture translator replaces), moving in steps from `from` to `to`.
fn finger_drag(h: &mut Harness<'static, Phone>, from: egui::Pos2, to: egui::Pos2, steps: usize) {
    let touch = |phase, pos| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(1), phase, pos, force: None };
    h.input_mut().events.push(touch(egui::TouchPhase::Start, from));
    step(h, 2);
    for i in 1..=steps {
        let pos = from + (to - from) * (i as f32 / steps as f32);
        h.input_mut().events.push(touch(egui::TouchPhase::Move, pos));
        step(h, 1);
    }
    h.input_mut().events.push(touch(egui::TouchPhase::End, to));
    step(h, 4);
}

fn project_item_center(h: &Harness<'static, Phone>, item: ItemId) -> egui::Pos2 {
    control_rect(h, &format!("project.item.{}", item.0)).center()
}

/// A point in the empty Timeline's upper half, clear of its button.
fn empty_timeline_drop_point(h: &Harness<'static, Phone>) -> egui::Pos2 {
    let r = control_rect(h, "panel.Timeline");
    egui::pos2(r.center().x, r.min.y + r.height() * 0.3)
}

/// A point on the V1 row of the Timeline, past its track headers.
fn timeline_v1(h: &Harness<'static, Phone>) -> egui::Pos2 {
    let row = control_rect(h, "timeline.track.V1.enabled");
    let panel = control_rect(h, "panel.Timeline");
    egui::pos2(panel.min.x + h.state().app.ui.timeline.header_w + 80.0, row.center().y)
}

#[test]
fn a_finger_drag_from_the_project_panel_makes_and_fills_the_timeline() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    // onto the empty timeline: it makes a sequence from the clip
    let to = empty_timeline_drop_point(&h);
    let from = project_item_center(&h, ids[0]);
    finger_drag(&mut h, from, to, 12);
    assert_eq!(video_clips(&h), 1, "the dragged clip made the timeline");
    shot(&mut h, "06-after-drag-1");
    // onto the timeline that now exists: the clip lands on V1
    let from = project_item_center(&h, ids[1]);
    let to = timeline_v1(&h);
    finger_drag(&mut h, from, to, 12);
    assert_eq!(video_clips(&h), 2, "the second clip went onto V1");
    shot(&mut h, "07-after-drag-2");
}

#[test]
fn a_tap_selects_a_project_clip_and_a_long_press_opens_its_menu() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    let at = project_item_center(&h, ids[1]);
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(1), phase, pos: at, force: None };
    h.input_mut().events.push(touch(egui::TouchPhase::Start));
    step(&mut h, 2);
    h.input_mut().events.push(touch(egui::TouchPhase::End));
    step(&mut h, 4);
    assert_eq!(h.state().app.session.state.project_selection, vec![ids[1]], "a tap selects");
    // press and hold: egui turns a long touch into the context menu
    h.input_mut().events.push(touch(egui::TouchPhase::Start));
    step(&mut h, 2);
    // egui's own clock decides how long a hold is (the harness advances it a step at a time)
    let mut opened = false;
    for _ in 0..400 {
        step(&mut h, 1);
        if h.state().app.auto.find("project.itemMenu.label").is_some() {
            opened = true;
            break;
        }
    }
    shot(&mut h, "08-long-press");
    h.input_mut().events.push(touch(egui::TouchPhase::End));
    step(&mut h, 3);
    assert!(opened, "holding a clip opens its menu");
}

#[test]
fn the_keyboard_pushes_the_interface_up_and_the_bar_aside() {
    let mut h = phone(|s| {
        with_clips(s);
    });
    assert!(h.state().app.auto.find("touchbar.add").is_some());
    let search_before = control_rect(&h, "panel.Project");
    h.state_mut().chrome.keyboard = 200.0;
    step(&mut h, 3);
    assert!(h.state().app.auto.find("touchbar.add").is_none(), "the bar steps aside while the keyboard is up");
    let search_after = control_rect(&h, "panel.Project");
    let screen = h.ctx.content_rect();
    assert!(search_before.max.y > screen.max.y - 200.0, "without the keyboard the panel reaches into its place: {search_before:?} of {screen:?}");
    assert!(search_after.max.y <= screen.max.y - 200.0, "the Project panel is above the keyboard: {search_after:?} of {screen:?}");
    shot(&mut h, "10-keyboard");
    h.state_mut().chrome.keyboard = 0.0;
    step(&mut h, 3);
    assert!(h.state().app.auto.find("touchbar.add").is_some());
}

#[test]
fn the_export_and_import_screens_fit_a_phone() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    h.state_mut().app.session.state.project_selection = vec![ids[0]];
    tap(&mut h, "touchbar.add");
    tap(&mut h, "touchbar.menu");
    tap(&mut h, "touchbar.menu.export");
    shot(&mut h, "11-export-screen");
    assert_eq!(h.state().app.ui.mode, filmcraft_ui_egui::state::Mode::Export);
    h.state_mut().app.ui.mode = filmcraft_ui_egui::state::Mode::Import;
    step(&mut h, 3);
    shot(&mut h, "12-import-screen");
}
