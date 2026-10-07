//! What is logged and where it goes (spec section 8). The shell installs the subscriber; this
//! module holds what the app and the privacy test must agree on (the filter), the log files
//! themselves, and the diagnostics export.
//!
//! The log is local: it is written to files in the app data folder and leaves the machine only
//! when the user exports it and sends it somewhere.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use tracing_appender::rolling::{RollingFileAppender, Rotation};

use crate::db::Db;

/// The log files are named `<LOG_PREFIX>.<YYYY-MM-DD>.<LOG_SUFFIX>`, one a day.
const LOG_PREFIX: &str = "bhayanakshare";
const LOG_SUFFIX: &str = "log";

/// How many days' files are kept: this many, counting today's.
const KEEP_FILES: usize = 7;

/// The most the log files may hold altogether, in bytes.
pub const MAX_LOG_BYTES: u64 = 50_000_000;

/// How much is written between looks at the size of the log folder.
const CHECK_EVERY_BYTES: u64 = 1 << 20;

/// While the log is full (today's file alone is over the cap), how long to wait between looks at
/// whether it has room again.
const FULL_RECHECK: Duration = Duration::from_secs(60);

/// The setting that holds whether debug logging is on: `"1"` or `"0"`. Off when unset.
pub(crate) const DEBUG_SETTING: &str = "debug_logging";

/// Whether debug logging is on, as stored.
pub(crate) async fn load_debug_setting(db: &Db) -> bool {
    match db.setting(DEBUG_SETTING).await {
        Ok(value) => value.as_deref() == Some("1"),
        Err(e) => {
            tracing::warn!("could not read the debug logging setting: {e}");
            false
        }
    }
}

/// The filter directive for the log: this app's own crates at info, or at debug while the
/// "Debug logging" setting is on, and every dependency at warn, or info while it is on.
///
/// A dependency is never raised to debug: iroh and its neighbours print other Devices' full
/// endpoint IDs, and which of their lines do is not ours to audit. Our own lines name a Device
/// only by its Fingerprint, and never log a file name, a folder name or text, at any level.
pub fn log_filter(debug: bool) -> String {
    let (own, others) = if debug { ("debug", "info") } else { ("info", "warn") };
    format!("{others},bhayanakshare_core={own},bhayanakshare_lib={own},bhayanakshare={own}")
}

/// The rolling log files in one folder: a new file each day (UTC), the last [`KEEP_FILES`]
/// kept, and the folder never holding more than [`MAX_LOG_BYTES`] for long.
///
/// Writes go straight to the file, one `write` call per log line, under a lock, so that a
/// line (a panic's, say) is on its way to the disk before the call returns and an export reads
/// everything logged so far. A background writer would batch more, but there is no way to wait
/// for it short of dropping it, and a log line is small and rare next to what a Transfer does.
///
/// The cap is kept by looking at the folder every megabyte written: the oldest files go until
/// what is left fits, but today's file is never deleted. If today's file alone is over the cap,
/// nothing more is written to it until it has room, which is when the next day's file starts
/// and it is deleted. Lines dropped that way are not counted.
pub struct LogFiles {
    dir: PathBuf,
    cap: u64,
    state: Mutex<State>,
}

struct State {
    appender: RollingFileAppender,
    written_since_check: u64,
    /// Set while today's file is over the cap, with when the folder was last looked at.
    full_since: Option<Instant>,
}

impl LogFiles {
    /// Opens today's file in `dir` (made if it is not there), and deletes what is over the
    /// limits.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::open_with_cap(dir, MAX_LOG_BYTES)
    }

    fn open_with_cap(dir: &Path, cap: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(LOG_PREFIX)
            .filename_suffix(LOG_SUFFIX)
            .max_log_files(KEEP_FILES)
            .build(dir)
            .map_err(io::Error::other)?;
        let logs = Self {
            dir: dir.to_owned(),
            cap,
            state: Mutex::new(State { appender, written_since_check: 0, full_since: None }),
        };
        // Files left by an older run may already be over the cap.
        let total = prune_to_cap(dir, cap)?;
        logs.lock().full_since = (total >= cap).then(Instant::now);
        Ok(logs)
    }

    /// Where the files are.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Makes sure everything logged so far is in the files. Writes are not buffered here, so
    /// this only has the file handle flushed; it is for what reads the files next.
    pub fn flush(&self) -> io::Result<()> {
        self.lock().appender.flush()
    }
}

/// Written to as `&LogFiles`, so that an `Arc<LogFiles>` is a `MakeWriter` for the `fmt` layer.
impl Write for &LogFiles {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.lock();
        if let Some(since) = state.full_since {
            if since.elapsed() < FULL_RECHECK {
                return Ok(buf.len());
            }
            let total = prune_to_cap(&self.dir, self.cap)?;
            state.full_since = (total >= self.cap).then(Instant::now);
            if state.full_since.is_some() {
                return Ok(buf.len());
            }
        }
        let n = state.appender.write(buf)?;
        state.written_since_check += n as u64;
        if state.written_since_check >= CHECK_EVERY_BYTES {
            state.written_since_check = 0;
            let total = prune_to_cap(&self.dir, self.cap)?;
            state.full_since = (total >= self.cap).then(Instant::now);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        LogFiles::flush(self)
    }
}

