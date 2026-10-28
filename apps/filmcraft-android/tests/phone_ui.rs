//! The phone layout on a phone-sized screen, driven by taps and finger drags. Renders to PNG when
//! a (software) GPU exists and `FILMCRAFT_SCREENSHOT_DIR` names a folder; without a GPU the checks
//! that need pixels are skipped.

use egui_kittest::Harness;
use filmcraft::chrome::Chrome;
use filmcraft_engine::Session;
use filmcraft_project::ItemId;
use filmcraft_ui_egui::FilmcraftApp;
use serde_json::json;

/// A 1080 x 2400 px phone at 2.75x (393 x 873 dp), upright; [`LANDSCAPE`] turned on its side.
const PORTRAIT: (f32, f32) = (1080.0, 2400.0);
const LANDSCAPE: (f32, f32) = (2400.0, 1080.0);
const DENSITY: f32 = 2.75;

struct Phone {
    app: FilmcraftApp,
    chrome: Chrome,
    /// Touch -> mouse translation, as the Android shell applies it to the window's events.
    gestures: filmcraft::touch::Gestures,
}

impl eframe::App for Phone {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.gestures.process(&mut raw.events, std::time::Instant::now());
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
    phone_sized(PORTRAIT, project)
}

fn phone_sized(px: (f32, f32), project: impl FnOnce(&mut Session)) -> Harness<'static, Phone> {
    let mut s = Session::default();
    project(&mut s);
    let size = egui::vec2(px.0, px.1) / DENSITY;
    let mut h = Harness::builder().with_size(size).with_pixels_per_point(DENSITY).with_max_steps(10_000).wgpu().build_eframe(move |_cc| Phone {
        app: FilmcraftApp::new(s),
        chrome: Chrome::default(),
        gestures: Default::default(),
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
    reveal(h, id);
    let at = control_rect(h, id).center();
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(7), phase, pos: at, force: None };
    h.input_mut().events.push(touch(egui::TouchPhase::Start));
    step(h, 1);
    h.input_mut().events.push(touch(egui::TouchPhase::End));
    step(h, 4);
}

/// One finger on the screen: touch events as winit delivers them, moving in steps from `from` to `to`.
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

/// Scroll the tool strip or the open sheet until the control is on screen, as a finger would.
fn reveal(h: &mut Harness<'static, Phone>, id: &str) {
    let in_sheet = ["phone.ramp.", "phone.speed.", "phone.point."].iter().any(|p| id.starts_with(p));
    if !(id.starts_with("touchbar.") || in_sheet) {
        return;
    }
    for _ in 0..8 {
        let r = control_rect(h, id);
        let screen = h.ctx.content_rect();
        let strip_top = control_rect(h, "touchbar.import").min.y;
        if id.starts_with("touchbar.") {
            if r.max.x <= screen.max.x && r.min.x >= screen.min.x {
                return;
            }
            let y = r.center().y;
            finger_drag(h, egui::pos2(screen.max.x - 20.0, y), egui::pos2(screen.min.x + 40.0, y), 8);
        } else {
            let top = control_rect(h, "phone.sheet.close").max.y;
            if r.min.y >= top && r.max.y <= strip_top {
                return;
            }
            let x = screen.center().x;
            let (from, to) = if r.min.y < top { (top + 40.0, strip_top - 40.0) } else { (strip_top - 40.0, top + 40.0) };
            finger_drag(h, egui::pos2(x, from), egui::pos2(x, to), 8);
        }
    }
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

fn clip_ids(h: &Harness<'static, Phone>) -> Vec<filmcraft_project::ClipId> {
    h.state().app.session.active_sequence().map(|q| q.video_tracks.iter().flat_map(|t| t.items.iter().map(|i| i.id)).collect()).unwrap_or_default()
}

fn control_exists(h: &Harness<'static, Phone>, id: &str) -> bool {
    h.state().app.auto.find(id).is_some()
}

#[test]
fn tapping_add_puts_the_chosen_clips_on_the_timeline() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    shot(&mut h, "01-start");
    assert_eq!(video_clips(&h), 0);
    // tapping Add with nothing chosen says what to do
    tap(&mut h, "touchbar.add");
    assert!(h.state().app.ui.status.contains("Project panel"), "status: {}", h.state().app.ui.status);
    // Media opens the Project panel in place of the timeline; choose a clip, as a tap there does,
    // then Add: a timeline is made from it
    tap(&mut h, "touchbar.media");
    shot(&mut h, "02-media");
    h.state_mut().app.session.state.project_selection = vec![ids[0]];
    tap(&mut h, "phone.media.add");
    assert_eq!(video_clips(&h), 1);
    // more clips go back to back after it
    h.state_mut().app.session.state.project_selection = vec![ids[1], ids[2]];
    tap(&mut h, "phone.media.add");
    assert_eq!(video_clips(&h), 3);
    // Insert at the playhead
    h.state_mut().app.session.state.project_selection = vec![ids[0]];
    tap(&mut h, "phone.media.insert");
    assert_eq!(video_clips(&h), 4);
    // closing the sheet brings the timeline back
    tap(&mut h, "phone.sheet.close");
    assert!(control_exists(&h, "panel.Timeline"));
    shot(&mut h, "03-timeline");
    // Undo takes the last edit back as one step
    tap(&mut h, "phone.undo");
    assert_eq!(video_clips(&h), 3);
    tap(&mut h, "phone.redo");
    assert_eq!(video_clips(&h), 4);
}

