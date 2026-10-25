//! Android: hardware decoding through MediaCodec ([`mediacodec`]) and the calls into the app's
//! activity that the Android app needs from Java ([`jni`]: storage and microphone permissions,
//! storage volumes, links, the clipboard, the soft keyboard, keeping the screen on).
//!
//! Only [`jni`] contains `unsafe` (the JNI entry points); [`mediacodec`] goes through the `ndk`
//! crate's safe MediaCodec and ImageReader bindings.

#[allow(unsafe_code)]
pub mod jni;
pub mod mediacodec;
