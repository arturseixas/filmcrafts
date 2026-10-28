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
| Files | the system **Gallery** and **Files** pickers (copied in), or an in-app browser of shared storage, SD cards and USB drives that opens **by path** with no copy |

## Install

Download `FilmCraft-<version>-arm64-v8a.apk` from the **Actions** tab (the `Android` workflow's
artifacts) or from a release, then open it on the device (allow installing from your browser or file
manager when Android asks). Or, with USB debugging:

```sh
adb install -r FilmCraft-0.13.0-arm64-v8a.apk
```

**Importing media.** File ▸ Import (and the Project panel's import button) opens FilmCraft's
browser with two ways in at the top, like other mobile editors:

- **Gallery** opens Android's Photo Picker: your videos and photos, with thumbnails, several at
  once. No permission is needed.
- **Files app** opens the system document picker: anything in Downloads, an SD card or USB drive,
  or a cloud drive (Google Drive and others), again with no permission.

What you pick is copied into FilmCraft's own folder (`Android/data/ai.storyteller.filmcraft/files/Imported`,
with a progress bar and Cancel) and imported from there; picking the same file again reuses the
copy. The picker's access is temporary, which is why a copy is needed, and it keeps the project
working after Android stops giving FilmCraft that access. The browser itself below those two
buttons opens files in place, without copying, once you give it storage access.

On the first use of the browser, FilmCraft asks for storage access. **Allow access to all files** lets it read
media and save projects and exports anywhere, like on a computer. **Media only** reads videos,
pictures and music. Without either, FilmCraft works inside its own folder
(`Android/data/ai.storyteller.filmcraft/files`).

## Using it on a touch screen

**Which interface.** A tablet (a screen 600 dp or more on its short side, such as the Galaxy Tab S9
Ultra) gets the **desktop interface**: the header with the File … Help menus, the docked panels
(Source, Program, Project, Timeline, Effect Controls …), workspaces and every keyboard shortcut,
for a mouse and keyboard. Touch works there too (see below). A phone gets the **phone layout**
described next. The choice follows the screen size every frame (split screen, folding).

**Mouse and keyboard.** A mouse has hover, left, right and middle buttons and the wheel (Ctrl +
wheel zooms the timeline), and the keyboard's Ctrl, Shift, Alt and Meta reach the shortcuts: Ctrl+Z,
comma and period to insert and overwrite, J K L, and so on. winit 0.30 reports every Android pointer
as a touch and has no modifier events, so the repository carries a small patch of it
(`third_party/winit`, `PATCHES.md` F1).

**Speed ramps on the desktop interface.** Effect Controls ▸ Time Remapping ▸ Speed has the
stopwatch and keyframes like any effect; Clip ▸ Speed Ramp has ready-made shapes (slow to fast, hero
time, bullet time …) that keep the clip's length and picture, and *Remove Speed Ramp*.

**The phone layout** replaces the desktop panels with what an edit needs, in dp-sized controls:
the **Program** picture and its transport on top (in landscape on the left), the **Timeline**
under it (on the right), a bar with Undo, Redo and **Export** above, and a **tool strip** along the
bottom: Import, Media, Add, Split, Delete, Speed, Properties, Audio, Effects, Text and Color. ☰ opens
the desktop menus as a list. A tool that needs a panel opens it as a *sheet* in place of the
Timeline (the picture stays in view); tap the tool again or ✕ to go back. Timeline zoom is the pill
at the Timeline's corner. The result of each tap shows above the strip for a few seconds. While the
soft keyboard is up, the strip steps aside and the interface keeps above the keyboard.

**Putting clips on the timeline.** Import (see above), open **Media**, tap a clip (tap several to
choose several), then tap **Add**: the clips land back to back at the end of the timeline, and a
timeline that does not exist yet is made from the first clip. **Insert** and **Overwrite** put them
at the playhead. **Split** cuts the chosen clip at the playhead and **Delete** removes the chosen
clips and closes the gap.

**Speed and speed ramps.** Tap a clip on the timeline, then **Speed**: tap 25 % … 400 % or Reverse
for a constant speed (the clip gets shorter or longer and the clips after it follow), or tap a ramp
shape (slow to fast, fast to slow, hero time, bullet time…) to speed up and slow down along it while
the clip keeps its length and picture. The graph shows the speed along the clip (tap it to move the
playhead); every point of the ramp is listed with ‒ / + (a quarter less or more) and ✕, and **Add
point at the playhead** adds one. A clip's linked sound ramps with it. The ramp is the Time
Remapping effect's Speed keyframes (media time, like every effect keyframe), so the desktop
Effect Controls edit the same points, and `clip.speedRamp` / `clip.speedRampKey` do from the
control channel.

- A tap clicks and a drag drags. **Press and hold** opens a context menu where the desktop uses a
  right-click. **Two fingers** scroll panels and the timeline; spreading or pinching them zooms the
  timeline.
- The soft keyboard appears when a text field has focus, and any language or emoji types into it.
  A hardware keyboard (Bluetooth, USB, a Chromebook's) gets every desktop shortcut.
- **Back** closes the file browser, a dialog or a menu (it acts as Escape). It never closes the app.
- Projects auto-save, and the crash-recovery journal brings back unsaved changes if Android stops
  the app in the background.
- Exports go where you choose in the save dialog; without storage access that is FilmCraft's own
  folder, which other apps cannot see, so allow access to all files to export to Movies.

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
  │     phone layout and speed sheet (src/phone.rs, speed_sheet.rs, chrome.rs, edit_actions.rs), touch gestures
  │     (src/touch.rs), file browser (src/file_browser.rs), Back, soft keyboard, clipboard
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
- **Files by path, and the system pickers.** FilmCraft's engine, projects, relinking, proxies and
  recovery all work with paths. Android's Gallery and Files pickers return content URIs with
  temporary access, so what they return is copied into the app's `Imported` folder
  (`FilmcraftActivity.pickMedia`) and the copies' paths go to the same import as any other file.
  With storage access, the in-app browser reads paths directly and imports are zero-copy.
- **Graphics fallback** (`src/gfx.rs`): if a start dies before the first frames paint (a Vulkan
  driver crash), the next start uses OpenGL ES and says so, with a button back to the default.

Not yet on Android: the system share sheet and opening files from other apps ("Open with"),
hardware encoding (exports use FilmCraft's own
encoders), and the TCP control channel / MCP bridge.
