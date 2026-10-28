//! JNI calls into the app's activity (`ai.storyteller.filmcraft.FilmcraftActivity`, a
//! `NativeActivity` subclass in `apps/filmcraft-android/java`) and the one native method it calls
//! back (`nativeText`, text from the soft keyboard).
//!
//! The activity and the Java VM are given by the app at start-up ([`set_activity`], from
//! android-activity's `AndroidApp`). Every call attaches the calling thread for its duration, clears a pending
//! Java exception and reports failure as `None` / `Err` (a missing method, an old activity class,
//! no activity yet): a JNI problem never takes the app down.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use jni::objects::{JObject, JObjectArray, JString, JValue};
use jni::sys::{jint, jlong};
use jni::{JNIEnv, JavaVM};

/// The process's Java VM and the `FilmcraftActivity` object, set by [`set_activity`].
static VM: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static ACTIVITY: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Remember the Java VM and the activity for the calls below. `ndk-context` is not used for this:
/// the context it holds is the `Application`, which has none of the activity's methods.
///
/// # Safety
/// `vm` must be the process's `JavaVM*` and `activity` a JNI *global* reference to the
/// `FilmcraftActivity`, both valid until this is called again (android-activity's
/// `AndroidApp::vm_as_ptr` and `activity_as_ptr` are, while that `AndroidApp` lives, and
/// `android_main` does not return before the app is over).
pub unsafe fn set_activity(vm: *mut c_void, activity: *mut c_void) {
    VM.store(vm, Ordering::Release);
    ACTIVITY.store(activity, Ordering::Release);
}

/// Run `f` with a JNI environment for this thread and the activity object.
fn with_activity<T>(f: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>) -> Result<T, String> {
    let (vm, activity) = (VM.load(Ordering::Acquire), ACTIVITY.load(Ordering::Acquire));
    if vm.is_null() || activity.is_null() {
        return Err("no Android activity".into());
    }
    // SAFETY: `set_activity`'s contract: the process's JavaVM pointer, valid for the life of the
    // process.
    let vm = unsafe { JavaVM::from_raw(vm.cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // SAFETY: `set_activity`'s contract: a global reference that stays valid; this borrowed JObject
    // never deletes it (a plain JObject has no destructor).
    let activity = unsafe { JObject::from_raw(activity.cast()) };
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
/// Wakes the UI when the keyboard or an import has something for it (set by the app).
static ON_WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
/// A keyboard can't type faster than this between two frames; more is dropped.
const TYPED_MAX: usize = 4096;

/// Call `wake` (from the keyboard's or the import's thread) whenever the soft keyboard commits
/// text or an import from the Gallery or Files moves on.
pub fn set_wake(wake: Box<dyn Fn() + Send + Sync>) {
    let _ = ON_WAKE.set(wake);
}

fn wake() {
    if let Some(wake) = ON_WAKE.get() {
        wake();
    }
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
        wake();
    }));
}

// ------------------------------------------------------------------------------ keyboard height

static KEYBOARD_PX: AtomicU32 = AtomicU32::new(0);

/// How many pixels of the window's bottom the soft keyboard covers (0: it is hidden).
pub fn keyboard_height_px() -> u32 {
    KEYBOARD_PX.load(Ordering::Relaxed)
}

/// `FilmcraftActivity.nativeKeyboardHeight(int)`: from the activity's UI thread whenever the
/// keyboard shows, hides or changes size.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_filmcraft_FilmcraftActivity_nativeKeyboardHeight<'local>(
    _env: JNIEnv<'local>,
    _class: JObject<'local>,
    pixels: jint,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        KEYBOARD_PX.store(u32::try_from(pixels).unwrap_or(0), Ordering::Relaxed);
        wake();
    }));
}

// ------------------------------------------------------------------------ export to the Gallery

/// How the last [`publish_to_gallery`] ended, until [`take_publish_result`] takes it: where the file
/// went (`Movies/FilmCraft/<name>`), or what stopped it.
static PUBLISH_RESULT: Mutex<Option<Result<String, String>>> = Mutex::new(None);

/// Copy the finished export at `path` (in the app's own folder) into the shared Movies, Music or
/// Pictures folder, which needs no permission, and delete the original. Android 10 and later.
pub fn publish_to_gallery(path: &str) -> Result<(), String> {
    with_activity(|env, a| {
        let s = env.new_string(path)?;
        env.call_method(a, "publishToGallery", "(Ljava/lang/String;)V", &[JValue::Object(&s)])?.v()
    })
}

