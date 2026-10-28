//! The edits the touch bar offers: what a tap does where the desktop app has a drag, a keyboard
//! shortcut or a menu. They work on the session alone (no egui), so they are tested on the host.
//!
//! The common story, on a phone: import clips, tap one (or several) in the Project panel, tap
//! **Add**. The clips land back to back at the end of the timeline, and a timeline that does not
//! exist yet is made from the first clip (its size and frame rate), as dropping a clip on an empty
//! Timeline does on the desktop.

use filmcraft_engine::Session;
use filmcraft_project::ItemId;
use filmcraft_ui_egui::i18n::t;
use serde_json::json;

/// Where Add puts the chosen clips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// After the last clip of the timeline, back to back (the playhead moves to the first one).
    End,
    /// At the playhead, pushing everything after it later (Insert, `,`).
    Insert,
    /// At the playhead, replacing what is there (Overwrite, `.`).
    Overwrite,
}

/// The items Add takes: the Project panel's selection (bins and the open sequence left out), else
/// the clip open in the Source monitor.
pub fn items_to_add(s: &Session) -> Vec<ItemId> {
    let active = s.state.active_sequence;
    let mut items: Vec<ItemId> = s.state.project_selection.iter().copied().filter(|i| Some(*i) != active && s.project.item(*i).is_some()).collect();
    if items.is_empty()
        && let Some(src) = s.state.source_item.filter(|i| Some(*i) != active && s.project.item(*i).is_some())
    {
        items.push(src);
    }
    items
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { t(one) } else { t(many) })
}

/// Add the chosen clips to the timeline. Returns what to tell the user.
pub fn add_to_timeline(s: &mut Session, how: Place) -> Result<String, String> {
    let items = items_to_add(s);
    if items.is_empty() {
        return Err(t("Tap a clip in the Project panel first, then tap Add").to_string());
    }
    let ids: Vec<u64> = items.iter().map(|i| i.0).collect();
    // no timeline yet: make one from the first clip, which also places the clips
    if s.active_sequence().is_none() {
        let r = s.execute("file.newSequenceFromClip", json!({"items": ids})).map_err(|e| e.to_string())?;
        let _ = r;
        s.set_playhead(filmcraft_time::Tick::ZERO);
        return Ok(format!("{} {}", t("New timeline made:"), plural(items.len(), "clip added", "clips added")));
    }
    let end = s.active_sequence().map(|q| q.duration()).unwrap_or_default();
    let mut params = json!({"items": ids});
    match how {
        Place::End => params["time"] = json!(end.0),
        Place::Insert => params["insert"] = json!(true),
        Place::Overwrite => params["insert"] = json!(false),
    }
    s.execute("timeline.place", params).map_err(|e| e.to_string())?;
    if how == Place::End {
        // show what was added
        s.set_playhead(end);
    }
    Ok(plural(items.len(), "clip added", "clips added"))
}

/// Cut the clips under the playhead in two (Add Edit).
pub fn split_at_playhead(s: &mut Session) -> Result<String, String> {
    if s.active_sequence().is_none() {
        return Err(t("There is no timeline yet: add a clip first").to_string());
    }
    s.execute("sequence.addEdit", json!({})).map_err(|e| e.to_string())?;
    Ok(t("Cut at the playhead").to_string())
}

