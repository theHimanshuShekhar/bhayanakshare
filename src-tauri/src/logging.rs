//! The log (spec section 8): `tracing` to rolling files in `<data folder>/logs`, a Debug logging
//! switch that takes effect at once, and a panic hook that writes to the same files. The files
//! and the filter live in the core (`LogFiles`, `log_filter`), so the privacy test uses what
//! the app uses; this installs them.
//!
//! Nothing here sends anything anywhere: no telemetry, no crash reporting. The log leaves the
//! machine only when the user exports it (`export_diagnostics`) and passes the zip on.

use std::{io, panic::PanicHookInfo, path::Path, sync::Arc};

use bhayanakshare_core::{LogFiles, log_filter};
use tracing_subscriber::{EnvFilter, fmt, prelude::*, reload};

/// The running log, as the commands see it.
pub struct Logging {
    /// Where the log is written; none if the folder could not be used.
    files: Option<Arc<LogFiles>>,
    /// Switches the filter between normal and debug, in the running subscriber.
    apply_debug: Box<dyn Fn(bool) -> Result<(), String> + Send + Sync>,
}

impl Logging {
    /// Opens the log files in `dir` and makes the log the process's `tracing` subscriber, at
    /// info level. Without a usable folder the app runs without a log file.
    pub fn install(dir: &Path) -> Self {
        let files = match LogFiles::open(dir) {
            Ok(files) => Some(Arc::new(files)),
            Err(e) => {
                eprintln!("could not open the log files in {}: {e}", dir.display());
                None
            }
        };
        let (filter, handle) = reload::Layer::new(EnvFilter::new(log_filter(false)));
        let to_file = files.clone().map(|files| fmt::layer().with_ansi(false).with_writer(files));
        // Where a developer is looking, as before there was a log: only in a debug build.
        let to_stderr = cfg!(debug_assertions).then(|| fmt::layer().with_writer(io::stderr));
        let installed = tracing_subscriber::registry().with(filter).with(to_file).with(to_stderr).try_init();
        if let Err(e) = installed {
            // Something else already installed one (a test, say): leave it be.
            eprintln!("could not install the log: {e}");
        }
        log_panics();
        Self::new(files, move |on| {
            handle.reload(EnvFilter::new(log_filter(on))).map_err(|e| e.to_string())
        })
    }

    /// A log that is not installed anywhere: for when there is nowhere to put one, and for tests
    /// of the commands. `apply_debug` is called when the level is changed.
    pub fn new(
        files: Option<Arc<LogFiles>>,
        apply_debug: impl Fn(bool) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self { files, apply_debug: Box::new(apply_debug) }
    }

    /// Applies the Debug logging setting to the log, now.
    pub fn set_debug(&self, on: bool) -> Result<(), String> {
        (self.apply_debug)(on)
    }

    /// The folder of the log files, for the export; none if there is no log.
    pub fn dir(&self) -> Option<&Path> {
        self.files.as_deref().map(LogFiles::dir)
    }

    /// Makes sure everything logged is in the files, as the app quits.
    pub fn flush(&self) {
        if let Some(files) = &self.files {
            if let Err(e) = files.flush() {
                eprintln!("could not flush the log: {e}");
            }
        }
    }
}

/// Writes a panic to the log, where a crash report would go if there were one, and then does
/// what would have happened anyway. (A panic in the log's own write, which holds a lock, is
/// logged from inside it; `LogFiles` drops that line rather than wait for itself.)
fn log_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("{}", panic_text(info));
        default(info);
    }));
}

/// A panic as the log has it: where, and what it said, if what it said is surely not made of
/// anything the user chose. A message built at run time (what `unwrap` and `expect` make from
/// an error) may hold a file name or a path, and the log never does; the place it was raised
/// from is what finds the cause.
fn panic_text(info: &PanicHookInfo<'_>) -> String {
    let place = info.location().map_or_else(|| "an unknown place".to_owned(), |l| format!("{}:{}", l.file(), l.line()));
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed");
    match info.payload().downcast_ref::<&'static str>() {
        Some(message) => format!("panic in thread '{thread}' at {place}: {message}"),
        None => format!("panic in thread '{thread}' at {place} (the message is not logged: it may name files)"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// What the hook would log for a panic raised with `raise`, which runs on a thread of its own.
    fn logged_for(raise: fn()) -> String {
        static SEEN: Mutex<Option<String>> = Mutex::new(None);
        // Panic hooks are process-wide, so take the default back out for the others when done.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|info| {
            *SEEN.lock().unwrap() = Some(panic_text(info));
        }));
        let result = std::thread::Builder::new().name("worker".into()).spawn(raise).unwrap().join();
        std::panic::set_hook(previous);
        assert!(result.is_err());
        SEEN.lock().unwrap().take().unwrap()
    }

    // One test, because the panic hook is the process's.
    #[test]
    fn a_panic_is_logged_with_where_it_happened_and_a_fixed_message_but_not_one_made_at_run_time() {
        let text = logged_for(|| panic!("the Device cannot be in two places"));
        assert!(text.contains("thread 'worker'"), "{text}");
        assert!(text.contains("logging.rs:"), "{text}");
        assert!(text.ends_with("the Device cannot be in two places"), "{text}");

        let text = logged_for(|| {
            let name = "secret-plan-XYZZY.txt";
            panic!("could not open {name}");
        });
        assert!(text.contains("logging.rs:"), "{text}");
        assert!(!text.contains("secret-plan"), "{text}");
    }
}
