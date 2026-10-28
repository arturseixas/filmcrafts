//! Exports on Android: where they go by default, and getting a finished one out of the app's own
//! folder (which other apps and the Gallery cannot see) into Movies.
//!
//! With "All files access" an export goes straight to `Movies/FilmCraft`. Without it, it is written
//! to the app's `Exports` folder, and when the job finishes the file is copied into the shared
//! Movies, Music or Pictures folder through MediaStore (no permission needed) and the original is
//! deleted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use filmcraft_engine::Session;

/// The folder exports go to by default: `<shared storage>/Movies/FilmCraft` with storage access,
/// else `<app folder>/Exports`.
pub fn export_dir(has_access: bool, shared_root: Option<&Path>, app_dir: Option<&Path>) -> Option<PathBuf> {
    match (has_access, shared_root, app_dir) {
        (true, Some(root), _) => Some(root.join("Movies").join("FilmCraft")),
        (_, _, Some(app)) => Some(app.join("Exports")),
        (_, Some(root), None) => Some(root.join("Movies").join("FilmCraft")),
        _ => None,
    }
}

/// Whether `path` is inside one of the app's own folders, which no other app can read.
pub fn is_private(path: &Path, private_roots: &[PathBuf]) -> bool {
    private_roots.iter().any(|root| path.starts_with(root))
}

/// Whether Android's media collections take a file with this name.
pub fn publishable(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    filmcraft_media::VIDEO_EXTENSIONS
        .iter()
        .chain(filmcraft_media::AUDIO_EXTENSIONS)
        .chain(filmcraft_media::STILL_EXTENSIONS)
        .any(|e| e.eq_ignore_ascii_case(&ext))
}

/// Finds the exports that have finished since the last look.
#[derive(Default)]
pub struct Watcher {
    seen: HashSet<u64>,
}

impl Watcher {
    /// The files of the export jobs that finished successfully since the last call. Preview
    /// renders ("Rendering …") are not exports.
    pub fn finished(&mut self, s: &Session) -> Vec<String> {
        let mut out = Vec::new();
        for job in &s.jobs {
            if job.label.starts_with("Rendering") || self.seen.contains(&job.id) {
                continue;
            }
            let result = job.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            match result {
                Some(Ok(report)) => {
                    self.seen.insert(job.id);
                    out.push(report.path);
                }
                // a failed or cancelled export has nothing to move
                Some(Err(_)) => {
                    self.seen.insert(job.id);
                }
                None => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_go_to_movies_with_access_and_to_the_app_folder_without() {
        let root = Path::new("/storage/emulated/0");
        let app = Path::new("/storage/emulated/0/Android/data/ai.storyteller.filmcraft/files");
        assert_eq!(export_dir(true, Some(root), Some(app)), Some(root.join("Movies/FilmCraft")));
        assert_eq!(export_dir(false, Some(root), Some(app)), Some(app.join("Exports")));
        assert_eq!(export_dir(false, None, None), None);
    }

    #[test]
    fn only_files_in_the_apps_own_folders_are_moved() {
        let roots = vec![
            PathBuf::from("/data/user/0/ai.storyteller.filmcraft/files"),
            PathBuf::from("/storage/emulated/0/Android/data/ai.storyteller.filmcraft/files"),
        ];
        assert!(is_private(Path::new("/storage/emulated/0/Android/data/ai.storyteller.filmcraft/files/Exports/a.mp4"), &roots));
        assert!(is_private(Path::new("/data/user/0/ai.storyteller.filmcraft/files/x.mp4"), &roots));
        assert!(!is_private(Path::new("/storage/emulated/0/Movies/FilmCraft/a.mp4"), &roots));
        assert!(!is_private(Path::new("/storage/emulated/0/Android/data/other.app/files/a.mp4"), &roots));
        assert!(publishable(Path::new("a.MP4")) && publishable(Path::new("a.wav")) && publishable(Path::new("a.png")));
        assert!(!publishable(Path::new("a.fcproj")) && !publishable(Path::new("noext")));
    }

    #[test]
    fn the_watcher_reports_each_finished_export_once() {
        let mut s = Session::default();
        s.execute("file.newProject", serde_json::json!({"name": "t"})).unwrap();
        s.execute("file.importDemoFootage", serde_json::json!({"scene": "OceanSunset"})).unwrap();
        let item = s.project.items.keys().copied().next().unwrap();
        s.state.project_selection = vec![item];
        s.execute("file.newSequenceFromClip", serde_json::json!({"items": [item.0]})).unwrap();
        let dir = std::env::temp_dir().join(format!("fc-android-exports-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.mp4");
        let mut w = Watcher::default();
        assert!(w.finished(&s).is_empty());
        s.execute("file.exportMedia", serde_json::json!({"path": out.to_string_lossy(), "wait": true})).unwrap();
        let done = w.finished(&s);
        assert_eq!(done.len(), 1, "{done:?}");
        assert!(done[0].ends_with("t.mp4"));
        assert!(w.finished(&s).is_empty(), "reported once");
        std::fs::remove_dir_all(&dir).ok();
    }
}
