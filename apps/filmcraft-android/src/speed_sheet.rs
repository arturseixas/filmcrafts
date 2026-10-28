//! The Speed sheet: constant speed and speed ramps for the chosen clip, with finger-sized controls.
//!
//! * **Speed**: tap a preset (25 % … 400 %) or Reverse. The clip gets shorter or longer and the
//!   clips after it follow (`clip.speedDuration` with ripple).
//! * **Ramp**: tap a shape (slow to fast, hero time, …) and the clip speeds up and slows down
//!   along it while still playing the same picture in the same time (`clip.speedRamp`).
//! * **Points**: every point of the ramp with its time and speed; ‒ and + change a speed by a
//!   quarter, ✕ removes the point, and *Add point* puts one at the playhead (`clip.speedRampKey`).
//!
//! The graph shows the speed along the clip; tapping it moves the playhead. The sheet only runs
//! commands (by id, like the menus), so it works the same from the control channel.

use egui::{Align2, Color32, Id, Rect, Sense, Stroke, UiBuilder, pos2, vec2};
use filmcraft_engine::speed_ramp::PRESETS;
use filmcraft_project::{ClipId, TrackItem};
use filmcraft_time::{FrameRate, Tick, TimeDisplay, format_time};
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::i18n::t;
use serde_json::{Value, json};

use crate::phone::{dp, notify, pill};

/// Constant speeds offered, in percent.
const SPEEDS: [f64; 7] = [25.0, 50.0, 75.0, 100.0, 150.0, 200.0, 400.0];
/// Samples along the clip in the graph.
const SAMPLES: usize = 96;
/// A point's speed is multiplied or divided by this by ‒ and +.
const STEP: f64 = 1.25;

/// The clip the sheet works on: the first chosen clip that is on a video track, else the first.
pub fn target(app: &FilmcraftApp) -> Option<ClipId> {
    let q = app.session.active_sequence()?;
    let chosen: Vec<ClipId> = app.session.state.selection.iter().copied().filter(|c| q.find_item(*c).is_some()).collect();
    chosen.iter().copied().find(|c| q.find_item(*c).is_some_and(|(t, _)| q.video_tracks.iter().any(|v| v.id == t))).or_else(|| chosen.first().copied())
}

