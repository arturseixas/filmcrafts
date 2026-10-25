//! FilmCraft on Android.
//!
//! The same engine and egui UI as the desktop app, built as `libfilmcraft.so` and started by
//! `FilmcraftActivity` (a `NativeActivity`, `java/`) through android-activity, winit and eframe,
//! drawing with wgpu on Vulkan (OpenGL ES where Vulkan is missing or crashed, [`gfx`]). What the
//! desktop shell gets from the OS, this crate gets from Android:
//!
//! | Desktop | Android |
//! |---|---|
//! | file dialogs (rfd) | a browser of shared storage drawn in egui ([`file_browser`]) |
//! | per-user data folder | the app's internal storage (auto-save, recovery, settings, logs) |
//! | stderr + log file | logcat + log file ([`log_sink`]) |
//! | cpal output / input | cpal on AAudio (the desktop `audio.rs` / `audio_in.rs`), microphone permission ([`mic`]) |
//! | VideoToolbox, Media Foundation, VA-API… | MediaCodec (`filmcraft_platform::android::mediacodec`) |
//! | keyboard | hardware keys through winit; the soft keyboard's text through the activity's input view |
//! | system clipboard (text) | `ClipboardManager` through JNI |
//!
//! Everything but the touch gesture translator ([`touch`], tested on every target) is
//! Android-only.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
#[path = "../../filmcraft/src/audio.rs"]
mod audio;
#[cfg(target_os = "android")]
#[path = "../../filmcraft/src/audio_in.rs"]
mod audio_in;
#[cfg(target_os = "android")]
pub mod file_browser;
#[cfg(target_os = "android")]
pub mod gfx;
#[cfg(target_os = "android")]
pub mod log_sink;
// The desktop app's log filter, rotation and file sink (its stderr logger is not used here).
#[cfg(target_os = "android")]
#[allow(dead_code)]
#[path = "../../filmcraft/src/logging.rs"]
mod logging;
#[cfg(target_os = "android")]
pub mod mic;
pub mod touch;