/// Remove the clips selected on the timeline and close the gap (Ripple Delete).
pub fn delete_selected(s: &mut Session) -> Result<String, String> {
    if s.state.selection.is_empty() {
        return Err(t("Tap a clip on the timeline first, then tap Delete").to_string());
    }
    let n = s.state.selection.len();
    s.execute("edit.rippleDelete", json!({})).map_err(|e| e.to_string())?;
    Ok(plural(n, "clip deleted", "clips deleted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_clips() -> (Session, Vec<ItemId>) {
        let mut s = Session::default();
        s.execute("file.newProject", json!({"name": "t"})).unwrap();
        let ids: Vec<ItemId> = ["OceanSunset", "Aurora"]
            .iter()
            .flat_map(|scene| {
                let r = s.execute("file.importDemoFootage", json!({"scene": scene})).unwrap();
                r["items"].as_array().cloned().unwrap_or_default().into_iter().filter_map(|v| v.as_u64().map(ItemId)).collect::<Vec<_>>()
            })
            .collect();
        (s, ids)
    }

    fn video_clips(s: &Session) -> usize {
        s.active_sequence().map_or(0, |q| q.video_tracks.iter().map(|t| t.items.len()).sum())
    }

    #[test]
    fn nothing_chosen_says_what_to_do() {
        let (mut s, _) = session_with_clips();
        let e = add_to_timeline(&mut s, Place::End).unwrap_err();
        assert!(e.contains("Project panel"), "{e}");
    }

    #[test]
    fn add_makes_a_timeline_when_there_is_none() {
        let (mut s, ids) = session_with_clips();
        assert!(s.active_sequence().is_none() || video_clips(&s) == 0);
        s.state.project_selection = vec![ids[0]];
        add_to_timeline(&mut s, Place::End).unwrap();
        assert_eq!(video_clips(&s), 1, "the clip is on the new timeline");
    }

    #[test]
    fn add_appends_back_to_back_at_the_end() {
        let (mut s, ids) = session_with_clips();
        s.state.project_selection = vec![ids[0]];
        add_to_timeline(&mut s, Place::End).unwrap();
        let first_end = s.active_sequence().unwrap().duration();
        s.state.project_selection = vec![ids[1]];
        add_to_timeline(&mut s, Place::End).unwrap();
        assert_eq!(video_clips(&s), 2);
        let q = s.active_sequence().unwrap();
        let starts: Vec<_> = q.video_tracks.iter().flat_map(|t| t.items.iter().map(|c| c.start)).collect();
        assert!(starts.contains(&first_end), "the second clip starts where the first ends: {starts:?} vs {first_end:?}");
        assert_eq!(s.playhead(), first_end, "the playhead shows the clip just added");
    }

    #[test]
    fn several_selected_clips_are_added_in_one_undo_step() {
        let (mut s, ids) = session_with_clips();
        s.state.project_selection = vec![ids[0]];
        add_to_timeline(&mut s, Place::End).unwrap();
        s.state.project_selection = ids.clone();
        add_to_timeline(&mut s, Place::End).unwrap();
        assert_eq!(video_clips(&s), 3);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(video_clips(&s), 1, "one undo removes the clips added together");
    }

    #[test]
    fn the_source_monitor_clip_is_added_when_nothing_is_selected() {
        let (mut s, ids) = session_with_clips();
        s.state.project_selection = vec![ids[0]];
        add_to_timeline(&mut s, Place::End).unwrap();
        s.state.project_selection.clear();
        s.state.source_item = Some(ids[1]);
        add_to_timeline(&mut s, Place::End).unwrap();
        assert_eq!(video_clips(&s), 2);
    }

    #[test]
    fn insert_and_overwrite_work_at_the_playhead() {
        let (mut s, ids) = session_with_clips();
        s.state.project_selection = vec![ids[0]];
        add_to_timeline(&mut s, Place::End).unwrap();
        s.set_playhead(filmcraft_time::Tick::ZERO);
        s.state.project_selection = vec![ids[1]];
        add_to_timeline(&mut s, Place::Insert).unwrap();
        assert_eq!(video_clips(&s), 2, "insert pushes the first clip later");
        s.state.project_selection = vec![ids[1]];
        add_to_timeline(&mut s, Place::Overwrite).unwrap();
        assert!(video_clips(&s) >= 2);
    }

    #[test]
    fn split_and_delete_need_a_timeline_and_a_selection() {
        let (mut s, ids) = session_with_clips();
        assert!(split_at_playhead(&mut s).is_err(), "no timeline");
        s.state.project_selection = vec![ids[0]];
        add_to_timeline(&mut s, Place::End).unwrap();
        assert!(delete_selected(&mut s).is_err(), "nothing selected on the timeline");
        // cut the clip in the middle, select the first half, delete it
        let half = s.active_sequence().unwrap().duration().0 / 2;
        s.set_playhead(filmcraft_time::Tick(half));
        s.execute("edit.selectAll", json!({})).unwrap();
        split_at_playhead(&mut s).unwrap();
        assert_eq!(video_clips(&s), 2, "the clip is cut in two");
        let first = s.active_sequence().unwrap().video_tracks.iter().flat_map(|t| t.items.iter()).min_by_key(|c| c.start).map(|c| c.id).unwrap();
        s.execute("timeline.select", json!({"clips": [first.0], "linked": true})).unwrap();
        delete_selected(&mut s).unwrap();
        assert_eq!(video_clips(&s), 1);
    }
}
