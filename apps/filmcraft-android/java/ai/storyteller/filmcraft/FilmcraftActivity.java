// FilmCraft for Android: the activity around the native app (libfilmcraft.so).
//
// Everything FilmCraft draws and decides lives in Rust (apps/filmcraft-android, NativeActivity
// through android-activity and winit). This class only adds what NativeActivity lacks and the
// native side cannot do on its own thread: immersive full screen, a soft keyboard that delivers
// any Unicode text, runtime permissions, storage volumes, links, the clipboard and keeping the
// screen on. The
// native side calls the public methods below through JNI (crates/platform/src/android/jni.rs),
// from its own thread: each one is safe to call from any thread.
package ai.storyteller.filmcraft;

import android.Manifest;
import android.app.NativeActivity;
import android.content.ClipData;
import android.content.ActivityNotFoundException;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import android.provider.Settings;
import android.webkit.MimeTypeMap;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.concurrent.atomic.AtomicBoolean;

public class FilmcraftActivity extends NativeActivity {
    static {
        // NativeActivity opens the library itself, but only a library loaded through
        // System.loadLibrary resolves this class's `native` methods (same file, same handle).
        System.loadLibrary("filmcraft");
    }

    /** Text the soft keyboard committed (any Unicode, emoji included): sent to egui as typed text. */
    private static native void nativeText(String text);

    /** An export was copied into the Gallery (where, on success) or could not be (error). */
    private static native void nativeExportPublished(String where, String error);

    /** How tall the soft keyboard is in pixels (0: hidden), as it covers the bottom of the window. */
    private static native void nativeKeyboardHeight(int pixels);

    /** Importing from the Gallery or Files: the file being copied and how far along it is. */
    private static native void nativeImportProgress(String name, int index, int count, long done, long total);

    /**
     * The import is over: the copied files' paths (none when the picker was dismissed or the copy
     * cancelled) and the error that stopped it, or null.
     */
    private static native void nativeImportFinished(String[] paths, String error);

    private static final int REQUEST_STORAGE = 1;
    private static final int REQUEST_MEDIA = 2;
    private static final int REQUEST_MICROPHONE = 3;
    private static final int REQUEST_PICK_MEDIA = 10;

    /** Pickers hand out time-limited read access, so what is chosen is copied here at once. */
    private static final String IMPORT_DIR = "Imported";

    private final AtomicBoolean importCancelled = new AtomicBoolean();
    private volatile boolean importing;
    private int keyboardPixels;

