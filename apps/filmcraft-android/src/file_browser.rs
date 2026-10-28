//! The file dialogs on Android: a browser of shared storage drawn with egui, behind
//! [`filmcraft_ui_egui::HostHooks::run_dialog`] (the non-blocking dialog hook the desktop app uses
//! on Linux), so every Open, Import, Save, Export and folder dialog of the shared UI lands here.
//!
//! FilmCraft opens media by path, so the browser works on paths too: with "All files access" (or
//! the storage permissions before Android 11) that is all of shared storage, SD cards and USB
//! drives; with media access only, the videos, pictures and music in it; and always this app's own
//! folder in shared storage. Reading by path keeps imports zero-copy: a camera clip is decoded
//! where it is, never copied into the app first.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use filmcraft_platform::android::jni;
use filmcraft_ui_egui::i18n::t;
use filmcraft_ui_egui::native_dialogs::FileDialog;

/// Folder entries listed at most (a folder of camera frames can hold more).
const MAX_ENTRIES: usize = 10_000;
/// How often the storage permissions are checked again while the browser is open.
const ACCESS_POLL: Duration = Duration::from_millis(800);
/// Two taps on the same file within this open it.
const DOUBLE_TAP: Duration = Duration::from_millis(450);
/// Video containers: saved under Movies rather than Documents.
const VIDEO_EXTS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "mxf", "avi", "ts", "mts", "m2ts", "gif"];

/// Where a dialog of each kind last was, for the next one (this session).
#[derive(Default)]
struct Remembered {
    media: Option<PathBuf>,
    project: Option<PathBuf>,
    export: Option<PathBuf>,
    folder: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Media,
    Project,
    Export,
    Folder,
}

#[derive(Clone, PartialEq, Eq)]
enum Mode {
    Open { multiple: bool },
    Save,
    Folder,
}

struct Entry {
    path: PathBuf,
    name: String,
    dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

/// An open dialog.
struct View {
    mode: Mode,
    purpose: Purpose,
    title: String,
    /// Lower-case extensions to show (empty: every file).
    exts: Vec<String>,
    dir: PathBuf,
    entries: Vec<Entry>,
    error: Option<String>,
    selected: Vec<PathBuf>,
    /// Save: the file name.
    name: String,
    show_all: bool,
    by_date: bool,
    /// Save: the existing file the user is asked about replacing.
    replace: Option<PathBuf>,
    answer: Sender<Vec<String>>,
    access: (bool, bool),
    access_checked: Instant,
    /// The Places column (storage volumes and their usual folders), found when the dialog opens
    /// and when storage access changes rather than on every frame (each is a JNI call and stats).
    places: Vec<(String, PathBuf)>,
    last_tap: Option<(PathBuf, Instant)>,
    /// The dialog imports media: it offers the Gallery and Files pickers of the system.
    can_pick_media: bool,
    /// The system picker is open or its picks are being copied in; the dialog answers with the
    /// copies when they are done.
    waiting_import: bool,
}

/// The browser: [`Browser::hook`] receives dialogs from the UI, [`Browser::ui`] draws the open one.
#[derive(Default)]
pub struct Browser {
    incoming: Rc<RefCell<Option<(FileDialog, Sender<Vec<String>>)>>>,
    view: Option<View>,
    remembered: Remembered,
}

/// Whether a dialog's extension filter is for media (or for everything), as opposed to a settings
/// or preset file: the Gallery and Files pickers only offer media.
fn looks_like_media(exts: &[String]) -> bool {
    exts.is_empty() || exts.iter().any(|e| matches!(e.as_str(), "mp4" | "mov" | "mkv" | "wav" | "mp3" | "png" | "jpg" | "jpeg"))
}

fn lower(exts: &[String]) -> Vec<String> {
    exts.iter().map(|e| e.trim_start_matches('.').to_ascii_lowercase()).filter(|e| !e.is_empty() && e != "*").collect()
}

/// The top folder of the device's shared storage, else this app's folder.
fn primary_root() -> PathBuf {
    jni::storage_roots().into_iter().next().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/storage/emulated/0"))
}

fn app_dir() -> Option<PathBuf> {
    jni::app_storage_dir().map(PathBuf::from)
}

fn first_dir(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|d| d.is_dir())
}