#[test]
fn the_buttons_are_finger_sized_and_fit_the_screen_in_both_orientations() {
    for (name, px) in [("portrait", PORTRAIT), ("landscape", LANDSCAPE)] {
        let mut h = phone_sized(px, |s| {
            with_clips(s);
        });
        let screen = h.ctx.content_rect();
        let zoom = h.ctx.zoom_factor();
        for id in [
            "phone.menu",
            "phone.undo",
            "phone.redo",
            "phone.export",
            "phone.play",
            "phone.stepback",
            "phone.stepforward",
            "touchbar.import",
            "touchbar.media",
            "touchbar.add",
            "touchbar.split",
            "touchbar.delete",
            "touchbar.speed",
        ] {
            let r = control_rect(&h, id);
            // at least 44 dp each way: points x zoom = dp
            assert!(r.height() * zoom >= 44.0, "{name}: {id} is {} dp tall", r.height() * zoom);
            assert!(r.width() * zoom >= 44.0, "{name}: {id} is {} dp wide", r.width() * zoom);
            assert!(r.min.y >= screen.min.y - 0.5 && r.max.y <= screen.max.y + 0.5, "{name}: {id} {r:?} is off the screen {screen:?}");
            assert!(
                r.min.x >= screen.min.x - 0.5 && r.max.x <= screen.max.x + 0.5 || id.starts_with("touchbar."),
                "{name}: {id} {r:?} is off the screen {screen:?}"
            );
        }
        // the picture and the timeline both have room
        let monitor = control_rect(&h, "panel.Program");
        let timeline = control_rect(&h, "panel.Timeline");
        assert!(monitor.width() * zoom >= 200.0 && monitor.height() * zoom >= 110.0, "{name}: monitor {monitor:?}");
        assert!(timeline.width() * zoom >= 250.0 && timeline.height() * zoom >= 150.0, "{name}: timeline {timeline:?}");
        shot(&mut h, &format!("04-{name}"));
    }
}

#[test]
fn every_sheet_opens_and_closes() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    h.state_mut().app.session.state.project_selection = vec![ids[0]];
    tap(&mut h, "touchbar.add");
    for (tool, panel) in [
        ("media", Some("panel.Project")),
        ("speed", None),
        ("properties", Some("panel.EffectControls")),
        ("audio", Some("panel.AudioClipMixer")),
        ("effects", Some("panel.Effects")),
        ("text", Some("panel.Text")),
        ("color", Some("panel.LumetriColor")),
    ] {
        tap(&mut h, &format!("touchbar.{tool}"));
        assert!(control_exists(&h, "phone.sheet.close"), "{tool} opens a sheet");
        assert!(!control_exists(&h, "panel.Timeline"), "{tool} takes the timeline's place");
        if let Some(p) = panel {
            assert!(control_exists(&h, p), "{tool} shows {p}");
        }
        shot(&mut h, &format!("05-sheet-{tool}"));
        // the same tool again closes it
        tap(&mut h, &format!("touchbar.{tool}"));
        assert!(!control_exists(&h, "phone.sheet.close"), "{tool} closes");
    }
    // the menus
    tap(&mut h, "phone.menu");
    assert!(control_exists(&h, "phone.sheet.close"));
    shot(&mut h, "06-menu");
    tap(&mut h, "phone.sheet.close");
    // a command that shows a panel opens its sheet
    h.state_mut().app.show_panel(filmcraft_ui_egui::dock::PanelKind::Effects);
    step(&mut h, 3);
    assert!(control_exists(&h, "panel.Effects"));
}

fn timeline_clip_point(h: &Harness<'static, Phone>) -> egui::Pos2 {
    let panel = control_rect(h, "panel.Timeline");
    let row = control_rect(h, "timeline.track.V1.enabled");
    egui::pos2(panel.min.x + h.state().app.ui.timeline.header_w + 60.0, row.center().y)
}

