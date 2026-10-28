//! Speed ramps: the Time Remapping effect's Speed keyframes, written in one step.
//!
//! A ramp keeps the clip's length and the media it plays: a preset is a *shape* (relative speeds
//! along the media the clip plays), scaled so the clip still ends where it did. Keys are in media
//! time, like every effect keyframe, so they stay on their frames when the clip is trimmed.
use filmcraft_edit::{EditError, Result};
use filmcraft_project::{ClipId, Interpolation, Keyframe, Param, ParamValue, Sequence};
use filmcraft_time::Tick;

/// Preset ramps: id, label, and the shape as (position along the media 0..=1, relative speed %).
pub const PRESETS: &[(&str, &str, &[(f64, f64)])] = &[
    ("slowToFast", "Slow to fast", &[(0.0, 25.0), (1.0, 250.0)]),
    ("fastToSlow", "Fast to slow", &[(0.0, 250.0), (1.0, 25.0)]),
    ("easeInOut", "Slow, fast, slow", &[(0.0, 30.0), (0.5, 220.0), (1.0, 30.0)]),
    ("montage", "Fast, slow, fast", &[(0.0, 300.0), (0.5, 35.0), (1.0, 300.0)]),
    ("hero", "Hero time", &[(0.0, 100.0), (0.3, 100.0), (0.45, 20.0), (0.6, 20.0), (0.75, 100.0), (1.0, 100.0)]),
    ("bullet", "Bullet time", &[(0.0, 300.0), (0.4, 300.0), (0.47, 8.0), (0.53, 8.0), (0.6, 300.0), (1.0, 300.0)]),
    ("pulse", "Pulse", &[(0.0, 60.0), (0.25, 240.0), (0.5, 60.0), (0.75, 240.0), (1.0, 60.0)]),
];

pub fn preset(id: &str) -> Option<&'static [(f64, f64)]> {
    PRESETS.iter().find(|(name, _, _)| name.eq_ignore_ascii_case(id)).map(|(_, _, shape)| *shape)
}

/// The Time Remapping Speed range (percent).
const LIMITS: (f64, f64) = (1.0, 10_000.0);

/// Give `clips` the speed ramp `shape` over the media they play now (an empty shape removes the
/// ramp). With `keep_length` the speeds are scaled so each clip still plays the same media in the
/// same time. Clips that cannot ramp (a frame hold) are an error; nothing changes then.
pub fn apply(q: &mut Sequence, clips: &[ClipId], shape: &[(f64, f64)], keep_length: bool) -> Result<usize> {
    if shape.iter().any(|(at, v)| !(at.is_finite() && v.is_finite())) {
        return Err(EditError::Other("a speed ramp needs finite positions and speeds".into()));
    }
    let mut keys: Vec<(f64, f64)> = shape.iter().map(|&(at, v)| (at.clamp(0.0, 1.0), v.abs().clamp(LIMITS.0, LIMITS.1))).collect();
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    keys.dedup_by(|b, a| a.0 == b.0);
    let mut changed = 0;
    for &c in clips {
        let (_, it) = q.find_item_mut(c).ok_or(EditError::NoItem(c))?;
        if it.frame_hold.is_some() {
            return Err(EditError::Other("a frozen frame has no speed to ramp".into()));
        }
        let mut next = it.clone();
        if next.effect("time_remap").is_none()
            && let Some(def) = filmcraft_project::find_effect("time_remap")
        {
            next.effects.push(def.instance());
        }
        let span = (next.source_out() - next.source_in).0.max(0);
        let Some(effect) = next.effect_mut("time_remap") else { continue };
        effect.enabled = true;
        let param = effect.params.entry("speed".into()).or_insert_with(|| Param::new(ParamValue::Float(100.0)));
        param.keyframes.clear();
        param.value = ParamValue::Float(100.0);
        if keys.len() >= 2 && span > 0 {
            for &(at, v) in &keys {
                let time = next.source_in + Tick((at * span as f64).round() as i64);
                let mut k = Keyframe::new(time, ParamValue::Float(v));
                k.interp = Interpolation::Linear;
                if let Some(p) = next.effect_mut("time_remap").and_then(|e| e.params.get_mut("speed")) {
                    p.keyframes.push(k);
                }
            }
            if keep_length && next.duration > Tick::ZERO {
                // all speeds scale together, so the time the shape takes scales by the inverse
                let shape_time = next.timeline_time_of_source(next.source_in + Tick(span)) - next.start;
                let k = shape_time.0 as f64 / next.duration.0 as f64;
                if k.is_finite() && k > 0.0 {
                    for key in next.effect_mut("time_remap").and_then(|e| e.params.get_mut("speed")).map(|p| &mut p.keyframes).into_iter().flatten() {
                        if let ParamValue::Float(v) = &mut key.value {
                            *v = (*v * k).clamp(LIMITS.0, LIMITS.1);
                        }
                    }
                }
            }
        }
        *it = next;
        changed += 1;
    }
    Ok(changed)
}

/// Set (`Some(percent)`) or remove (`None`) the ramp key at media time `media` on `clips`. A clip
/// without a ramp gets a flat one first; removing the key that leaves fewer than two removes the
/// ramp. The keys of a clip and its linked sound stay the same, so they stay in sync.
pub fn edit_key(q: &mut Sequence, clips: &[ClipId], media: Tick, value: Option<f64>) -> Result<()> {
    if let Some(v) = value
        && !v.is_finite()
    {
        return Err(EditError::Other("a speed is a number".into()));
    }
    for &c in clips {
        let has_ramp = q.find_item(c).ok_or(EditError::NoItem(c))?.1.speed_ramp().is_some();
        if value.is_some() && !has_ramp {
            apply(q, &[c], &[(0.0, 100.0), (1.0, 100.0)], false)?;
        }
        let (_, it) = q.find_item_mut(c).ok_or(EditError::NoItem(c))?;
        let Some(param) = it.effect_mut("time_remap").and_then(|e| e.params.get_mut("speed")) else { continue };
        match value {
            Some(v) => param.put_keyframe(media, ParamValue::Float(v.abs().clamp(LIMITS.0, LIMITS.1))),
            None => {
                param.remove_keyframe_at(media);
                if param.keyframes.len() < 2 {
                    apply(q, &[c], &[], false)?;
                }
            }
        }
    }
    Ok(())
}
