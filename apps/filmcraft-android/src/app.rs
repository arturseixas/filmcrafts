//! The Android app proper: `android_main`, the eframe app around the shared UI ([`AndroidShell`])
//! and the app creator.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::TimeZone;
use filmcraft_engine::Session;
use filmcraft_engine::autosave::AutosaveConfig;
use filmcraft_platform::android::jni;
use filmcraft_ui_egui::FilmcraftApp;
use serde_json::json;
use winit::platform::android::activity::AndroidApp;

/// The entry point android-activity calls on its own thread once the activity is created (and
/// again if Android recreates the activity in the same process).
#[allow(unsafe_code)] // `no_mangle`: android-activity looks this symbol up by name; nothing unsafe runs.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    // SAFETY: both pointers come from `app`, which lives until this function returns, and a global
    // reference to the activity is what `activity_as_ptr` returns.
    unsafe { jni::set_activity(app.vm_as_ptr(), app.activity_as_ptr()) };
    let logger = crate::log_sink::install();
    let files = app.internal_data_path();
    // Android sets no HOME or TMPDIR an app can write to (std::env::temp_dir is /data/local/tmp).
    if let Some(f) = &files {
        jni::set_env("HOME", &f.to_string_lossy());
        if let Some(cache) = f.parent().map(|p| p.join("cache")) {
            let _ = std::fs::create_dir_all(&cache);
            jni::set_env("TMPDIR", &cache.to_string_lossy());
        }
        jni::set_env("FILMCRAFT_DATA_DIR", &f.to_string_lossy());
    }
    let data_dir: Option<PathBuf> = files.clone();
    let log_dir = data_dir.as_ref().map(|d| d.join(crate::logging::LOG_DIR));
    log::info!("FilmCraft {} (Android, API {}, {})", env!("CARGO_PKG_VERSION"), jni::sdk_int().unwrap_or(0), std::env::consts::ARCH);
    if let (Some(logger), Some(dir)) = (logger, log_dir.as_deref()) {
        match logger.attach_dir(dir) {
            Ok(path) => log::info!("log file {}", path.display()),
            Err(e) => log::warn!("no log file: {e}"),
        }
    }
    // Panics are logged to logcat, then (crash::install) to <data dir>/Logs/crash-<day>.log; the
    // UI pass and frame workers catch them and keep running.
    install_logcat_panic_hook();
    filmcraft_ui_egui::crash::install(log_dir);
    let hardware = filmcraft_platform::register();
    log::info!("hardware decoding: {hardware:?}");
    let mut graphics = crate::gfx::GraphicsStart::begin(data_dir.as_deref());
    if graphics.mode() == crate::gfx::Mode::Default {
        register_gpu_frame_renderer();
    }
    loop {
        let mut options = eframe::NativeOptions { android_app: Some(app.clone()), ..Default::default() };
        graphics.apply(&mut options);
        let created = Arc::new(AtomicBool::new(false));
        let result = eframe::run_native("FilmCraft", options, app_creator(data_dir.clone(), graphics.clone(), Arc::clone(&created)));
        match &result {
            // wgpu reported the failure before the app existed: try OpenGL ES in this process
            Err(eframe::Error::Wgpu(e)) if !created.load(Ordering::Relaxed) && graphics.step_down() => {
                log::warn!("graphics start failed: {e}");
                continue;
            }
            Err(e) => {
                let msg = format!("FilmCraft could not start: {e}");
                log::error!("{msg}");
                filmcraft_ui_egui::crash::record(&msg);
            }
            Ok(()) => {}
        }
        break;
    }
}

fn install_logcat_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}");
            prev(info);
        }));
    });
}

/// The shared UI plus what Android adds around it each frame: the file browser, touch gestures,
/// the soft keyboard, the clipboard, Back, keeping the screen on while playing, and the interface
/// zoom on phones.
struct AndroidShell {
    app: FilmcraftApp,
    browser: crate::file_browser::Browser,
    gestures: crate::touch::Gestures,
    chrome: crate::chrome::Chrome,
    /// Events at the front of the raw input that this hook already rewrote: a logic-only pass
    /// leaves its input to the next pass, which offers it to the hook again ahead of new input.
    offered: usize,
    keyboard: bool,
    screen_on: bool,
    exports: crate::exports::Watcher,
    /// When the default export folder was last worked out (it follows the storage permission).
    export_dir_checked: Option<std::time::Instant>,
    export_dir: Option<PathBuf>,
}