/// The playing speed at fractions `0..=1` of the clip, as multiples of normal, for the graph.
pub fn curve(it: &TrackItem, samples: usize) -> Vec<f64> {
    let samples = samples.clamp(2, 4096);
    let dur = it.duration.0.max(1) as f64;
    let step = dur / samples as f64;
    (0..=samples)
        .map(|i| {
            let a = Tick(it.start.0.saturating_add((step * i as f64) as i64));
            let b = Tick(it.start.0.saturating_add((step * (i as f64 + 1.0)) as i64));
            let (m0, m1) = (it.moving_source_time_at(a), it.moving_source_time_at(b));
            let d = (b.0 - a.0).max(1) as f64;
            ((m1.0 - m0.0).abs() as f64 / d).clamp(0.0, 1000.0)
        })
        .collect()
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, body: Rect) {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    let Some(id) = target(app) else {
        ui.painter().text(
            body.center(),
            Align2::CENTER_CENTER,
            t("Tap a clip on the timeline to change its speed."),
            egui::FontId::proportional(dp(&ctx, 15.0)),
            tk.text_dim,
        );
        return;
    };
    let Some((it, rate)) = app.session.active_sequence().and_then(|q| q.find_item(id).map(|(_, it)| (it.clone(), q.settings.frame_rate))) else { return };
    let playhead = app.session.playhead();
    let mut actions: Vec<(&'static str, Value)> = Vec::new();
    let pad = dp(&ctx, 12.0);
    let mut inner = ui.new_child(UiBuilder::new().max_rect(body.shrink2(vec2(pad, 0.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt("phone-speed").auto_shrink([false, false]).show(&mut inner, |ui| {
        ui.add_space(pad);
        ui.set_width(body.width() - 2.0 * pad);
        ui.spacing_mut().item_spacing = vec2(dp(&ctx, 8.0), dp(&ctx, 8.0));
        let ramped = it.speed_ramp().is_some();
        let average = {
            let media = (it.source_out() - it.source_in).0 as f64;
            media / it.duration.0.max(1) as f64
        };
        heading(ui, &ctx, app, &format!("{} · {}", it.name, speed_text(average, ramped, it.reverse)));

        // ---- constant speed
        label(ui, &ctx, app, t("Speed"));
        ui.horizontal_wrapped(|ui| {
            for pct in SPEEDS {
                let on = !ramped && (it.speed.abs() * 100.0 - pct).abs() < 0.5;
                if chip(ui, app, &format!("phone.speed.{pct}"), &format!("{pct}%"), on) {
                    if ramped {
                        actions.push(("clip.speedRamp", json!({"clips": [id.0], "preset": "none"})));
                    }
                    actions.push(("clip.speedDuration", json!({"clips": [id.0], "speed": pct, "reverse": it.reverse, "ripple": true})));
                }
            }
            if chip(ui, app, "phone.speed.reverse", t("Reverse"), it.reverse) {
                actions.push(("clip.speedDuration", json!({"clips": [id.0], "speed": it.speed.abs() * 100.0, "reverse": !it.reverse, "ripple": true})));
            }
        });

        // ---- ramp
        ui.add_space(dp(&ctx, 6.0));
        label(ui, &ctx, app, t("Speed ramp"));
        graph(ui, app, &it, playhead, id, &mut actions);
        ui.horizontal_wrapped(|ui| {
            for (key, name, _) in PRESETS {
                if chip(ui, app, &format!("phone.ramp.{key}"), t(name), false) {
                    actions.push(("clip.speedRamp", json!({"clips": [id.0], "preset": key})));
                }
            }
            if ramped && chip(ui, app, "phone.ramp.none", t("Remove ramp"), false) {
                actions.push(("clip.speedRamp", json!({"clips": [id.0], "preset": "none"})));
            }
        });

        // ---- points
        ui.add_space(dp(&ctx, 6.0));
        label(ui, &ctx, app, t("Points"));
        if let Some(param) = it.speed_ramp() {
            for (n, k) in param.keyframes.iter().enumerate() {
                let value = k.value.as_f64().unwrap_or(100.0);
                let at = it.timeline_time_of_source(k.time);
                point_row(ui, app, id, n, value, at - it.start, at, rate, k.time, &mut actions);
            }
        } else {
            ui.label(
                egui::RichText::new(t("Choose a shape above, or add a point at the playhead, to ramp the speed.")).color(tk.text_dim).size(dp(&ctx, 13.0)),
            );
        }
        let w = ui.available_width().min(dp(&ctx, 360.0));
        let (r, _) = ui.allocate_exact_size(vec2(w, dp(&ctx, 48.0)), Sense::hover());
        if pill(ui, app, "phone.ramp.addpoint", r, t("Add point at the playhead"), true) {
            actions.push(("clip.speedRampKey", json!({"clips": [id.0]})));
        }
        ui.add_space(dp(&ctx, 24.0));
    });
    for (cmd, params) in actions {
        match app.session.execute(cmd, params) {
            Ok(_) => {}
            Err(e) => {
                app.ui.status = e.to_string();
                notify(&ctx, e.to_string(), true);
            }
        }
    }
}

fn speed_text(average: f64, ramped: bool, reverse: bool) -> String {
    let pct = (average * 100.0).round() as i64;
    let base = if ramped { format!("{} {pct}%", t("Ramp, average")) } else { format!("{pct}%") };
    if reverse { format!("{base} · {}", t("Reverse")) } else { base }
}

fn heading(ui: &mut egui::Ui, ctx: &egui::Context, app: &FilmcraftApp, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(dp(ctx, 15.0)).color(app.tokens.tab_text_active)).truncate());
}

fn label(ui: &mut egui::Ui, ctx: &egui::Context, app: &FilmcraftApp, text: &str) {
    ui.label(egui::RichText::new(text.to_uppercase()).size(dp(ctx, 11.0)).color(app.tokens.text_dim));
}

/// A selectable rounded chip, at least 44 dp high; `true` when tapped.
fn chip(ui: &mut egui::Ui, app: &mut FilmcraftApp, id: &str, text: &str, on: bool) -> bool {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    let font = egui::FontId::proportional(dp(&ctx, 14.0));
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, if on { Color32::WHITE } else { tk.text });
    let size = vec2((galley.size().x + dp(&ctx, 28.0)).max(dp(&ctx, 64.0)), dp(&ctx, 44.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = ui.interact(rect, Id::unique(id), Sense::click()).union(resp);
    let fill = match (on, resp.is_pointer_button_down_on()) {
        (true, _) => tk.accent,
        (false, true) => tk.pressed,
        (false, false) => tk.field_bg,
    };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    ui.painter().galley(rect.center() - galley.size() * 0.5, galley, tk.text);
    app.auto.add(id, rect, text);
    resp.clicked()
}

/// The speed along the clip, with the ramp's points and the playhead; a tap moves the playhead.
fn graph(ui: &mut egui::Ui, app: &mut FilmcraftApp, it: &TrackItem, playhead: Tick, id: ClipId, actions: &mut Vec<(&'static str, Value)>) {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), dp(&ctx, 120.0)), Sense::click());
    app.auto.add("phone.ramp.graph", rect, "Speed graph");
    let p = ui.painter();
    p.rect_filled(rect, dp(&ctx, 10.0), tk.plot_bg);
    let area = rect.shrink2(vec2(dp(&ctx, 10.0), dp(&ctx, 12.0)));
    let values = curve(it, SAMPLES);
    let top = values.iter().copied().fold(1.0f64, f64::max) * 1.15;
    let x_of = |f: f64| area.min.x + (f as f32).clamp(0.0, 1.0) * area.width();
    let y_of = |v: f64| area.max.y - (v / top) as f32 * area.height();
    // 100 %
    let normal = y_of(1.0);
    p.line_segment([pos2(area.min.x, normal), pos2(area.max.x, normal)], Stroke::new(1.0, tk.plot_grid));
    p.text(pos2(area.min.x + 2.0, normal - 2.0), Align2::LEFT_BOTTOM, "100%", egui::FontId::proportional(dp(&ctx, 10.0)), tk.text_faint);
    let n = values.len().saturating_sub(1).max(1);
    let line: Vec<egui::Pos2> = values.iter().enumerate().map(|(i, v)| pos2(x_of(i as f64 / n as f64), y_of(*v))).collect();
    // under the curve, in convex slices
    for w in line.windows(2) {
        let slice = vec![pos2(w[0].x, area.max.y), w[0], w[1], pos2(w[1].x, area.max.y)];
        p.add(egui::Shape::convex_polygon(slice, tk.accent.gamma_multiply(0.18), Stroke::NONE));
    }
    p.add(egui::Shape::line(line, Stroke::new(dp(&ctx, 2.5), tk.accent)));
    // the ramp's points
    let dur = it.duration.0.max(1) as f64;
    if let Some(param) = it.speed_ramp() {
        for k in &param.keyframes {
            let f = (it.timeline_time_of_source(k.time) - it.start).0 as f64 / dur;
            if (0.0..=1.0).contains(&f) {
                let v = k.value.as_f64().unwrap_or(100.0) / 100.0 * it.speed.abs();
                p.circle(pos2(x_of(f), y_of(v)), dp(&ctx, 5.0), Color32::WHITE, Stroke::new(dp(&ctx, 2.0), tk.accent));
            }
        }
    }
    // the playhead
    let at = (playhead - it.start).0 as f64 / dur;
    if (0.0..=1.0).contains(&at) {
        p.line_segment([pos2(x_of(at), rect.min.y + 2.0), pos2(x_of(at), rect.max.y - 2.0)], Stroke::new(dp(&ctx, 1.5), tk.focus));
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let f = ((pos.x - area.min.x) / area.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let time = it.start.0.saturating_add((f * dur) as i64);
        actions.push(("playhead.set", json!({"time": time})));
    }
    let _ = id;
}

/// One point of the ramp: its time in the clip, its speed with ‒ and +, and ✕.
#[allow(clippy::too_many_arguments)]
fn point_row(
    ui: &mut egui::Ui,
    app: &mut FilmcraftApp,
    id: ClipId,
    n: usize,
    value: f64,
    within: Tick,
    at: Tick,
    rate: FrameRate,
    media: Tick,
    actions: &mut Vec<(&'static str, Value)>,
) {
    let ctx = ui.ctx().clone();
    let tk = app.tokens;
    let h = dp(&ctx, 48.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    ui.painter().rect_filled(row, dp(&ctx, 10.0), tk.row_alt);
    let b = dp(&ctx, 44.0);
    // time: tap to go there
    let time_rect = Rect::from_min_size(row.min, vec2(dp(&ctx, 104.0), h));
    let resp = ui.interact(time_rect, Id::unique(("phone.point.time", n)), Sense::click());
    let text = format_time(within.max(Tick::ZERO), rate, false, TimeDisplay::Timecode, 48_000);
    ui.painter().text(pos2(time_rect.min.x + dp(&ctx, 12.0), row.center().y), Align2::LEFT_CENTER, text, egui::FontId::monospace(dp(&ctx, 13.0)), tk.timecode);
    app.auto.add(&format!("phone.point.{n}.time"), time_rect, "Go to point");
    if resp.clicked() {
        actions.push(("playhead.set", json!({"time": at.0})));
    }
    // ✕ at the right, + before it, the value, ‒ before that
    let del = Rect::from_center_size(pos2(row.max.x - b / 2.0 - dp(&ctx, 2.0), row.center().y), vec2(b, b));
    let plus = Rect::from_center_size(pos2(del.min.x - b / 2.0 - dp(&ctx, 2.0), row.center().y), vec2(b, b));
    let value_rect = Rect::from_center_size(pos2(plus.min.x - dp(&ctx, 36.0), row.center().y), vec2(dp(&ctx, 64.0), b));
    let minus = Rect::from_center_size(pos2(value_rect.min.x - b / 2.0 - dp(&ctx, 2.0), row.center().y), vec2(b, b));
    ui.painter().text(value_rect.center(), Align2::CENTER_CENTER, format!("{}%", value.round() as i64), egui::FontId::proportional(dp(&ctx, 15.0)), tk.text);
    for (label, r, key, new) in [("−", minus, "minus", Some(value / STEP)), ("+", plus, "plus", Some(value * STEP)), ("✕", del, "delete", None)] {
        let resp = ui.interact(r, Id::unique(("phone.point", n, key)), Sense::click());
        if resp.is_pointer_button_down_on() {
            ui.painter().rect_filled(r.shrink(3.0), dp(&ctx, 10.0), tk.hover);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, egui::FontId::proportional(dp(&ctx, 20.0)), tk.text);
        app.auto.add(&format!("phone.point.{n}.{key}"), r, label);
        if resp.clicked() {
            actions.push(match new {
                Some(v) => ("clip.speedRampKey", json!({"clips": [id.0], "mediaTime": media.0, "value": v.clamp(1.0, 10_000.0)})),
                None => ("clip.speedRampKey", json!({"clips": [id.0], "mediaTime": media.0, "delete": true})),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{Param, ParamValue};

    fn item() -> TrackItem {
        let mut p = filmcraft_project::Project::new("t");
        let seq = p.new_sequence("s", Default::default(), 1, 1, None);
        let _ = seq;
        let clip = p.add_item(
            "c",
            filmcraft_project::Label::Iris,
            filmcraft_project::ItemKind::Media(filmcraft_project::MediaClip {
                media: filmcraft_project::MediaRef::Generator(filmcraft_media::Generator::Demo(filmcraft_media::DemoScene::OceanSunset)),
                info: filmcraft_media::MediaSource::info(&filmcraft_media::generators::GeneratorSource::demo(filmcraft_media::DemoScene::OceanSunset)).clone(),
                interpret: Default::default(),
                mark_in: None,
                mark_out: None,
                markers: vec![],
                offline: false,
                proxy: None,
                identity: None,
            }),
            None,
        );
        let rate = FrameRate::FPS_24;
        p.make_track_item(clip, filmcraft_project::TrackKind::Video, Tick::ZERO, filmcraft_time::TimeRange::new(Tick::ZERO, rate.tick_of(96)), rate)
            .unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn the_curve_follows_the_ramp() {
        let mut it = item();
        let flat = curve(&it, 16);
        assert_eq!(flat.len(), 17);
        assert!(flat.iter().all(|v| (v - 1.0).abs() < 1e-3), "{flat:?}");
        let s = filmcraft_time::TICKS_PER_SECOND;
        let start = it.source_in;
        let e = it.effect_mut("time_remap").unwrap();
        let p = e.params.entry("speed".into()).or_insert_with(|| Param::new(ParamValue::Float(100.0)));
        p.put_keyframe(start, ParamValue::Float(50.0));
        p.put_keyframe(start + Tick(s), ParamValue::Float(400.0));
        let ramp = curve(&it, 16);
        assert!(ramp.first().copied().unwrap_or(0.0) < 1.0 && ramp.last().copied().unwrap_or(0.0) > 3.5, "{ramp:?}");
        assert!(ramp.windows(2).all(|w| w[1] >= w[0] - 1e-9));
    }
}