impl Browser {
    /// The [`filmcraft_ui_egui::HostHooks::run_dialog`] hook: queue `dialog` for the next frame and
    /// answer on the returned channel (no paths: cancelled).
    pub fn hook(&self) -> Box<dyn FnMut(FileDialog) -> Receiver<Vec<String>>> {
        let incoming = Rc::clone(&self.incoming);
        Box::new(move |dialog| {
            let (tx, rx) = channel();
            // one dialog at a time: a newer request replaces (and so cancels) a queued one
            *incoming.borrow_mut() = Some((dialog, tx));
            rx
        })
    }

    pub fn is_open(&self) -> bool {
        self.view.is_some() || self.incoming.borrow().is_some()
    }

    /// Escape / Back: cancel the open dialog. Returns whether one was open.
    pub fn cancel(&mut self) -> bool {
        match self.view.take() {
            Some(v) => {
                if v.waiting_import {
                    jni::cancel_import();
                }
                let _ = v.answer.send(Vec::new());
                true
            }
            None => false,
        }
    }

    fn open(&mut self, dialog: FileDialog, answer: Sender<Vec<String>>) {
        if let Some(old) = self.view.take() {
            if old.waiting_import {
                jni::cancel_import();
            }
            let _ = old.answer.send(Vec::new());
        }
        let access = (jni::has_storage_access(), jni::has_media_access());
        let (mode, purpose, title, exts, name, at) = match dialog {
            FileDialog::OpenFiles { exts } => (Mode::Open { multiple: true }, Purpose::Media, t("Import Media").to_string(), lower(&exts), String::new(), None),
            FileDialog::OpenFile { filter, exts } => {
                (Mode::Open { multiple: false }, Purpose::Media, t(&filter).to_string(), lower(&exts), String::new(), None)
            }
            FileDialog::Relink { exts, .. } => {
                (Mode::Open { multiple: false }, Purpose::Media, t("Locate Media").to_string(), lower(&exts), String::new(), None)
            }
            FileDialog::OpenProject => {
                (Mode::Open { multiple: false }, Purpose::Project, t("Open Project").to_string(), vec!["fcproj".into()], String::new(), None)
            }
            FileDialog::SaveProject { name } => (Mode::Save, Purpose::Project, t("Save Project").to_string(), vec!["fcproj".into()], name, None),
            FileDialog::SaveAs { filter, exts, name } => {
                let exts = lower(&exts);
                let video = exts.first().is_some_and(|e| VIDEO_EXTS.contains(&e.as_str()));
                // the suggestion may be a full path: start in its folder
                let p = Path::new(&name);
                let at = p.parent().filter(|d| d.is_absolute() && d.is_dir()).map(Path::to_path_buf);
                let file = p.file_name().map_or(name.clone(), |n| n.to_string_lossy().into_owned());
                (Mode::Save, if video { Purpose::Export } else { Purpose::Project }, t(&filter).to_string(), exts, file, at)
            }
            FileDialog::Folder { at } => {
                (Mode::Folder, Purpose::Folder, t("Choose Folder").to_string(), Vec::new(), String::new(), at.filter(|d| !d.is_empty()).map(PathBuf::from))
            }
        };
        let dir = at.filter(|d| d.is_dir()).or_else(|| self.remembered_dir(purpose)).unwrap_or_else(|| default_dir(purpose, access.0 || access.1));
        let can_pick_media = matches!(mode, Mode::Open { .. }) && purpose == Purpose::Media && looks_like_media(&exts);
        let mut view = View {
            mode,
            purpose,
            title,
            exts,
            dir,
            entries: Vec::new(),
            error: None,
            selected: Vec::new(),
            name,
            show_all: false,
            by_date: purpose == Purpose::Media,
            replace: None,
            answer,
            access,
            access_checked: Instant::now(),
            places: places(),
            last_tap: None,
            can_pick_media,
            waiting_import: false,
        };
        view.refresh();
        self.view = Some(view);
    }