impl AndroidShell {
    /// Keep the default export folder in step with the storage permission, and move a finished
    /// export out of the app's own folder into Movies (see [`crate::exports`]).
    fn follow_exports(&mut self, ctx: &egui::Context) {
        use std::time::{Duration, Instant};
        if self.export_dir_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.export_dir_checked = Some(Instant::now());
            let roots = jni::storage_roots();
            let app_dir = jni::app_storage_dir().map(PathBuf::from);
            let dir = crate::exports::export_dir(jni::has_storage_access(), roots.first().map(Path::new), app_dir.as_deref());
            if dir != self.export_dir {
                if let Some(d) = &dir {
                    let _ = std::fs::create_dir_all(d);
                }
                // the Export screen shows the folder it started with: follow the default
                let old = self.export_dir.as_ref().map(|d| d.to_string_lossy().into_owned());
                if old.is_some() && old.as_deref() == Some(self.app.ui.export.location.as_str()) {
                    self.app.ui.export.location = dir.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                }
                filmcraft_engine::export_tools::set_default_export_dir(dir.clone());
                self.export_dir = dir;
            }
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        let mut private: Vec<PathBuf> = jni::app_storage_dir().map(PathBuf::from).into_iter().collect();
        if let Some(home) = std::env::var_os("HOME") {
            private.push(PathBuf::from(home));
        }
        for path in self.exports.finished(&self.app.session) {
            let p = Path::new(&path);
            if crate::exports::is_private(p, &private) && crate::exports::publishable(p) {
                match jni::publish_to_gallery(&path) {
                    Ok(()) => self.chrome.bar.notify(ctx, filmcraft_ui_egui::i18n::t("Export finished: saving it to your Gallery…"), false),
                    Err(e) => self.chrome.bar.notify(ctx, e, true),
                }
            } else {
                self.chrome.bar.notify(ctx, format!("{} {path}", filmcraft_ui_egui::i18n::t("Export finished:")), false);
            }
        }
        match jni::take_publish_result() {
            Some(Ok(at)) => self.chrome.bar.notify(ctx, format!("{} {at}", filmcraft_ui_egui::i18n::t("Saved to")), false),
            Some(Err(e)) => self.chrome.bar.notify(ctx, e, true),
            None => {}
        }
    }
}

impl eframe::App for AndroidShell {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let offered = self.offered.min(raw_input.events.len());
        let mut events = raw_input.events.split_off(offered);
        for text in jni::take_typed_text() {
            events.push(egui::Event::Text(text));
        }
        let mut back = false;
        events.retain_mut(|e| match e {
            // Back closes the file browser, else acts as Escape (dialogs, menus, full screen)
            egui::Event::Key { key: egui::Key::BrowserBack, pressed, .. } => {
                if *pressed {
                    back = true;
                }
                false
            }
            // Paste from a hardware keyboard: the system clipboard, not egui's in-app one
            egui::Event::Paste(text) => {
                if let Some(t) = jni::clipboard_text() {
                    *text = t;
                }
                true
            }
            _ => true,
        });
        if back && !self.browser.cancel() {
            for pressed in [true, false] {
                events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
            }
        }
        if let Some(wait) = self.gestures.process(&mut events, std::time::Instant::now()) {
            ctx.request_repaint_after(wait);
        }
        if self.gestures.pending() {
            ctx.request_repaint();
        }
        raw_input.events.append(&mut events);
        self.app.raw_input_hook(ctx, raw_input);
        self.offered = raw_input.events.len();
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // this pass consumes its raw input: the next hook sees only new events
        self.offered = 0;
        self.chrome.keyboard = jni::keyboard_height_px() as f32 / ui.ctx().pixels_per_point().max(0.1);
        self.chrome.ui(&mut self.app, ui, frame);
        let ctx = ui.ctx().clone();
        self.browser.ui(&ctx);
        for cmd in ctx.output(|o| o.commands.clone()) {
            if let egui::OutputCommand::CopyText(text) = cmd {
                jni::set_clipboard_text(&text);
            }
        }
        let keyboard = ctx.egui_wants_keyboard_input();
        if keyboard != self.keyboard {
            self.keyboard = keyboard;
            jni::set_keyboard_visible(keyboard);
        }
        let screen_on = self.app.playback.playing;
        if screen_on != self.screen_on {
            self.screen_on = screen_on;
            jni::set_keep_screen_on(screen_on);
        }
        self.follow_exports(&ctx);
    }

    fn on_exit(&mut self) {
        self.app.on_exit();
    }
}