pub fn take_publish_result() -> Option<Result<String, String>> {
    PUBLISH_RESULT.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// `FilmcraftActivity.nativeExportPublished(String, String)`, from the copy thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_filmcraft_FilmcraftActivity_nativeExportPublished<'local>(
    mut env: JNIEnv<'local>,
    _class: JObject<'local>,
    location: JString<'local>,
    error: JString<'local>,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let text = |env: &mut JNIEnv<'local>, s: &JString<'local>| if s.is_null() { None } else { env.get_string(s).ok().map(String::from) };
        let result = match (text(&mut env, &location), text(&mut env, &error)) {
            (Some(l), None) => Ok(l),
            (_, Some(e)) => Err(e),
            (None, None) => Err("the export was not copied".to_string()),
        };
        *PUBLISH_RESULT.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        wake();
    }));
}

// ------------------------------------------------------------- import from Gallery / Files

/// How far the copy of a picked file has got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportProgress {
    pub name: String,
    /// Position of this file among the picked ones, from 0.
    pub index: u32,
    pub count: u32,
    pub done: u64,
    /// The file's size; 0 when the provider does not say.
    pub total: u64,
}

/// The copy in progress (none once it is over).
static IMPORT_PROGRESS: Mutex<Option<ImportProgress>> = Mutex::new(None);
/// How the last import ended, until [`take_import_result`] takes it: the copied files' paths (none:
/// the picker was dismissed, or the copy cancelled), or what stopped it.
static IMPORT_RESULT: Mutex<Option<Result<Vec<String>, String>>> = Mutex::new(None);

/// Open the system's Gallery picker (`gallery`: videos and pictures, no permission needed) or
/// Files (documents from any app, cloud drive, SD card or USB drive), then copy what is chosen into
/// the app's `Imported` folder. Follow it with [`import_progress`] and [`take_import_result`].
pub fn pick_media(gallery: bool, multiple: bool) -> Result<(), String> {
    *IMPORT_PROGRESS.lock().unwrap_or_else(PoisonError::into_inner) = None;
    *IMPORT_RESULT.lock().unwrap_or_else(PoisonError::into_inner) = None;
    with_activity(|env, a| env.call_method(a, "pickMedia", "(ZZ)V", &[JValue::Bool(u8::from(gallery)), JValue::Bool(u8::from(multiple))])?.v())
}

/// The copy in progress, if any.
pub fn import_progress() -> Option<ImportProgress> {
    IMPORT_PROGRESS.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// How the import ended, once: `Ok` with no paths when the picker was dismissed or the copy
/// cancelled.
pub fn take_import_result() -> Option<Result<Vec<String>, String>> {
    IMPORT_RESULT.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Stop the copy in progress (its partial file is deleted).
pub fn cancel_import() {
    call_void("cancelImport");
}

/// `FilmcraftActivity.nativeImportProgress(String, int, int, long, long)`, from the copy thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_filmcraft_FilmcraftActivity_nativeImportProgress<'local>(
    mut env: JNIEnv<'local>,
    _class: JObject<'local>,
    name: JString<'local>,
    index: jint,
    count: jint,
    done: jlong,
    total: jlong,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let name = env.get_string(&name).map(String::from).unwrap_or_default();
        let progress = ImportProgress {
            name,
            index: u32::try_from(index).unwrap_or(0),
            count: u32::try_from(count).unwrap_or(0),
            done: u64::try_from(done).unwrap_or(0),
            total: u64::try_from(total).unwrap_or(0),
        };
        *IMPORT_PROGRESS.lock().unwrap_or_else(PoisonError::into_inner) = Some(progress);
        wake();
    }));
}

/// `FilmcraftActivity.nativeImportFinished(String[], String)`, from the copy thread (or the UI
/// thread when the picker was dismissed).
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_filmcraft_FilmcraftActivity_nativeImportFinished<'local>(
    mut env: JNIEnv<'local>,
    _class: JObject<'local>,
    paths: JObjectArray<'local>,
    error: JString<'local>,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut out = Vec::new();
        if !paths.is_null() {
            let n = env.get_array_length(&paths).unwrap_or(0);
            for i in 0..n.min(10_000) {
                if let Ok(item) = env.get_object_array_element(&paths, i)
                    && !item.is_null()
                    && let Ok(s) = env.get_string(&JString::from(item))
                {
                    out.push(String::from(s));
                }
            }
        }
        let error = if error.is_null() { None } else { env.get_string(&error).ok().map(String::from) };
        let result = match error {
            // some files were copied before the error: those still import
            Some(e) if out.is_empty() => Err(e),
            Some(e) => {
                log::warn!("import stopped early: {e}");
                Ok(out)
            }
            None => Ok(out),
        };
        *IMPORT_PROGRESS.lock().unwrap_or_else(PoisonError::into_inner) = None;
        *IMPORT_RESULT.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        wake();
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