#[test]
fn a_speed_ramp_can_be_made_edited_and_removed_with_taps() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    h.state_mut().app.session.state.project_selection = vec![ids[0]];
    tap(&mut h, "touchbar.add");
    let clip = clip_ids(&h)[0];
    let item = |h: &Harness<'static, Phone>| h.state().app.session.active_sequence().and_then(|q| q.find_item(clip).map(|(_, i)| i.clone())).unwrap();
    // choose the clip with a tap on the timeline, then open Speed
    let at = timeline_clip_point(&h);
    let touch = |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(3), phase, pos: at, force: None };
    h.input_mut().events.push(touch(egui::TouchPhase::Start));
    step(&mut h, 2);
    h.input_mut().events.push(touch(egui::TouchPhase::End));
    step(&mut h, 4);
    assert!(h.state().app.session.state.selection.contains(&clip), "a tap chooses the clip");
    tap(&mut h, "touchbar.speed");
    shot(&mut h, "07-speed");
    let before = item(&h);
    // a preset shape ramps the clip without changing what it plays or how long
    tap(&mut h, "phone.ramp.montage");
    let ramped = item(&h);
    assert!(ramped.speed_ramp().is_some_and(|p| p.keyframes.len() == 3), "the shape's points are on the clip");
    assert_eq!((ramped.start, ramped.duration), (before.start, before.duration));
    assert!((ramped.source_out() - before.source_out()).0.abs() < 1_000_000_000, "the same picture plays");
    shot(&mut h, "08-speed-ramp");
    // a point at the playhead, faster with +, and gone with the cross
    h.state_mut().app.session.set_playhead(ramped.start + filmcraft_time::Tick(ramped.duration.0 / 4));
    tap(&mut h, "phone.ramp.addpoint");
    assert_eq!(item(&h).speed_ramp().map(|p| p.keyframes.len()), Some(4));
    let first = item(&h).speed_ramp().unwrap().keyframes[0].value.as_f64().unwrap();
    tap(&mut h, "phone.point.0.plus");
    let after = item(&h).speed_ramp().unwrap().keyframes[0].value.as_f64().unwrap();
    assert!(after > first * 1.2, "{first} -> {after}");
    tap(&mut h, "phone.point.1.delete");
    assert_eq!(item(&h).speed_ramp().map(|p| p.keyframes.len()), Some(3));
    // removing the ramp, then a constant speed: the clip gets shorter and plays at 200 %
    tap(&mut h, "phone.ramp.none");
    assert!(item(&h).speed_ramp().is_none());
    tap(&mut h, "phone.speed.200");
    let fast = item(&h);
    assert!((fast.speed - 2.0).abs() < 1e-9 && fast.duration < before.duration, "{} {:?} status: {}", fast.speed, fast.duration, h.state().app.ui.status);
    tap(&mut h, "phone.speed.reverse");
    assert!(item(&h).reverse);
    shot(&mut h, "09-speed-reverse");
    // Undo goes back through them one at a time
    tap(&mut h, "phone.sheet.close");
    tap(&mut h, "phone.undo");
    assert!(!item(&h).reverse);
}

#[test]
fn a_tap_selects_a_project_clip_and_a_long_press_opens_its_menu() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    tap(&mut h, "touchbar.media");
    let at = control_rect(&h, &format!("project.item.{}", ids[1].0)).center();
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
    shot(&mut h, "10-long-press");
    h.input_mut().events.push(touch(egui::TouchPhase::End));
    step(&mut h, 3);
    assert!(opened, "holding a clip opens its menu");
}

#[test]
fn the_keyboard_pushes_the_interface_up_and_the_strip_aside() {
    let mut h = phone(|s| {
        with_clips(s);
    });
    tap(&mut h, "touchbar.media");
    assert!(control_exists(&h, "touchbar.add"));
    let before = control_rect(&h, "panel.Project");
    h.state_mut().chrome.keyboard = 200.0;
    step(&mut h, 3);
    assert!(!control_exists(&h, "touchbar.add"), "the strip steps aside while the keyboard is up");
    let after = control_rect(&h, "panel.Project");
    let screen = h.ctx.content_rect();
    assert!(before.max.y > screen.max.y - 200.0, "without the keyboard the panel reaches down: {before:?} of {screen:?}");
    assert!(after.max.y <= screen.max.y - 200.0, "the Project panel is above the keyboard: {after:?} of {screen:?}");
    shot(&mut h, "11-keyboard");
    h.state_mut().chrome.keyboard = 0.0;
    step(&mut h, 3);
    assert!(control_exists(&h, "touchbar.add"));
}

#[test]
fn the_export_screen_fits_a_phone_and_leads_back() {
    let mut ids = Vec::new();
    let mut h = phone(|s| ids = with_clips(s));
    h.state_mut().app.session.state.project_selection = vec![ids[0]];
    tap(&mut h, "touchbar.add");
    tap(&mut h, "phone.export");
    assert_eq!(h.state().app.ui.mode, filmcraft_ui_egui::state::Mode::Export);
    shot(&mut h, "12-export-screen");
    tap(&mut h, "phone.back");
    assert_eq!(h.state().app.ui.mode, filmcraft_ui_egui::state::Mode::Edit);
    // the desktop Import page is never shown: the Import button is the file browser
    h.state_mut().app.ui.mode = filmcraft_ui_egui::state::Mode::Import;
    step(&mut h, 3);
    assert_eq!(h.state().app.ui.mode, filmcraft_ui_egui::state::Mode::Edit);
}
