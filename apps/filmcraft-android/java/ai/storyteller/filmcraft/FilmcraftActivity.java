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
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
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
import java.util.ArrayList;

public class FilmcraftActivity extends NativeActivity {
    static {
        // NativeActivity opens the library itself, but only a library loaded through
        // System.loadLibrary resolves this class's `native` methods (same file, same handle).
        System.loadLibrary("filmcraft");
    }

    /** Text the soft keyboard committed (any Unicode, emoji included): sent to egui as typed text. */
    private static native void nativeText(String text);

    private static final int REQUEST_STORAGE = 1;
    private static final int REQUEST_MEDIA = 2;
    private static final int REQUEST_MICROPHONE = 3;

    private ImeView imeView;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        imeView = new ImeView(this);
        addContentView(imeView, new ViewGroup.LayoutParams(1, 1));
        immersive();
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
