//! JNI calls into the app's activity (`ai.storyteller.filmcraft.FilmcraftActivity`, a
//! `NativeActivity` subclass in `apps/filmcraft-android/java`) and the one native method it calls
//! back (`nativeText`, text from the soft keyboard).
//!
//! The activity and the Java VM come from `ndk-context`, which android-activity initialises before
//! `android_main` runs. Every call attaches the calling thread for its duration, clears a pending
//! Java exception and reports failure as `None` / `Err` (a missing method, an old activity class,
//! no activity yet): a JNI problem never takes the app down.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock, PoisonError};

use jni::objects::{JObject, JObjectArray, JString, JValue};
use jni::{JNIEnv, JavaVM};

/// Run `f` with a JNI environment for this thread and the activity object.
fn with_activity<T>(f: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>) -> Result<T, String> {
    let ctx = ndk_context::android_context();
    if ctx.vm().is_null() || ctx.context().is_null() {
        return Err("no Android activity".into());
    }
    // SAFETY: android-activity stores the process's JavaVM pointer in ndk-context before
    // android_main runs, and it stays valid for the life of the process.
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // SAFETY: ndk-context holds a global reference to the activity for the life of the process;
    // this borrowed JObject never deletes it (a plain JObject has no destructor).
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    let result = f(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.map_err(|e| e.to_string())
}

fn call_bool(name: &str) -> Option<bool> {
    with_activity(|env, a| env.call_method(a, name, "()Z", &[])?.z()).map_err(|e| log::debug!("{name}: {e}")).ok()
}

fn call_void(name: &str) {
    if let Err(e) = with_activity(|env, a| env.call_method(a, name, "()V", &[])?.v()) {
        log::warn!("{name}: {e}");
    }
}

fn call_string(name: &str) -> Option<String> {
    with_activity(|env, a| {
        let obj = env.call_method(a, name, "()Ljava/lang/String;", &[])?.l()?;
        if obj.is_null() {
            return Ok(None);
        }
        let s: String = env.get_string(&JString::from(obj))?.into();
        Ok(Some(s))
    })
    .map_err(|e| log::debug!("{name}: {e}"))
    .ok()
    .flatten()
}

/// Android's API level (`Build.VERSION.SDK_INT`).
pub fn sdk_int() -> Option<i32> {
    with_activity(|env, a| env.call_method(a, "sdkInt", "()I", &[])?.i()).ok()
}

/// Whether FilmCraft can read and write anywhere in shared storage by path (Android 11+: All
/// files access; before that, the storage permissions).
pub fn has_storage_access() -> bool {
    call_bool("hasStorageAccess").unwrap_or(false)
}

/// Whether FilmCraft can read the videos, pictures and music in shared storage by path.
pub fn has_media_access() -> bool {
    call_bool("hasMediaAccess").unwrap_or(false)
}

/// Open the system page that grants full storage access (or the permission prompt before
/// Android 11). The answer arrives later: poll [`has_storage_access`].
pub fn request_storage_access() {
    call_void("requestStorageAccess");
}

/// Ask for read access to videos, pictures and music (the system prompt).
pub fn request_media_access() {
    call_void("requestMediaAccess");
}

pub fn has_microphone() -> bool {
    call_bool("hasMicrophone").unwrap_or(false)
}

/// Ask for the microphone (voice-over recording).
pub fn request_microphone() {
    call_void("requestMicrophone");
}

/// A Java method returning `String[]`, as strings (nulls skipped; at most 64).
fn call_strings(name: &str) -> Vec<String> {
    with_activity(|env, a| {
        let arr = env.call_method(a, name, "()[Ljava/lang/String;", &[])?.l()?;
        if arr.is_null() {
            return Ok(Vec::new());
        }
        let arr = JObjectArray::from(arr);
        let n = env.get_array_length(&arr)?;
        let mut out = Vec::new();
        for i in 0..n.min(64) {
            let item = env.get_object_array_element(&arr, i)?;
            if !item.is_null() {
                let s: String = env.get_string(&JString::from(item))?.into();
                out.push(s);
            }
        }
        Ok(out)
    })
    .map_err(|e| log::debug!("{name}: {e}"))
    .unwrap_or_default()
}

/// The top folder of each shared-storage volume: internal shared storage first, then SD cards and
/// USB drives.
pub fn storage_roots() -> Vec<String> {
    call_strings("storageRoots")
}

/// The user's languages, most preferred first (Settings ▸ General ▸ Interface Language ▸ System
/// Language).
pub fn system_languages() -> Vec<String> {
    call_strings("locales")
}

/// This app's folder in shared storage (`…/Android/data/<package>/files`), always writable.
pub fn app_storage_dir() -> Option<String> {
    call_string("appStorageDir")
}

/// Open `url` in the browser (or the app that handles it).
pub fn open_url(url: &str) -> Result<(), String> {
    let opened = with_activity(|env, a| {
        let s = env.new_string(url)?;
        env.call_method(a, "openUrl", "(Ljava/lang/String;)Z", &[JValue::Object(&s)])?.z()
    })?;
    if opened { Ok(()) } else { Err(format!("no app can open {url}")) }
}

pub fn set_clipboard_text(text: &str) {
    let r = with_activity(|env, a| {
        let s = env.new_string(text)?;
        env.call_method(a, "setClipboardText", "(Ljava/lang/String;)V", &[JValue::Object(&s)])?.v()
    });
    if let Err(e) = r {
        log::warn!("clipboard: {e}");
    }
}

pub fn clipboard_text() -> Option<String> {
    call_string("clipboardText")
}

/// Keep the display on (playback, export) or let it sleep.
pub fn set_keep_screen_on(on: bool) {
    let r = with_activity(|env, a| env.call_method(a, "setKeepScreenOn", "(Z)V", &[JValue::Bool(u8::from(on))])?.v());
    if let Err(e) = r {
        log::debug!("keep screen on: {e}");
    }
}

/// Show or hide the soft keyboard (typed text arrives through [`take_typed_text`]).
pub fn set_keyboard_visible(visible: bool) {
    let r = with_activity(|env, a| env.call_method(a, "setKeyboardVisible", "(Z)V", &[JValue::Bool(u8::from(visible))])?.v());
    if let Err(e) = r {
        log::warn!("soft keyboard: {e}");
    }
}

// --------------------------------------------------------------------------- soft keyboard text

/// Text committed by the soft keyboard, waiting for the next UI frame.
static TYPED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Wakes the UI when text arrives (set by the app).
static ON_TEXT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
/// A keyboard can't type faster than this between two frames; more is dropped.
const TYPED_MAX: usize = 4096;

/// Call `wake` (from the keyboard's thread) whenever the soft keyboard commits text.
pub fn set_on_text(wake: Box<dyn Fn() + Send + Sync>) {
    let _ = ON_TEXT.set(wake);
}

/// The text the soft keyboard committed since the last call, oldest first.
pub fn take_typed_text() -> Vec<String> {
    std::mem::take(&mut *TYPED.lock().unwrap_or_else(PoisonError::into_inner))
}

/// `FilmcraftActivity.nativeText(String)`: called on the activity's UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_filmcraft_FilmcraftActivity_nativeText<'local>(
    mut env: JNIEnv<'local>,
    _class: JObject<'local>,
    text: JString<'local>,
) {
    // A panic must not unwind into the Java VM.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Ok(s) = env.get_string(&text) else { return };
        let s: String = s.into();
        if s.is_empty() {
            return;
        }
        {
            let mut typed = TYPED.lock().unwrap_or_else(PoisonError::into_inner);
            if typed.len() < TYPED_MAX {
                typed.push(s);
            }
        }
        if let Some(wake) = ON_TEXT.get() {
            wake();
        }
    }));
}

// ------------------------------------------------------------------------------- environment

/// Set a process environment variable (`HOME`, `TMPDIR`, `FILMCRAFT_DATA_DIR`: Android sets none
/// that point somewhere an app may write). Call it at start-up only, before FilmCraft starts any
/// thread of its own.
pub fn set_env(key: &str, value: &str) {
    // SAFETY: called from android_main before the app creates threads that read the environment;
    // the Java VM's threads do not read the process environment.
    unsafe { std::env::set_var(key, value) };
}