    private ImeView imeView;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        imeView = new ImeView(this);
        addContentView(imeView, new ViewGroup.LayoutParams(1, 1));
        immersive();
        // The window does not resize for the keyboard (it would resize the drawing surface on every
        // key), so the app is told how much of the bottom it covers and keeps its fields above it.
        final View decor = getWindow().getDecorView();
        decor.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            android.graphics.Rect visible = new android.graphics.Rect();
            decor.getWindowVisibleDisplayFrame(visible);
            int height = decor.getRootView().getHeight();
            int covered = height - visible.bottom;
            // a system bar shown for a moment is not a keyboard
            if (covered < height / 6) {
                covered = 0;
            }
            if (covered != keyboardPixels) {
                keyboardPixels = covered;
                nativeKeyboardHeight(covered);
            }
        });
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            immersive();
        }
    }

    /**
     * Hide the status and navigation bars (a swipe from the edge shows them for a moment). The
     * window keeps out of a display cutout (the default cutout mode), so no panel goes under it.
     */
    @SuppressWarnings("deprecation")
    private void immersive() {
        if (Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
            WindowInsetsController c = getWindow().getInsetsController();
            if (c != null) {
                c.hide(WindowInsets.Type.systemBars());
                c.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);
        }
    }

    // ------------------------------------------------------------------ called from native code

    /** Android's API level. */
    public int sdkInt() {
        return Build.VERSION.SDK_INT;
    }

    /**
     * Whether FilmCraft can read and write files anywhere in shared storage by path (Android 11+:
     * "All files access"; before that the storage permissions).
     */
    public boolean hasStorageAccess() {
        if (Build.VERSION.SDK_INT >= 30) {
            return Environment.isExternalStorageManager();
        }
        return checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED;
    }

    /** Whether FilmCraft can read the videos, pictures and music in shared storage by path. */
    public boolean hasMediaAccess() {
        if (hasStorageAccess()) {
            return true;
        }
        if (Build.VERSION.SDK_INT >= 33) {
            return checkSelfPermission(Manifest.permission.READ_MEDIA_VIDEO) == PackageManager.PERMISSION_GRANTED;
        }
        return checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED;
    }

    /** Ask for full storage access: the system's "All files access" page on Android 11+. */
    public void requestStorageAccess() {
        runOnUiThread(() -> {
            if (Build.VERSION.SDK_INT >= 30) {
                try {
                    Intent i = new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:" + getPackageName()));
                    startActivity(i);
                } catch (RuntimeException e) {
                    try {
                        startActivity(new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION));
                    } catch (RuntimeException ignored) {
                        // no settings page on this device: media access below still works
                    }
                }
            } else {
                requestPermissions(new String[] {Manifest.permission.READ_EXTERNAL_STORAGE, Manifest.permission.WRITE_EXTERNAL_STORAGE}, REQUEST_STORAGE);
            }
        });
    }

    /** Ask for read access to videos, pictures and music only. */
    public void requestMediaAccess() {
        runOnUiThread(() -> {
            if (Build.VERSION.SDK_INT >= 33) {
                requestPermissions(new String[] {Manifest.permission.READ_MEDIA_VIDEO, Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_AUDIO}, REQUEST_MEDIA);
            } else {
                requestPermissions(new String[] {Manifest.permission.READ_EXTERNAL_STORAGE}, REQUEST_MEDIA);
            }
        });
    }

    public boolean hasMicrophone() {
        return checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED;
    }

    public void requestMicrophone() {
        runOnUiThread(() -> requestPermissions(new String[] {Manifest.permission.RECORD_AUDIO}, REQUEST_MICROPHONE));
    }

    /**
     * Top folders of shared storage, one per volume: the internal shared storage first, then SD
     * cards and USB drives (derived from this app's folder on each volume).
     */
    @SuppressWarnings("deprecation")
    public String[] storageRoots() {
        ArrayList<String> roots = new ArrayList<>();
        File primary = Environment.getExternalStorageDirectory();
        if (primary != null) {
            roots.add(primary.getAbsolutePath());
        }
        File[] dirs = getExternalFilesDirs(null);
        if (dirs != null) {
            for (File d : dirs) {
                if (d == null) {
                    continue;
                }
                String p = d.getAbsolutePath();
                int cut = p.indexOf("/Android/data/");
                String root = cut > 0 ? p.substring(0, cut) : p;
                if (!roots.contains(root)) {
                    roots.add(root);
                }
            }
        }
        return roots.toArray(new String[0]);
    }

    /** This app's folder in shared storage (readable over USB and by file managers), or null. */
    public String appStorageDir() {
        File d = getExternalFilesDir(null);
        return d == null ? null : d.getAbsolutePath();
    }

    /** The user's languages, most preferred first (BCP 47 tags). */
    public String[] locales() {
        android.os.LocaleList list = android.os.LocaleList.getDefault();
        String[] out = new String[list.size()];
        for (int i = 0; i < list.size(); i++) {
            out[i] = list.get(i).toLanguageTag();
        }
        return out;
    }

    // ------------------------------------------------------------ import from Gallery / Files

    /**
     * Open the system's Gallery picker (the Photo Picker: videos and pictures, no permission
     * needed) or Files (the document picker: any app's storage, cloud drives, SD cards and USB
     * drives). What the user chooses is copied into this app's Imported folder and reported to
     * the native side ({@link #nativeImportProgress}, {@link #nativeImportFinished}).
     */
    public void pickMedia(boolean gallery, boolean multiple) {
        runOnUiThread(() -> {
            if (importing) {
                return;
            }
            try {
                if (gallery) {
                    pickFromGallery(multiple);
                } else {
                    Intent i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                    i.addCategory(Intent.CATEGORY_OPENABLE);
                    i.setType("*/*");
                    i.putExtra(Intent.EXTRA_MIME_TYPES, new String[] {"video/*", "audio/*", "image/*"});
                    i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
                    startActivityForResult(i, REQUEST_PICK_MEDIA);
                }
            } catch (RuntimeException e) {
                nativeImportFinished(new String[0], "No app can pick files here: " + e.getMessage());
            }
        });
    }

    private void pickFromGallery(boolean multiple) {
        if (Build.VERSION.SDK_INT >= 30) {
            // the Photo Picker: part of Android 13, and of Android 11 and 12 through system updates
            Intent i = new Intent("android.provider.action.PICK_IMAGES");
            if (multiple) {
                int max = Build.VERSION.SDK_INT >= 33 ? MediaStore.getPickImagesMaxLimit() : 100;
                i.putExtra("android.provider.extra.PICK_IMAGES_MAX", max);
            }
            try {
                startActivityForResult(i, REQUEST_PICK_MEDIA);
                return;
            } catch (ActivityNotFoundException e) {
                // no Photo Picker on this device: the generic content picker below
            }
        }
        Intent i = new Intent(Intent.ACTION_GET_CONTENT);
        i.addCategory(Intent.CATEGORY_OPENABLE);
        i.setType("*/*");
        i.putExtra(Intent.EXTRA_MIME_TYPES, new String[] {"video/*", "image/*"});
        i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
        startActivityForResult(i, REQUEST_PICK_MEDIA);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_PICK_MEDIA) {
            return;
        }
        ArrayList<Uri> uris = new ArrayList<>();
        if (resultCode == RESULT_OK && data != null) {
            if (data.getClipData() != null) {
                for (int i = 0; i < data.getClipData().getItemCount(); i++) {
                    uris.add(data.getClipData().getItemAt(i).getUri());
                }
            } else if (data.getData() != null) {
                uris.add(data.getData());
            }
        }
        if (uris.isEmpty()) {
            nativeImportFinished(new String[0], null);
            return;
        }
        importing = true;
        importCancelled.set(false);
        Thread worker = new Thread(() -> copyAll(uris), "filmcraft-import");
        worker.start();
    }

    /** Stop the copy in progress; its partial file is deleted. */
    public void cancelImport() {
        importCancelled.set(true);
    }

    private void copyAll(ArrayList<Uri> uris) {
        ArrayList<String> done = new ArrayList<>();
        String error = null;
        try {
            File dir = importDir();
            for (int i = 0; i < uris.size() && !importCancelled.get(); i++) {
                String path = copyOne(uris.get(i), dir, i, uris.size());
                if (path != null) {
                    done.add(path);
                }
            }
        } catch (IOException | RuntimeException e) {
            error = e.getMessage() == null ? e.toString() : e.getMessage();
        } finally {
            importing = false;
        }
        if (importCancelled.get()) {
            done.clear();
            error = null;
        }
        nativeImportFinished(done.toArray(new String[0]), error);
    }

    private File importDir() throws IOException {
        File base = getExternalFilesDir(null);
        File dir = new File(base != null ? base : getFilesDir(), IMPORT_DIR);
        if (!dir.isDirectory() && !dir.mkdirs()) {
            throw new IOException("Can't create " + dir);
        }
        return dir;
    }

    /** Copy one picked item into {@code dir}, reporting progress; returns its path, or null if cancelled. */
    private String copyOne(Uri uri, File dir, int index, int count) throws IOException {
        String name = null;
        long size = -1;
        try (Cursor c = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE}, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                name = c.getString(0);
                if (!c.isNull(1)) {
                    size = c.getLong(1);
                }
            }
        } catch (RuntimeException ignored) {
            // a provider without the columns: the name comes from the address
        }
        if (name == null || name.isEmpty()) {
            name = uri.getLastPathSegment() == null ? "Media" : uri.getLastPathSegment();
        }
        name = name.replaceAll("[\\\\/:*?\"<>|\\p{Cntrl}]", "_");
        if (name.startsWith(".")) {
            name = "_" + name.substring(1);
        }
        if (name.length() > 150) {
            int dot = name.lastIndexOf('.');
            String ext = dot > 0 && name.length() - dot <= 10 ? name.substring(dot) : "";
            name = name.substring(0, 150 - ext.length()) + ext;
        }
        if (name.lastIndexOf('.') <= 0) {
            String type = getContentResolver().getType(uri);
            String ext = type == null ? null : MimeTypeMap.getSingleton().getExtensionFromMimeType(type);
            if (ext != null) {
                name = name + "." + ext;
            }
        }
        File target = new File(dir, name);
        // the same file chosen again is not copied again
        if (size >= 0 && target.isFile() && target.length() == size) {
            return target.getAbsolutePath();
        }
        if (target.exists()) {
            int dot = name.lastIndexOf('.');
            String stem = dot > 0 ? name.substring(0, dot) : name;
            String ext = dot > 0 ? name.substring(dot) : "";
            for (int n = 2; target.exists(); n++) {
                target = new File(dir, stem + " (" + n + ")" + ext);
            }
        }
        File part = new File(dir, target.getName() + ".part");
        boolean complete = false;
        try (InputStream in = getContentResolver().openInputStream(uri); OutputStream out = new FileOutputStream(part)) {
            if (in == null) {
                throw new IOException("Can't open " + name);
            }
            byte[] buf = new byte[1 << 20];
            long copied = 0;
            long reported = 0;
            nativeImportProgress(name, index, count, 0, size);
            int n;
            while ((n = in.read(buf)) > 0) {
                if (importCancelled.get()) {
                    return null;
                }
                out.write(buf, 0, n);
                copied += n;
                long now = System.nanoTime();
                if (now - reported > 100_000_000L) {
                    reported = now;
                    nativeImportProgress(name, index, count, copied, size);
                }
            }
            out.flush();
            complete = true;
        } finally {
            if (!complete) {
                // cancelled, or the copy failed (a full disk): no half file stays behind
                //noinspection ResultOfMethodCallIgnored
                part.delete();
            }
        }
        if (!part.renameTo(target)) {
            //noinspection ResultOfMethodCallIgnored
            part.delete();
            throw new IOException("Can't save " + name);
        }
        return target.getAbsolutePath();
    }

    // ------------------------------------------------------------------ export to the Gallery

    /**
     * Copy a finished export from this app's own folder (which other apps cannot see) into the
     * shared Movies / Music / Pictures folders through MediaStore, which needs no permission, then
     * delete the original. Reports back through {@link #nativeExportPublished}. Android 10+.
     */
    public void publishToGallery(String path) {
        Thread worker = new Thread(() -> {
            File src = new File(path);
            try {
                if (Build.VERSION.SDK_INT < 29) {
                    throw new IOException("Needs Android 10 or later: allow access to all files and export to Movies instead");
                }
                String name = src.getName();
                int dot = name.lastIndexOf('.');
                String ext = dot > 0 ? name.substring(dot + 1).toLowerCase() : "";
                String mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext);
                if (mime == null) {
                    mime = "application/octet-stream";
                }
                Uri collection;
                String folder;
                if (mime.startsWith("audio/")) {
                    collection = MediaStore.Audio.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY);
                    folder = "Music/FilmCraft";
                } else if (mime.startsWith("image/")) {
                    collection = MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY);
                    folder = "Pictures/FilmCraft";
                } else {
                    collection = MediaStore.Video.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY);
                    folder = "Movies/FilmCraft";
                }
                android.content.ContentValues values = new android.content.ContentValues();
                values.put(MediaStore.MediaColumns.DISPLAY_NAME, name);
                values.put(MediaStore.MediaColumns.MIME_TYPE, mime);
                values.put(MediaStore.MediaColumns.RELATIVE_PATH, folder);
                values.put(MediaStore.MediaColumns.IS_PENDING, 1);
                Uri uri = getContentResolver().insert(collection, values);
                if (uri == null) {
                    throw new IOException("The system would not make a file in " + folder);
                }
                boolean complete = false;
                try (java.io.FileInputStream in = new java.io.FileInputStream(src); OutputStream out = getContentResolver().openOutputStream(uri)) {
                    if (out == null) {
                        throw new IOException("Can't write to " + folder);
                    }
                    byte[] buf = new byte[1 << 20];
                    int n;
                    while ((n = in.read(buf)) > 0) {
                        out.write(buf, 0, n);
                    }
                    out.flush();
                    complete = true;
                } finally {
                    if (!complete) {
                        getContentResolver().delete(uri, null, null);
                    }
                }
                values.clear();
                values.put(MediaStore.MediaColumns.IS_PENDING, 0);
                getContentResolver().update(uri, values, null, null);
                //noinspection ResultOfMethodCallIgnored
                src.delete();
                nativeExportPublished(folder + "/" + name, null);
            } catch (IOException | RuntimeException e) {
                nativeExportPublished(null, e.getMessage() == null ? e.toString() : e.getMessage());
            }
        }, "filmcraft-publish");
        worker.start();
    }

    public boolean openUrl(String url) {
        try {
            Intent i = new Intent(Intent.ACTION_VIEW, Uri.parse(url));
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            startActivity(i);
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    public void setClipboardText(String text) {
        runOnUiThread(() -> {
            ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
            if (cm != null) {
                cm.setPrimaryClip(ClipData.newPlainText("FilmCraft", text));
            }
        });
    }

    public String clipboardText() {
        ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm == null || !cm.hasPrimaryClip()) {
            return null;
        }
        ClipData clip = cm.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            return null;
        }
        CharSequence t = clip.getItemAt(0).coerceToText(this);
        return t == null ? null : t.toString();
    }

    /** Keep the display on (while playing or exporting) or let it sleep again. */
    public void setKeepScreenOn(boolean on) {
        runOnUiThread(() -> {
            if (on) {
                getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            } else {
                getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            }
        });
    }

    /** Show (true) or hide the soft keyboard for egui's focused text field. */
    public void setKeyboardVisible(boolean visible) {
        runOnUiThread(() -> {
            InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
            if (imm == null || imeView == null) {
                return;
            }
            if (visible) {
                imeView.requestFocus();
                imm.restartInput(imeView);
                imm.showSoftInput(imeView, 0);
            } else {
                imm.hideSoftInputFromWindow(imeView.getWindowToken(), 0);
            }
        });
    }

    /**
     * The view the soft keyboard types into. NativeActivity's own view has no input connection,
     * so keyboards could only send the few characters that map to key codes; this one hands every
     * committed string to the native side and turns deletions and Enter into key events, which
     * NativeActivity routes to the native input queue like hardware keys.
     */
    private static final class ImeView extends View {
        ImeView(Context context) {
            super(context);
            setFocusable(true);
            setFocusableInTouchMode(true);
        }

        @Override
        public boolean onCheckIsTextEditor() {
            return true;
        }

        @Override
        public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
            // No suggestions or composing: each key press arrives as committed text at once.
            outAttrs.inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD;
            outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_ACTION_DONE;
            return new BaseInputConnection(this, false) {
                @Override
                public boolean commitText(CharSequence text, int newCursorPosition) {
                    if (text != null && text.length() > 0) {
                        String s = text.toString();
                        if (s.equals("\n")) {
                            key(KeyEvent.KEYCODE_ENTER);
                        } else {
                            nativeText(s);
                        }
                    }
                    return true;
                }

                @Override
                public boolean deleteSurroundingText(int beforeLength, int afterLength) {
                    for (int i = 0; i < beforeLength; i++) {
                        key(KeyEvent.KEYCODE_DEL);
                    }
                    for (int i = 0; i < afterLength; i++) {
                        key(KeyEvent.KEYCODE_FORWARD_DEL);
                    }
                    return true;
                }

                @Override
                public boolean performEditorAction(int actionCode) {
                    key(KeyEvent.KEYCODE_ENTER);
                    return true;
                }

                private void key(int code) {
                    sendKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, code));
                    sendKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, code));
                }
            };
        }
    }
}