    fn remembered_dir(&self, purpose: Purpose) -> Option<PathBuf> {
        let r = &self.remembered;
        match purpose {
            Purpose::Media => r.media.clone(),
            Purpose::Project => r.project.clone(),
            Purpose::Export => r.export.clone(),
            Purpose::Folder => r.folder.clone(),
        }
        .filter(|d| d.is_dir())
    }

    fn remember(&mut self, purpose: Purpose, dir: PathBuf) {
        let r = &mut self.remembered;
        match purpose {
            Purpose::Media => r.media = Some(dir),
            Purpose::Project => r.project = Some(dir),
            Purpose::Export => r.export = Some(dir),
            Purpose::Folder => r.folder = Some(dir),
        }
    }

    /// Draw the open dialog (after the app's UI, on top of it).
    pub fn ui(&mut self, ctx: &egui::Context) {
        let incoming = self.incoming.borrow_mut().take();
        if let Some((dialog, answer)) = incoming {
            self.open(dialog, answer);
        }
        let Some(view) = self.view.as_mut() else { return };
        if view.access_checked.elapsed() > ACCESS_POLL {
            view.access_checked = Instant::now();
            let access = (jni::has_storage_access(), jni::has_media_access());
            if access != view.access {
                // permission granted (or revoked) in the system page: show what is now readable
                view.access = access;
                if !access.0 && !access.1 {
                    view.dir = default_dir(view.purpose, false);
                }
                view.places = places();
                view.refresh();
            }
        }
        ctx.request_repaint_after(ACCESS_POLL);
        let mut done: Option<Vec<String>> = None;
        if view.waiting_import
            && let Some(result) = jni::take_import_result()
        {
            view.waiting_import = false;
            match result {
                // the copies are imported like files chosen in the browser
                Ok(paths) if !paths.is_empty() => done = Some(paths),
                // the picker was dismissed: back to the browser
                Ok(_) => {}
                Err(e) => view.error = Some(e),
            }
        }
        let screen = ctx.content_rect();
        let size = egui::vec2((screen.width() * 0.92).min(1100.0), (screen.height() * 0.92).min(800.0));
        let modal = egui::Modal::new(egui::Id::unique("android-file-browser")).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            if let Some(d) = view.contents(ui) {
                done = Some(d);
            }
        });
        if modal.should_close() && done.is_none() {
            done = Some(Vec::new());
        }
        if let Some(paths) = done
            && let Some(v) = self.view.take()
        {
            let (purpose, dir) = (v.purpose, v.dir.clone());
            if v.waiting_import {
                jni::cancel_import();
            }
            let _ = v.answer.send(paths);
            self.remember(purpose, dir);
        }
    }
}

/// Where a dialog for `purpose` starts the first time.
fn default_dir(purpose: Purpose, shared: bool) -> PathBuf {
    let app = app_dir();
    if shared {
        let root = primary_root();
        let found = match purpose {
            Purpose::Media => first_dir([root.join("DCIM/Camera"), root.join("Movies"), root.join("DCIM")]),
            Purpose::Project => first_dir([root.join("Documents/FilmCraft"), root.join("Documents")]),
            Purpose::Export => first_dir([root.join("Movies/FilmCraft"), root.join("Movies")]),
            Purpose::Folder => None,
        };
        return found.unwrap_or(root);
    }
    let sub = match purpose {
        Purpose::Media => "Media",
        Purpose::Project => "Projects",
        Purpose::Export => "Exports",
        Purpose::Folder => "",
    };
    match app {
        Some(a) => {
            let d = if sub.is_empty() { a } else { a.join(sub) };
            // the app's own folder is always writable
            let _ = std::fs::create_dir_all(&d);
            d
        }
        None => primary_root(),
    }
}

