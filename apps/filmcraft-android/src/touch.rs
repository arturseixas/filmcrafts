//! Touch gestures: one finger is the mouse, two fingers zoom and scroll.
//!
//! egui-winit turns the first finger into a mouse press at once. A pinch then starts as a press,
//! and that press drags whatever is under the first finger (a clip in the Timeline) while the
//! fingers zoom. Here the press waits until the finger moves past [`SLOP`], is held for
//! [`PRESS_DELAY`], or lifts (a tap). A second finger that lands before then turns the touch into a
//! two-finger gesture with no press at all:
//!
//! - spreading or pinching the fingers sends [`egui::Event::Zoom`] (the Timeline zooms around the
//!   fingers, as with a trackpad pinch);
//! - moving them together sends a scroll ([`egui::Event::MouseWheel`], trackpad-like): panels,
//!   lists and the Timeline scroll.
//!
//! The translator replaces egui-winit's pointer events (on Android every pointer event comes from a
//! touch) and keeps the [`egui::Event::Touch`] events, so egui's own touch handling (long press for
//! context menus, `multi_touch`) still sees the fingers.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use egui::{Event, Modifiers, Pos2, TouchPhase, Vec2};

/// How far a finger moves (points) before its press starts a drag.
pub const SLOP: f32 = 8.0;
/// How long a still finger waits before it presses (a second finger within this makes a gesture).
pub const PRESS_DELAY: Duration = Duration::from_millis(120);

/// The finger that acts as the mouse.
#[derive(Clone, Copy, Debug)]
struct Primary {
    id: u64,
    start: Pos2,
    last: Pos2,
    at: Instant,
    pressed: bool,
}

#[derive(Default)]
pub struct Gestures {
    touches: BTreeMap<u64, Pos2>,
    primary: Option<Primary>,
    /// Two-finger gesture: the fingers' last distance and midpoint.
    pinch: Option<(f32, Pos2)>,
    /// A tap's release, sent a frame after its press (widgets see the press first).
    release_next: Option<Pos2>,
    /// The keyboard modifiers (a hardware keyboard), for the presses sent.
    modifiers: Modifiers,
}

fn press(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers }
}

impl Gestures {
    /// Rewrite this frame's events (see the module docs). Returns how soon another frame is needed
    /// to press a finger that is being held still, if one is.
    pub fn process(&mut self, events: &mut Vec<Event>, now: Instant) -> Option<Duration> {
        let mut out = Vec::with_capacity(events.len() + 4);
        if let Some(p) = self.release_next.take() {
            out.push(press(p, false, self.modifiers));
            out.push(Event::PointerGone);
        }
        for e in std::mem::take(events) {
            match e {
                Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone => {}
                Event::Touch { id, phase, pos, .. } => {
                    out.push(e);
                    self.touch(id.0, phase, pos, now, &mut out);
                }
                Event::ModifiersChanged(m) => {
                    self.modifiers = m;
                    out.push(e);
                }
                other => out.push(other),
            }
        }
        let modifiers = self.modifiers;
        let mut wait = None;
        if let Some(p) = self.primary.as_mut()
            && !p.pressed
            && self.pinch.is_none()
        {
            let held = now.saturating_duration_since(p.at);
            if held >= PRESS_DELAY {
                p.pressed = true;
                out.push(Event::PointerMoved(p.start));
                out.push(press(p.start, true, modifiers));
                if p.last != p.start {
                    out.push(Event::PointerMoved(p.last));
                }
            } else {
                wait = Some(PRESS_DELAY - held);
            }
        }
        *events = out;
        wait
    }

