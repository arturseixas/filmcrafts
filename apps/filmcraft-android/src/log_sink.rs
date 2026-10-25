//! The Android app's logger: `log::` records go to logcat (`adb logcat -s FilmCraft`) and to
//! `<data dir>/Logs/filmcraft.log`, with the desktop app's filter, rotation and size cap
//! (`logging.rs`, shared). Standard error goes nowhere on Android, so logcat takes its place.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use crate::logging::{DEFAULT_FILTER, Filter, MAX_FILE_BYTES, Sink, format_line, rotate, timestamp};

pub struct AndroidLogger {
    filter: Filter,
    logcat: android_logger::AndroidLogger,
    sink: Mutex<Sink>,
}

impl AndroidLogger {
    /// Rotate the logs in `dir` and send records to the fresh file as well; returns its path.
    pub fn attach_dir(&self, dir: &Path) -> Result<PathBuf, String> {
        match rotate(dir) {
            Ok((path, file)) => {
                self.sink.lock().unwrap_or_else(PoisonError::into_inner).attach(file);
                Ok(path)
            }
            Err(e) => {
                self.sink.lock().unwrap_or_else(PoisonError::into_inner).close();
                Err(format!("{}: {e}", dir.display()))
            }
        }
    }
}

impl log::Log for AndroidLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.filter.level_for(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        self.logcat.log(record);
        let thread = std::thread::current();
        let line = format_line(&timestamp(SystemTime::now()), record.level(), thread.name().unwrap_or("?"), record.target(), &record.args().to_string());
        self.sink.lock().unwrap_or_else(PoisonError::into_inner).write(&line);
    }

    fn flush(&self) {}
}

/// Install the logger; `None` when one was installed already (android_main runs again when the
/// activity is recreated in the same process).
pub fn install() -> Option<&'static AndroidLogger> {
    static LOGGER: OnceLock<AndroidLogger> = OnceLock::new();
    let filter = Filter::parse(DEFAULT_FILTER);
    let max = filter.max();
    let logger = LOGGER.get_or_init(|| AndroidLogger {
        filter,
        // the filter above decides; logcat takes every record it is given
        logcat: android_logger::AndroidLogger::new(android_logger::Config::default().with_tag("FilmCraft").with_max_level(log::LevelFilter::Trace)),
        sink: Mutex::new(Sink::new(MAX_FILE_BYTES)),
    });
    log::set_logger(logger).ok()?;
    log::set_max_level(max);
    Some(logger)
}