/// The Places column: each storage volume (the internal one with its usual folders) and this
/// app's folder.
fn places() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let roots = jni::storage_roots();
    for (i, r) in roots.iter().enumerate() {
        let root = PathBuf::from(r);
        if i == 0 {
            out.push((t("Internal storage").to_string(), root.clone()));
            for (label, sub) in [
                ("Camera", "DCIM/Camera"),
                ("Movies", "Movies"),
                ("Download", "Download"),
                ("Documents", "Documents"),
                ("Music", "Music"),
                ("Pictures", "Pictures"),
            ] {
                let d = root.join(sub);
                if d.is_dir() {
                    out.push((format!("  {}", t(label)), d));
                }
            }
        } else {
            let name = root.file_name().map_or_else(|| r.clone(), |n| n.to_string_lossy().into_owned());
            out.push((format!("{} ({name})", t("Storage")), root));
        }
    }
    if let Some(a) = app_dir() {
        out.push((t("FilmCraft folder").to_string(), a));
    }
    out
}

fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

fn date(t: Option<SystemTime>) -> String {
    t.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

impl View {
    /// The system picker is open, then its picks are copied into the app: show how far along.
    fn import_progress(&mut self, ui: &mut egui::Ui, row_h: f32) {
        ui.add_space(row_h);
        ui.vertical_centered(|ui| match jni::import_progress() {
            Some(p) => {
                let which = if p.count > 1 { format!("{} {} / {}", t("Copying"), p.index.saturating_add(1), p.count) } else { t("Copying").to_string() };
                ui.label(format!("{which}: {}", p.name));
                let fraction = if p.total > 0 { (p.done as f64 / p.total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
                let text = if p.total > 0 { format!("{} / {}", human_size(p.done), human_size(p.total)) } else { human_size(p.done) };
                let bar = egui::ProgressBar::new(fraction).text(text).animate(p.total == 0);
                ui.add_sized([ui.available_width().min(520.0), row_h], bar);
            }
            None => {
                ui.label(t("Waiting for your choice in the picker…"));
                ui.spinner();
            }
        });
        ui.add_space(row_h * 0.5);
        ui.vertical_centered(|ui| {
            if ui.add(egui::Button::new(t("Cancel")).min_size(egui::vec2(140.0, row_h))).clicked() {
                // the copy stops, deletes its partial file and reports back; the browser returns
                jni::cancel_import();
            }
        });
    }

    fn matches(&self, name: &str) -> bool {
        if self.show_all || self.exts.is_empty() {
            return true;
        }
        let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        self.exts.contains(&ext)
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.error = None;
        self.replace = None;
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) => {
                self.error = Some(if e.kind() == std::io::ErrorKind::PermissionDenied {
                    t("This folder can't be read without storage access").to_string()
                } else {
                    format!("{}: {e}", self.dir.display())
                });
                return;
            }
        };
        for e in rd.flatten().take(MAX_ENTRIES) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !self.show_all {
                continue;
            }
            let path = e.path();
            // follows symlinks: a linked folder is a folder
            let meta = std::fs::metadata(&path).ok();
            let dir = meta.as_ref().is_some_and(|m| m.is_dir());
            if !dir && (self.mode == Mode::Folder || !self.matches(&name)) {
                continue;
            }
            self.entries.push(Entry { path, name, dir, size: meta.as_ref().map_or(0, |m| m.len()), modified: meta.and_then(|m| m.modified().ok()) });
        }
        self.sort();
    }

    fn sort(&mut self) {
        let by_date = self.by_date;
        self.entries.sort_by(|a, b| {
            b.dir.cmp(&a.dir).then_with(|| if by_date && !a.dir { b.modified.cmp(&a.modified) } else { a.name.to_lowercase().cmp(&b.name.to_lowercase()) })
        });
    }

    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.selected.clear();
        self.refresh();
    }

    /// The path a Save writes to: the typed name in this folder, with the dialog's extension.
    fn save_path(&self) -> Option<PathBuf> {
        let name = self.name.trim();
        if name.is_empty() || name.contains('/') {
            return None;
        }
        let mut file = name.to_string();
        let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
        if let Some(first) = self.exts.first()
            && !ext.is_some_and(|e| self.exts.contains(&e))
        {
            file = format!("{file}.{first}");
        }
        Some(self.dir.join(file))
    }

    /// Draw the dialog; `Some(paths)` when it is done (empty: cancelled).
    fn contents(&mut self, ui: &mut egui::Ui) -> Option<Vec<String>> {
        let mut done = None;
        let row_h = 34.0;
        ui.horizontal(|ui| {
            ui.heading(&self.title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(t("Cancel")).min_size(egui::vec2(90.0, row_h))).clicked() {
                    done = Some(Vec::new());
                }
            });
        });
        if self.waiting_import {
            self.import_progress(ui, row_h);
            return done;
        }
        if self.can_pick_media {
            ui.horizontal_wrapped(|ui| {
                let size = egui::vec2(150.0, row_h);
                let multiple = self.mode == Mode::Open { multiple: true };
                for (gallery, label) in [(true, t("Gallery")), (false, t("Files app"))] {
                    if ui.add(egui::Button::new(label).min_size(size)).clicked() {
                        match jni::pick_media(gallery, multiple) {
                            Ok(()) => self.waiting_import = true,
                            Err(e) => self.error = Some(e),
                        }
                    }
                }
                ui.weak(t("Pick videos and photos from your Gallery or the Files app (cloud drives, SD cards): no storage access needed"));
            });
            ui.separator();
        }
        if !self.access.0 {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(if self.mode == Mode::Save {
                        t("Allow FilmCraft to save to Movies and Documents. Until then it saves inside its own folder, which other apps can't see.")
                    } else if self.access.1 {
                        t("FilmCraft can read your videos, pictures and music. Allow access to all files to open projects and save anywhere.")
                    } else {
                        t("Allow FilmCraft to read your files to import media and open projects. Until then it uses its own folder.")
                    });
                    if ui.add(egui::Button::new(t("Allow access to all files")).min_size(egui::vec2(0.0, row_h))).clicked() {
                        jni::request_storage_access();
                    }
                    if !self.access.1 && ui.add(egui::Button::new(t("Media only")).min_size(egui::vec2(0.0, row_h))).clicked() {
                        jni::request_media_access();
                    }
                });
            });
        }
        ui.horizontal(|ui| {
            let up = self.dir.parent().map(Path::to_path_buf);
            if ui.add_enabled(up.is_some(), egui::Button::new("⬆").min_size(egui::vec2(row_h * 1.4, row_h))).on_hover_text(t("Up")).clicked()
                && let Some(up) = up
            {
                self.go(up);
            }
            if ui.add(egui::Button::new("⟳").min_size(egui::vec2(row_h * 1.4, row_h))).on_hover_text(t("Refresh")).clicked() {
                self.refresh();
            }
            ui.add(egui::Label::new(egui::RichText::new(self.dir.to_string_lossy()).monospace()).truncate());
        });
        ui.separator();
        let footer_h = row_h * 2.4;
        let body_h = (ui.available_height() - footer_h).max(row_h * 3.0);
        ui.horizontal(|ui| {
            ui.set_height(body_h);
            // places
            ui.vertical(|ui| {
                ui.set_width(ui.available_width().min(220.0));
                egui::ScrollArea::vertical().id_salt("places").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                    let mut go = None;
                    for (label, path) in &self.places {
                        let here = self.dir == *path;
                        if ui.add(egui::Button::selectable(here, label.as_str()).min_size(egui::vec2(ui.available_width(), row_h))).clicked() {
                            go = Some(path.clone());
                        }
                    }
                    if let Some(path) = go {
                        self.go(path);
                    }
                });
            });
            ui.separator();
            // folder contents
            ui.vertical(|ui| {
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                }
                let mut open_dir = None;
                let mut chosen = None;
                egui::ScrollArea::vertical().id_salt("entries").max_height(body_h).auto_shrink([false, false]).show_rows(
                    ui,
                    row_h,
                    self.entries.len(),
                    |ui, rows| {
                        for i in rows {
                            let Some(e) = self.entries.get(i) else { continue };
                            let selected = self.selected.contains(&e.path);
                            let icon = if e.dir { "📁" } else { "🎞" };
                            let right = if e.dir { date(e.modified) } else { format!("{}   {}", human_size(e.size), date(e.modified)) };
                            let button = egui::Button::selectable(selected, format!("{icon}  {}", e.name))
                                .right_text(right)
                                .min_size(egui::vec2(ui.available_width(), row_h - 2.0));
                            if ui.add(button).clicked() {
                                if e.dir {
                                    open_dir = Some(e.path.clone());
                                } else {
                                    let double = self.last_tap.as_ref().is_some_and(|(p, at)| *p == e.path && at.elapsed() < DOUBLE_TAP);
                                    self.last_tap = Some((e.path.clone(), Instant::now()));
                                    chosen = Some((e.path.clone(), e.name.clone(), double));
                                }
                            }
                        }
                    },
                );
                if let Some(d) = open_dir {
                    self.go(d);
                }
                if let Some((path, name, double)) = chosen {
                    match self.mode {
                        Mode::Open { multiple: true } => {
                            if let Some(i) = self.selected.iter().position(|p| *p == path) {
                                self.selected.remove(i);
                            } else {
                                self.selected.push(path);
                            }
                        }
                        Mode::Open { multiple: false } => {
                            if double {
                                done = Some(vec![path.to_string_lossy().into_owned()]);
                            }
                            self.selected = vec![path];
                        }
                        Mode::Save => {
                            self.name = name;
                            self.replace = None;
                        }
                        Mode::Folder => {}
                    }
                }
                if self.entries.is_empty() && self.error.is_none() {
                    ui.weak(t("No matching files in this folder"));
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            let size = egui::vec2(140.0, row_h);
            if self.mode != Mode::Folder && !self.exts.is_empty() && ui.checkbox(&mut self.show_all, t("Show all files")).changed() {
                self.refresh();
            }
            if ui.checkbox(&mut self.by_date, t("Newest first")).changed() {
                self.sort();
            }
            match self.mode {
                Mode::Open { multiple } => {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let n = self.selected.len();
                        let label = if multiple && n > 1 {
                            format!("{} ({n})", t("Import"))
                        } else if multiple {
                            t("Import").to_string()
                        } else {
                            t("Open").to_string()
                        };
                        if ui.add_enabled(n > 0, egui::Button::new(label).min_size(size)).clicked() {
                            done = Some(self.selected.iter().map(|p| p.to_string_lossy().into_owned()).collect());
                        }
                        if multiple && ui.add(egui::Button::new(t("Select all")).min_size(size)).clicked() {
                            self.selected = self.entries.iter().filter(|e| !e.dir).map(|e| e.path.clone()).collect();
                        }
                    });
                }
                Mode::Folder => {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(t("Choose this folder")).min_size(size)).clicked() {
                            done = Some(vec![self.dir.to_string_lossy().into_owned()]);
                        }
                    });
                }
                Mode::Save => {
                    ui.label(t("Name:"));
                    let save = self.save_path();
                    let replacing = self.replace.is_some() && self.replace == save;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if replacing { t("Replace") } else { t("Save") };
                        if ui.add_enabled(save.is_some(), egui::Button::new(label).min_size(size)).clicked()
                            && let Some(path) = save.clone()
                        {
                            if path.exists() && !replacing {
                                self.replace = Some(path);
                            } else {
                                done = Some(vec![path.to_string_lossy().into_owned()]);
                            }
                        }
                        let edit = egui::TextEdit::singleline(&mut self.name).desired_width(ui.available_width()).min_size(egui::vec2(0.0, row_h));
                        if ui.add(edit).changed() {
                            self.replace = None;
                        }
                    });
                }
            }
        });
        if let Some(p) = &self.replace {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "{} {}",
                    p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    t("already exists. Press Replace to overwrite it.")
                ),
            );
        }
        // a save into a folder that doesn't exist yet (Movies/FilmCraft): create it
        if let Some(paths) = &done
            && self.mode == Mode::Save
            && let Some(parent) = paths.first().and_then(|p| Path::new(p).parent())
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            log::warn!("{}: {e}", parent.display());
        }
        done
    }
}
