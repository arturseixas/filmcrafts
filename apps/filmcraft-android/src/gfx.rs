//! Graphics start-up on Android: Vulkan, else OpenGL ES.
//!
//! eframe's default instance has both backends and wgpu prefers Vulkan. Some older Vulkan drivers
//! crash during device or surface creation, where no Rust error handling runs, so every launch
//! would die the same way. Each start therefore leaves a marker in the data folder ([`MARKER`])
//! that is removed once two frames have painted; a later start that finds it knows the previous
//! one died before painting and moves to OpenGL ES only, for good ([`STATE`], until the app data is
//! cleared). A start-up error wgpu does report (no adapter, no device) moves on at once.

use std::path::{Path, PathBuf};

use eframe::egui_wgpu::WgpuSetup;
use eframe::wgpu::{self, Backends};

const MARKER: &str = "graphics-starting";
const STATE: &str = "graphics-mode";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Vulkan, or OpenGL ES where there is no Vulkan.
    Default,
    /// OpenGL ES only (a Vulkan driver crashed at start-up).
    Gles,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Default => "default graphics",
            Mode::Gles => "OpenGL ES only",
        }
    }
}

#[derive(Clone)]
pub struct GraphicsStart {
    dir: Option<PathBuf>,
    mode: Mode,
}

impl GraphicsStart {
    /// Decide this start's mode from the data folder's state and leave this start's marker.
    pub fn begin(dir: Option<&Path>) -> GraphicsStart {
        let dir = dir.map(Path::to_path_buf);
        let mut mode = match dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(STATE)).ok()) {
            Some(s) if s.trim() == "gles" => Mode::Gles,
            _ => Mode::Default,
        };
        if let Some(d) = &dir {
            if mode == Mode::Default && d.join(MARKER).exists() {
                log::warn!("the previous start died before its window painted: using OpenGL ES from now on");
                mode = Mode::Gles;
                let _ = std::fs::write(d.join(STATE), "gles");
            }
            let _ = std::fs::write(d.join(MARKER), b"");
        }
        GraphicsStart { dir, mode }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Move to the next mode after a start-up error wgpu reported; false when there is none.
    pub fn step_down(&mut self) -> bool {
        if self.mode == Mode::Gles {
            return false;
        }
        self.mode = Mode::Gles;
        if let Some(d) = &self.dir {
            let _ = std::fs::write(d.join(STATE), "gles");
        }
        true
    }

    /// The window painted: this start did not die.
    pub fn confirm(&self) {
        if let Some(d) = &self.dir {
            let _ = std::fs::remove_file(d.join(MARKER));
        }
    }

    /// Back to the default mode (the notice's button), from the next start on.
    pub fn reset_to_default(&self) -> Result<(), String> {
        match &self.dir {
            Some(d) => match std::fs::remove_file(d.join(STATE)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            },
            None => Ok(()),
        }
    }

    pub fn apply(&self, options: &mut eframe::NativeOptions) {
        let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup else {
            return;
        };
        // nothing draws indirectly: skip wgpu's indirect-argument validation shader (see the
        // desktop app's graphics.rs)
        create.instance_descriptor.flags = create.instance_descriptor.flags.difference(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL).with_env();
        create.power_preference = wgpu::PowerPreference::HighPerformance;
        if self.mode == Mode::Gles {
            create.instance_descriptor.backends = Backends::GL;
        }
    }
}
