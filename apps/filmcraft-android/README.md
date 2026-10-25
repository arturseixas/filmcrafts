# FilmCraft for Android

The full FilmCraft editor on Android phones, tablets and Chromebooks: the same engine, codecs,
GPU compositor and egui interface as the desktop app, compiled to `libfilmcraft.so` and started by a
small `NativeActivity` subclass. There is no second UI and no WebView. Every panel, workspace,
command and shortcut on the desktop works the same way here.

| | |
|---|---|
| Minimum | Android 9 (API 28), 64-bit ARM (`arm64-v8a`); `x86_64` for emulators and Chromebooks |
| Graphics | wgpu on **Vulkan**, or **OpenGL ES 3** where Vulkan is missing (or crashed at start-up) |
| Decoding | **MediaCodec** hardware H.264 / HEVC (8-bit 4:2:0), FilmCraft's pure-Rust decoders for everything else and as the fallback |
| Audio | AAudio through cpal (playback clock, voice-over recording) |
| Files | opened **by path**, never copied in: an in-app browser of shared storage, SD cards and USB drives |

## Install

Download `FilmCraft-<version>-arm64-v8a.apk` from the **Actions** tab (the `Android` workflow's
artifacts) or from a release, then open it on the device (allow installing from your browser or file
manager when Android asks). Or, with USB debugging:

```sh
adb install -r FilmCraft-0.13.0-arm64-v8a.apk
```

On the first Import, FilmCraft asks for storage access. **Allow access to all files** lets it read
media and save projects and exports anywhere, like on a computer. **Media only** reads videos,
pictures and music. Without either, FilmCraft works inside its own folder
(`Android/data/ai.storyteller.filmcraft/files`).

## Using it on a touch screen

- The interface opens in landscape, full screen. On a phone, Settings ▸ Appearance ▸ UI Scale ▸ Auto
  zooms it out so the panels fit. Pick a fixed scale there to make it larger or smaller.
- A tap clicks and a drag drags. **Press and hold** opens a context menu where the desktop uses a
  right-click. Pinch to zoom where the desktop zooms with Ctrl + scroll.
- The soft keyboard appears when a text field has focus, and any language or emoji types into it.
  A hardware keyboard (Bluetooth, USB, a Chromebook's) gets every desktop shortcut.
- **Back** closes the file browser, a dialog or a menu (it acts as Escape). It never closes the app.
- Projects auto-save, and the crash-recovery journal brings back unsaved changes if Android stops
  the app in the background.

## Build

Requirements: Rust (stable, 1.95 or newer), the Android SDK with build-tools 35+ and
`platforms/android-35` (or newer), the Android NDK r27 or newer, and a JDK (17+). Gradle is not
needed.

```sh
export ANDROID_HOME=$HOME/Android/Sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version>
apps/filmcraft-android/build.sh                          # dist/android/FilmCraft-<v>-arm64-v8a.apk
apps/filmcraft-android/build.sh --abis x86_64            # for the emulator
apps/filmcraft-android/build.sh --abis arm64-v8a,x86_64  # one APK with both
apps/filmcraft-android/build.sh --debug                  # unoptimised, faster to build
```

The script builds the library with the `android-release` profile (release: thin LTO, one codegen
unit, debug info stripped, 16 KB-aligned segments for Android 15+ devices with 16 KB pages). It
then compiles the activity with javac and d8, links the resources and manifest with aapt2, and
stores the libraries uncompressed and page-aligned so Android maps them straight from the APK. It
signs with `FILMCRAFT_KEYSTORE` (see the top of `build.sh`) or, without one, a debug key.

Logs: `adb logcat -s FilmCraft`, plus `files/Logs/filmcraft.log` and the crash logs in the app's
data folder.

## How it fits together

```
FilmcraftActivity.java (NativeActivity)    immersive full screen, soft-keyboard text, permissions,
        │  JNI                             clipboard, links, keep-screen-on
        ▼
libfilmcraft.so: android_main (src/lib.rs)
  ├─ eframe + winit (android-native-activity) → wgpu (Vulkan | GLES)       src/gfx.rs
  ├─ filmcraft_ui_egui::FilmcraftApp (the shared UI), wrapped by AndroidShell:
  │     file browser (src/file_browser.rs), Back, soft keyboard, clipboard, phone zoom
  ├─ cpal / AAudio: the desktop app's audio.rs and audio_in.rs (+ src/mic.rs for the permission)
  ├─ logcat + file logger (src/log_sink.rs, the desktop logging.rs underneath)
  └─ filmcraft_platform::android: MediaCodec decoder, JNI calls      crates/platform/src/android
```

- **NativeActivity, not GameActivity.** The winit used here reads typed text only from key events,
  so a GameActivity soft keyboard (which sends text events) would type nothing. NativeActivity plus
  an input view in the activity sends each committed string to the native side (`nativeText`).
  Deletions and Enter arrive as key events. AccessKit's Android adapter needs GameActivity, so the
  egui accessibility tree is still built (automation ids, roles) but not exposed to TalkBack.
- **Hardware decoding** (`crates/platform/src/android/mediacodec.rs`) renders MediaCodec output into
  an `AImageReader` (`YUV_420_888`). That image layout is the same on every vendor, unlike
  MediaCodec's byte buffers, and its planes are copied into FilmCraft's planar frames. It sits behind
  the same `HybridDecoder` as VideoToolbox, Media Foundation and VA-API, so a decoder that fails
  mid-stream continues in software without a visible glitch. Settings ▸ Playback ▸ Hardware decoding
  turns it off. At most six hardware sessions are open at once; more clips decode in software.
- **Files by path.** FilmCraft's engine, projects, relinking, proxies and recovery all work with
  paths. Android's system picker returns content URIs, which would mean copying every clip into the
  app first. With storage access, paths work as on Linux, and imports are zero-copy.
- **Graphics fallback** (`src/gfx.rs`): if a start dies before the first frames paint (a Vulkan
  driver crash), the next start uses OpenGL ES and says so, with a button back to the default.

Not yet on Android: the system share sheet and opening files from other apps ("Open with"),
Android's system file picker (cloud drives), hardware encoding (exports use FilmCraft's own
encoders), and the TCP control channel / MCP bridge.
