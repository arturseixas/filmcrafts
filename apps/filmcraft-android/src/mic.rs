//! Voice-over input: the desktop app's cpal input (AAudio on Android) behind the microphone
//! permission, which Android grants at run time. Recording without it asks for it and reports why
//! nothing was recorded; the next take works once the user allows it.

use filmcraft_engine::voiceover::{AudioInput, InputFormat};
use filmcraft_platform::android::jni;

use crate::audio_in::CpalIn;

pub struct MicIn(pub CpalIn);

impl AudioInput for MicIn {
    fn configure_host(&mut self, host: &str) {
        self.0.configure_host(host);
    }

    fn devices(&self) -> Vec<String> {
        self.0.devices()
    }

    fn channels(&self, device: &str) -> u16 {
        self.0.channels(device)
    }

    fn start(&mut self, device: &str, sample_rate: u32) -> Result<InputFormat, String> {
        if !jni::has_microphone() {
            jni::request_microphone();
            return Err(filmcraft_ui_egui::i18n::t("FilmCraft needs microphone access to record: allow it, then record again").to_string());
        }
        self.0.start(device, sample_rate)
    }

    fn read(&mut self, frames: usize) -> Vec<Vec<f32>> {
        self.0.read(frames)
    }

    fn error(&self) -> Option<String> {
        self.0.error()
    }

    fn discard(&mut self) {
        self.0.discard();
    }

    fn stop(&mut self) {
        self.0.stop();
    }
}
