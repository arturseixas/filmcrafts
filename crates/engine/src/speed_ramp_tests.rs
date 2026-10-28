//! `clip.speedRamp`: a ramp keeps the clip's length and the media it plays, reaches the linked
//! sound, undoes in one step and refuses what cannot ramp.

use filmcraft_project::{ClipId, TrackItem};
use filmcraft_time::Tick;
use serde_json::json;

use crate::Session;

fn item(s: &Session, c: ClipId) -> TrackItem {
    s.active_sequence().unwrap().find_item(c).unwrap().1.clone()
}

fn one_clip(s: &mut Session) -> (ClipId, ClipId) {
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Ramp", "video": 1, "audio": 1})).unwrap();
    let rate = s.sequence_rate();
    let media = s.project.items.values().find(|i| i.has_video() && i.has_audio() && i.as_media().is_some()).map(|i| i.id).unwrap();
    s.execute("timeline.place", json!({"item": media.0, "frame": 0, "sourceIn": rate.tick_of(24).0, "duration": rate.tick_of(96).0})).unwrap();
    let q = s.active_sequence().unwrap();
    (q.video_tracks[0].items[0].id, q.audio_tracks[0].items[0].id)
}

#[test]
fn a_preset_ramps_the_clip_and_its_sound_without_changing_what_they_play() {
    let mut s = Session::default();
    let (picture, sound) = one_clip(&mut s);
    let before = item(&s, picture);
    let r = s.execute("clip.speedRamp", json!({"clips": [picture.0], "preset": "montage"})).unwrap();
    assert_eq!(r["clips"], json!(2), "the linked sound ramps with the picture");
    s.active_sequence().unwrap().check().unwrap();
    for c in [picture, sound] {
        let ramped = item(&s, c);
        let keys = &ramped.speed_ramp().unwrap().keyframes;
        assert_eq!(keys.len(), 3);
        assert_eq!((ramped.start, ramped.duration, ramped.source_in), (before.start, before.duration, before.source_in));
        let tol = s.sequence_rate().frame_duration();
        assert!(
            (ramped.source_out() - before.source_out()).0.abs() < tol.0 / 4,
            "same media in the same time: {:?} vs {:?}",
            ramped.source_out(),
            before.source_out()
        );
        // the middle is slow, the ends are fast: media played in one frame
        let mid = Tick(ramped.duration.0 / 2);
        let step = |at: Tick| ramped.moving_source_time_at(at + tol) - ramped.moving_source_time_at(at);
        assert!(step(Tick::ZERO) > step(mid), "{:?} {:?}", step(Tick::ZERO), step(mid));
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert!(item(&s, picture).speed_ramp().is_none() && item(&s, sound).speed_ramp().is_none(), "one undo step");
}

#[test]
fn custom_keys_and_clearing() {
    let mut s = Session::default();
    let (picture, _) = one_clip(&mut s);
    s.execute("clip.speedRamp", json!({"clips": [picture.0], "keys": [[0, 100], [1, 400]], "keepLength": false})).unwrap();
    let it = item(&s, picture);
    let keys = &it.speed_ramp().unwrap().keyframes;
    assert_eq!(keys[1].value.as_f64(), Some(400.0));
    assert!(it.source_out() - it.source_in > it.duration, "faster plays more media");
    s.execute("clip.speedRamp", json!({"clips": [picture.0], "preset": "none"})).unwrap();
    assert!(item(&s, picture).speed_ramp().is_none());
    for bad in [json!({}), json!({"preset": "wobble"}), json!({"keys": [[0]]}), json!({"keys": [["a", 1]]})] {
        assert!(s.execute("clip.speedRamp", bad).is_err());
    }
    // a single key and keys outside 0..1 or beyond the speed range stay in bounds
    s.execute("clip.speedRamp", json!({"clips": [picture.0], "keys": [[-3, 1e9], [9, -5]], "keepLength": false})).unwrap();
    let it = item(&s, picture);
    assert!(it.speed_ramp().unwrap().keyframes.iter().all(|k| (1.0..=10_000.0).contains(&k.value.as_f64().unwrap())));
    assert!(it.source_out() > Tick::ZERO);
}