/// The log files in `dir`, oldest first: their names hold the date, so by name is by day.
fn log_files(dir: &Path) -> io::Result<Vec<(PathBuf, u64)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let meta = entry.metadata()?;
        if meta.is_file()
            && name.starts_with(&format!("{LOG_PREFIX}."))
            && name.ends_with(&format!(".{LOG_SUFFIX}"))
        {
            files.push((entry.path(), meta.len()));
        }
    }
    files.sort();
    Ok(files)
}

/// Deletes the oldest log files in `dir` until what is left is at most `cap` bytes, or only the
/// newest file is left, which is never deleted. Returns how many bytes the files hold now. A
/// folder that is not there holds none.
fn prune_to_cap(dir: &Path, cap: u64) -> io::Result<u64> {
    let files = match log_files(dir) {
        Ok(files) => files,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut total: u64 = files.iter().map(|(_, len)| len).sum();
    let newest = files.len().saturating_sub(1);
    for (path, len) in files.into_iter().take(newest) {
        if total <= cap {
            break;
        }
        match fs::remove_file(&path) {
            Ok(()) => total -= len,
            // Gone already: not counted any more either.
            Err(e) if e.kind() == io::ErrorKind::NotFound => total -= len,
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

/// Writes a zip of the log files in `logs` and `about` (as `about.txt`) to `dest`: all the user
/// has to hand over when something goes wrong. Nothing else from the machine goes in it.
/// A partly written zip is deleted if this fails.
pub(crate) fn write_diagnostics_zip(logs: &Path, dest: &Path, about: &str) -> io::Result<()> {
    let result = (|| {
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut zip = zip::ZipWriter::new(File::create(dest)?);
        zip.start_file("about.txt", options).map_err(io::Error::other)?;
        zip.write_all(about.as_bytes())?;
        for (path, _) in log_files(logs).or_else(|e| match e.kind() {
            io::ErrorKind::NotFound => Ok(Vec::new()),
            _ => Err(e),
        })? {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("log");
            zip.start_file(format!("logs/{name}"), options).map_err(io::Error::other)?;
            io::copy(&mut File::open(&path)?, &mut zip)?;
        }
        zip.finish().map_err(io::Error::other)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

/// The operating system and its version, as one line for `about.txt`.
pub(crate) fn os_description() -> String {
    let version = os_version().unwrap_or_else(|| "unknown version".to_owned());
    format!("{} ({version}), {}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(target_os = "linux")]
fn os_version() -> Option<String> {
    let release = fs::read_to_string("/etc/os-release").ok()?;
    let name = release
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|name| name.trim().trim_matches('"').to_owned());
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").ok().map(|k| format!("kernel {}", k.trim()));
    match (name, kernel) {
        (Some(name), Some(kernel)) => Some(format!("{name}, {kernel}")),
        (name, kernel) => name.or(kernel),
    }
}

#[cfg(target_os = "macos")]
fn os_version() -> Option<String> {
    command_output("sw_vers", &["-productVersion"])
}

#[cfg(target_os = "windows")]
fn os_version() -> Option<String> {
    command_output("cmd", &["/C", "ver"])
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn os_version() -> Option<String> {
    None
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn write_file(dir: &Path, name: &str, len: usize) {
        fs::write(dir.join(name), vec![b'x'; len]).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        log_files(dir).unwrap().into_iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_filter_keeps_dependencies_quieter_than_the_app_and_never_at_debug() {
        let quiet = log_filter(false);
        assert_eq!(quiet, "warn,bhayanakshare_core=info,bhayanakshare_lib=info,bhayanakshare=info");
        let loud = log_filter(true);
        assert_eq!(loud, "info,bhayanakshare_core=debug,bhayanakshare_lib=debug,bhayanakshare=debug");
        // Parses as a filter, with the dependencies' default level first.
        for filter in [quiet, loud] {
            assert!(filter.split(',').next().is_some_and(|level| !level.contains('=')));
        }
    }

    #[test]
    fn pruning_deletes_the_oldest_files_until_the_rest_fit() {
        let tmp = tempfile::tempdir().unwrap();
        for day in 1..=5 {
            write_file(tmp.path(), &format!("bhayanakshare.2026-10-0{day}.log"), 100);
        }
        write_file(tmp.path(), "notes.txt", 10_000);

        let total = prune_to_cap(tmp.path(), 250).unwrap();

        assert_eq!(total, 200);
        assert_eq!(names(tmp.path()), ["bhayanakshare.2026-10-04.log", "bhayanakshare.2026-10-05.log"]);
        // What is not a log file is left alone, and not counted.
        assert!(tmp.path().join("notes.txt").exists());
    }

    #[test]
    fn pruning_never_deletes_the_newest_file_even_when_it_alone_is_over_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "bhayanakshare.2026-10-01.log", 100);
        write_file(tmp.path(), "bhayanakshare.2026-10-02.log", 500);

        assert_eq!(prune_to_cap(tmp.path(), 250).unwrap(), 500);
        assert_eq!(names(tmp.path()), ["bhayanakshare.2026-10-02.log"]);
    }

    #[test]
    fn pruning_a_folder_that_is_not_there_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(prune_to_cap(&tmp.path().join("nope"), 10).unwrap(), 0);
    }

    #[test]
    fn opening_prunes_what_an_older_run_left_over_the_cap_and_keeps_seven_days() {
        let tmp = tempfile::tempdir().unwrap();
        for day in 1..=9 {
            write_file(tmp.path(), &format!("bhayanakshare.2026-09-0{day}.log"), 100);
        }
        let logs = LogFiles::open_with_cap(tmp.path(), 450).unwrap();
        // Today's file is made on the first write.
        (&logs).write_all(b"hello\n").unwrap();

        let kept = names(tmp.path());
        let total: u64 = log_files(tmp.path()).unwrap().iter().map(|(_, len)| len).sum();
        assert!(total <= 450 + 6, "{total} bytes in {kept:?}");
        assert!(kept.len() <= KEEP_FILES, "{kept:?}");
        // (Which of the old files the count leaves is up to the appender, which goes by when each
        // was created; here they were all made just now.)
        assert!(kept.last().is_some_and(|name| name != "bhayanakshare.2026-09-09.log"), "today's file: {kept:?}");
    }

    #[test]
    fn writing_past_the_cap_deletes_older_files_then_stops_writing() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "bhayanakshare.2000-01-01.log", 1000);
        // A cap far under what one megabyte-sized look at the folder allows.
        let logs = LogFiles::open_with_cap(tmp.path(), 4 * CHECK_EVERY_BYTES).unwrap();
        let chunk = vec![b'y'; CHECK_EVERY_BYTES as usize];
        let mut writer = &logs;
        for _ in 0..10 {
            writer.write_all(&chunk).unwrap();
        }

        let files = log_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 1, "the old file is gone: {files:?}");
        let len = files[0].1;
        assert!(len >= 4 * CHECK_EVERY_BYTES, "{len}");
        assert!(len <= 5 * CHECK_EVERY_BYTES, "writing stopped once the cap was reached: {len}");
    }

    #[test]
    fn a_line_is_in_the_file_as_soon_as_it_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = LogFiles::open(tmp.path()).unwrap();
        (&logs).write_all(b"first line\n").unwrap();

        let (path, _) = log_files(tmp.path()).unwrap().remove(0);
        assert_eq!(fs::read_to_string(path).unwrap(), "first line\n");
    }

    #[test]
    fn the_diagnostics_zip_holds_about_and_every_log_file_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        fs::create_dir(&logs).unwrap();
        fs::write(logs.join("bhayanakshare.2026-10-01.log"), "old line\n").unwrap();
        fs::write(logs.join("bhayanakshare.2026-10-02.log"), "new line\n").unwrap();
        fs::write(logs.join("unrelated.txt"), "not a log").unwrap();
        let dest = tmp.path().join("out.zip");

        write_diagnostics_zip(&logs, &dest, "App version: 1.2.3\n").unwrap();

        let mut zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let mut entries: Vec<String> = zip.file_names().map(str::to_owned).collect();
        entries.sort();
        assert_eq!(entries, ["about.txt", "logs/bhayanakshare.2026-10-01.log", "logs/bhayanakshare.2026-10-02.log"]);
        let mut about = String::new();
        zip.by_name("about.txt").unwrap().read_to_string(&mut about).unwrap();
        assert_eq!(about, "App version: 1.2.3\n");
        let mut line = String::new();
        zip.by_name("logs/bhayanakshare.2026-10-02.log").unwrap().read_to_string(&mut line).unwrap();
        assert_eq!(line, "new line\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_export_leaves_no_partial_zip() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        fs::create_dir(&logs).unwrap();
        let unreadable = logs.join("bhayanakshare.2026-10-01.log");
        fs::write(&unreadable, "line\n").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let dest = tmp.path().join("out.zip");

        // As root the file would be readable anyway, and there is nothing to test.
        if File::open(&unreadable).is_err() {
            assert!(write_diagnostics_zip(&logs, &dest, "about").is_err());
            assert!(!dest.exists(), "a partly written zip is left behind");
        }
        // A destination that cannot be made is an error too.
        let missing = tmp.path().join("no-such-folder").join("out.zip");
        assert!(write_diagnostics_zip(&logs, &missing, "about").is_err());
    }

    #[test]
    fn the_os_description_names_the_system_and_architecture() {
        let text = os_description();
        assert!(text.starts_with(std::env::consts::OS), "{text}");
        assert!(text.ends_with(std::env::consts::ARCH), "{text}");
    }
}