    fn touch(&mut self, id: u64, phase: TouchPhase, pos: Pos2, now: Instant, out: &mut Vec<Event>) {
        let modifiers = self.modifiers;
        match phase {
            TouchPhase::Start => {
                self.touches.insert(id, pos);
                match self.touches.len() {
                    1 => {
                        self.primary = Some(Primary { id, start: pos, last: pos, at: now, pressed: false });
                        self.pinch = None;
                        out.push(Event::PointerMoved(pos));
                    }
                    // a second finger before the first pressed: a gesture, never a press
                    2 if self.primary.is_some_and(|p| !p.pressed) => {
                        self.primary = None;
                        let (d, c) = self.span();
                        self.pinch = Some((d, c));
                        out.push(Event::PointerMoved(c));
                    }
                    // the first finger already pressed (a drag is under way), or a third finger
                    _ => {}
                }
            }
            TouchPhase::Move => {
                if let Some(p) = self.touches.get_mut(&id) {
                    *p = pos;
                }
                if let Some((d0, c0)) = self.pinch {
                    if self.touches.len() < 2 {
                        return;
                    }
                    let (d, c) = self.span();
                    out.push(Event::PointerMoved(c));
                    if d0 > 1.0 && d > 1.0 {
                        let zoom = d / d0;
                        if zoom.is_finite() && (zoom - 1.0).abs() > 1e-3 {
                            out.push(Event::Zoom(zoom));
                        }
                    }
                    let delta: Vec2 = c - c0;
                    if delta.length_sq() > 0.0 {
                        out.push(Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            source: egui::MouseWheelSource::Trackpad,
                            delta,
                            modifiers: Modifiers::NONE,
                            phase: TouchPhase::Move,
                        });
                    }
                    self.pinch = Some((d, c));
                    return;
                }
                let Some(p) = self.primary.as_mut().filter(|p| p.id == id) else { return };
                p.last = pos;
                if !p.pressed && (pos - p.start).length() > SLOP {
                    p.pressed = true;
                    out.push(Event::PointerMoved(p.start));
                    out.push(press(p.start, true, modifiers));
                }
                if p.pressed {
                    out.push(Event::PointerMoved(pos));
                }
            }
            TouchPhase::End | TouchPhase::Cancel => {
                self.touches.remove(&id);
                if self.pinch.is_some() {
                    if self.touches.len() < 2 {
                        // the finger left behind does not become a mouse
                        self.pinch = None;
                        out.push(Event::PointerGone);
                    }
                    return;
                }
                let Some(p) = self.primary.filter(|p| p.id == id) else { return };
                self.primary = None;
                match (p.pressed, phase) {
                    (true, TouchPhase::Cancel) => {
                        out.push(press(pos, false, modifiers));
                        out.push(Event::PointerGone);
                    }
                    (true, _) => {
                        out.push(Event::PointerMoved(pos));
                        out.push(press(pos, false, modifiers));
                        out.push(Event::PointerGone);
                    }
                    // a tap: press now, release next frame
                    (false, TouchPhase::End) => {
                        out.push(Event::PointerMoved(p.start));
                        out.push(press(p.start, true, modifiers));
                        self.release_next = Some(p.start);
                    }
                    (false, _) => out.push(Event::PointerGone),
                }
            }
        }
    }

    /// Distance between the first two fingers and their midpoint.
    fn span(&self) -> (f32, Pos2) {
        let mut it = self.touches.values();
        match (it.next(), it.next()) {
            (Some(a), Some(b)) => ((*b - *a).length(), a.lerp(*b, 0.5)),
            (Some(a), None) => (0.0, *a),
            _ => (0.0, Pos2::ZERO),
        }
    }

    /// A release is waiting for the next frame.
    pub fn pending(&self) -> bool {
        self.release_next.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(id: u64, phase: TouchPhase, x: f32, y: f32) -> Event {
        Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos: egui::pos2(x, y), force: None }
    }

    fn run(g: &mut Gestures, events: Vec<Event>, now: Instant) -> Vec<Event> {
        let mut events = events;
        g.process(&mut events, now);
        events.retain(|e| !matches!(e, Event::Touch { .. }));
        events
    }

    fn presses(events: &[Event]) -> Vec<bool> {
        events.iter().filter_map(|e| if let Event::PointerButton { pressed, .. } = e { Some(*pressed) } else { None }).collect()
    }

    #[test]
    fn a_tap_presses_then_releases_on_the_next_frame() {
        let mut g = Gestures::default();
        let t = Instant::now();
        let down = run(&mut g, vec![touch(1, TouchPhase::Start, 10.0, 10.0)], t);
        assert!(presses(&down).is_empty(), "no press before the finger moves, waits or lifts");
        let up = run(&mut g, vec![touch(1, TouchPhase::End, 10.0, 10.0)], t + Duration::from_millis(50));
        assert_eq!(presses(&up), vec![true]);
        assert!(g.pending());
        let next = run(&mut g, vec![], t + Duration::from_millis(60));
        assert_eq!(presses(&next), vec![false]);
        assert!(next.contains(&Event::PointerGone));
    }

    #[test]
    fn a_drag_presses_where_the_finger_landed() {
        let mut g = Gestures::default();
        let t = Instant::now();
        run(&mut g, vec![touch(1, TouchPhase::Start, 10.0, 10.0)], t);
        let moved = run(&mut g, vec![touch(1, TouchPhase::Move, 30.0, 10.0)], t + Duration::from_millis(20));
        let Some(Event::PointerButton { pos, pressed: true, .. }) = moved.iter().find(|e| matches!(e, Event::PointerButton { .. })) else {
            panic!("no press: {moved:?}")
        };
        assert_eq!(*pos, egui::pos2(10.0, 10.0));
        assert_eq!(moved.last(), Some(&Event::PointerMoved(egui::pos2(30.0, 10.0))));
        let up = run(&mut g, vec![touch(1, TouchPhase::End, 30.0, 10.0)], t + Duration::from_millis(40));
        assert_eq!(presses(&up), vec![false]);
    }

    #[test]
    fn a_held_finger_presses_after_the_delay() {
        let mut g = Gestures::default();
        let t = Instant::now();
        let mut events = vec![touch(1, TouchPhase::Start, 5.0, 5.0)];
        let wait = g.process(&mut events, t);
        assert!(wait.is_some_and(|w| w <= PRESS_DELAY));
        let later = run(&mut g, vec![], t + PRESS_DELAY);
        assert_eq!(presses(&later), vec![true]);
    }

    #[test]
    fn two_fingers_zoom_and_scroll_without_pressing() {
        let mut g = Gestures::default();
        let t = Instant::now();
        run(&mut g, vec![touch(1, TouchPhase::Start, 100.0, 100.0)], t);
        run(&mut g, vec![touch(2, TouchPhase::Start, 200.0, 100.0)], t + Duration::from_millis(30));
        let spread = run(&mut g, vec![touch(2, TouchPhase::Move, 300.0, 100.0)], t + Duration::from_millis(60));
        let zoom = spread.iter().find_map(|e| if let Event::Zoom(z) = e { Some(*z) } else { None });
        assert!(zoom.is_some_and(|z| (z - 2.0).abs() < 1e-4), "{spread:?}");
        let pan = run(&mut g, vec![touch(1, TouchPhase::Move, 100.0, 140.0), touch(2, TouchPhase::Move, 300.0, 140.0)], t + Duration::from_millis(90));
        let scrolled: Vec2 =
            pan.iter().filter_map(|e| if let Event::MouseWheel { delta, .. } = e { Some(*delta) } else { None }).fold(Vec2::ZERO, |a, d| a + d);
        assert!((scrolled.y - 40.0).abs() < 1e-3, "{pan:?}");
        let mut all = spread;
        all.extend(pan);
        all.extend(run(&mut g, vec![touch(1, TouchPhase::End, 100.0, 140.0), touch(2, TouchPhase::End, 300.0, 140.0)], t + Duration::from_millis(120)));
        all.extend(run(&mut g, vec![], t + Duration::from_secs(1)));
        assert!(presses(&all).is_empty(), "a gesture never presses: {all:?}");
    }

    #[test]
    fn a_second_finger_during_a_drag_changes_nothing() {
        let mut g = Gestures::default();
        let t = Instant::now();
        run(&mut g, vec![touch(1, TouchPhase::Start, 10.0, 10.0)], t);
        run(&mut g, vec![touch(1, TouchPhase::Move, 40.0, 10.0)], t + Duration::from_millis(10));
        let second = run(&mut g, vec![touch(2, TouchPhase::Start, 200.0, 10.0), touch(2, TouchPhase::Move, 260.0, 10.0)], t + Duration::from_millis(20));
        assert!(!second.iter().any(|e| matches!(e, Event::Zoom(_) | Event::MouseWheel { .. })), "{second:?}");
        let up = run(&mut g, vec![touch(1, TouchPhase::End, 40.0, 10.0)], t + Duration::from_millis(30));
        assert_eq!(presses(&up), vec![false]);
    }

    #[test]
    fn native_pointer_events_are_replaced() {
        let mut g = Gestures::default();
        let out = run(
            &mut g,
            vec![Event::PointerMoved(egui::pos2(1.0, 1.0)), press(egui::pos2(1.0, 1.0), true, Modifiers::NONE), Event::Text("a".into())],
            Instant::now(),
        );
        assert_eq!(out, vec![Event::Text("a".into())]);
    }
}