/// Creates the app once eframe has its window and graphics device.
fn app_creator(data_dir: Option<PathBuf>, graphics: crate::gfx::GraphicsStart, created: Arc<AtomicBool>) -> eframe::AppCreator<'static> {
    Box::new(move |cc| {
        filmcraft_ui_egui::install_ime_reset_callbacks(&cc.egui_ctx);
        created.store(true, Ordering::Relaxed);
        let ctx = cc.egui_ctx.clone();
        jni::set_wake(Box::new(move || ctx.request_repaint()));
        let notice = (graphics.mode() != crate::gfx::Mode::Default).then(|| {
            let start = graphics.clone();
            filmcraft_ui_egui::graphics_notice::GraphicsNotice::new(graphics.mode().label(), Box::new(move || start.reset_to_default()))
        });
        let confirmed = AtomicBool::new(false);
        let start = graphics.clone();
        cc.egui_ctx.on_end_pass(
            "graphics-start",
            Arc::new(move |ui| {
                if ui.ctx().cumulative_frame_nr() >= 2 && !confirmed.swap(true, Ordering::Relaxed) {
                    start.confirm();
                }
            }),
        );
        let mut session = Session::default();
        if let Some(dir) = data_dir.clone() {
            let mut cfg = AutosaveConfig::new(dir);
            cfg.local_offset = local_offset;
            if let Err(e) = session.start_autosave(cfg) {
                log::warn!("auto-save and crash recovery unavailable: {e}");
            }
        }
        session.voiceover.input = Some(Box::new(crate::mic::MicIn(crate::audio_in::CpalIn::new(&session.prefs.audio_hardware.device_class))));
        // Settings ▸ General ▸ At Startup
        match session.prefs.general.at_startup.as_str() {
            "openMostRecent" => {
                let recent = session.prefs.general.recent_projects.iter().find(|p| std::path::Path::new(p).exists()).cloned();
                let opened = recent.map(|p| session.execute("file.open", json!({"path": p})));
                if !matches!(opened, Some(Ok(_))) {
                    if let Some(Err(e)) = opened {
                        log::error!("{e}");
                    }
                    let _ = session.execute("file.openDemoProject", json!({}));
                }
            }
            "emptyProject" => {}
            _ => {
                let _ = session.execute("file.openDemoProject", json!({}));
            }
        }
        let mut app = FilmcraftApp::new(session);
        app.graphics_notice = notice;
        if let Some(rs) = cc.wgpu_render_state.clone() {
            log::info!("graphics: {:?}", rs.adapter.get_info());
            app.set_wgpu(rs);
        }
        app.audio = Some(Box::new(crate::audio::CpalOut::new()));
        let browser = crate::file_browser::Browser::default();
        app.hooks.run_dialog = Some(browser.hook());
        app.hooks.open_url = Some(Box::new(jni::open_url));
        app.hooks.system_languages = Some(Box::new(jni::system_languages));
        Ok(Box::new(AndroidShell {
            app,
            browser,
            gestures: crate::touch::Gestures::default(),
            chrome: crate::chrome::Chrome::default(),
            offered: 0,
            keyboard: false,
            screen_on: false,
            exports: Default::default(),
            export_dir_checked: None,
            export_dir: None,
        }))
    })
}

/// Local UTC offset at a unix time (auto-save file names and recovery times use local time).
fn local_offset(unix: i64) -> i32 {
    chrono::Local.timestamp_opt(unix, 0).single().map(|d| d.offset().local_minus_utc()).unwrap_or(0)
}

/// The GPU export frame renderer (as in the desktop app): exports composite on the GPU where they
/// can and fall back to the CPU reference renderer where they cannot.
struct GpuFrameRenderer(filmcraft_gpu::ExportRenderer);

impl filmcraft_export::FrameRenderer for GpuFrameRenderer {
    fn render(
        &mut self,
        project: &filmcraft_project::Project,
        seq: filmcraft_project::ItemId,
        t: filmcraft_time::Tick,
        opts: filmcraft_render::RenderOptions,
        sources: &dyn filmcraft_render::SourceProvider,
    ) -> Option<filmcraft_render::Image> {
        self.0.render(project, seq, t, opts, sources)
    }
}

fn register_gpu_frame_renderer() {
    filmcraft_export::register_frame_renderer(|| {
        filmcraft_gpu::ExportRenderer::new().map(|r| Box::new(GpuFrameRenderer(r)) as Box<dyn filmcraft_export::FrameRenderer>)
    });
}
