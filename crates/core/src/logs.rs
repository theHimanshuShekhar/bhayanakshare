//! What is logged and where it goes (spec section 8). The shell installs the subscriber; this
//! module holds what the app and the privacy test must agree on (the filter), the log files
//! themselves, and the diagnostics export.
//!
//! The log is local: it is written to files in the app data folder and leaves the machine only
//! when the user exports it and sends it somewhere.

use std::{
    borrow::Cow,
    cell::Cell,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::db::Db;

/// The log files are named `<LOG_PREFIX>.<YYYY-MM-DD>.<LOG_SUFFIX>`, one a day (UTC).
const LOG_PREFIX: &str = "bhayanakshare";
const LOG_SUFFIX: &str = "log";

/// How many days' files are kept: this many, counting today's.
const KEEP_FILES: usize = 7;

/// About the most the log files hold altogether, in bytes: the folder is looked at every
/// [`CHECK_EVERY_BYTES`], so it can be over by that much.
pub const MAX_LOG_BYTES: u64 = 50_000_000;

/// How much is written between looks at the size of the log folder.
const CHECK_EVERY_BYTES: u64 = 1 << 20;

/// How long writing stops, when it must (the log is full, or its file cannot be written), before
/// it looks again at whether it can go on.
const PAUSE: Duration = Duration::from_secs(60);

/// The setting that holds whether debug logging is on: `"1"` or `"0"`. Off when unset.
pub(crate) const SETTING: &str = "debug_logging";

/// Whether debug logging is on, as stored.
pub(crate) async fn load(db: &Db) -> bool {
    match db.setting(SETTING).await {
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
/// endpoint IDs, and which of their lines do is not ours to audit. Two are known to print them
/// even at info or warn, and are held down in either mode: the DHT lookup (`resolving <ID>`, the
/// endpoint info it found) is held to warn, and iroh's pkarr publisher (its `warn` carries the
/// URL it failed to reach, which ends in this Device's own ID) is off. [`LogFiles`] redacts
/// anything shaped like an ID as well, for the lines nobody knows about.
///
/// Our own lines name a Device only by its Fingerprint, and never log a file name, a folder name
/// or text, at any level.
pub fn log_filter(debug: bool) -> String {
    let (own, others) = if debug { ("debug", "info") } else { ("info", "warn") };
    format!(
        "{others},bhayanakshare_core={own},bhayanakshare_lib={own},bhayanakshare={own},\
         iroh_mainline_address_lookup=warn,iroh::address_lookup::pkarr=off"
    )
}

/// Replaces what looks like a Device ID in a log line with a stand-in: 52 characters of base32
/// (the Device ID; the stand-in keeps its Fingerprint), 52 of z-base-32 (the form iroh's pkarr
/// and DNS lookups put in URLs) and 64 of hex (iroh's own form of the ID). The 64 hex digits are
/// found inside a longer run too; the others only as a word of their own, so that a longer word
/// is not taken for an ID. A blob hash is 64 hex digits as well, and goes the same way.
///
/// This is for what no one has read the logging of: our own lines never hold an ID to begin with
/// (see [`log_filter`]). It looks at one line at a time, as the `fmt` layer writes them.
fn redact_ids(line: &[u8]) -> Cow<'_, [u8]> {
    const Z32: &[u8] = b"ybndrfg8ejkmcpqxot1uwisza345h769";
    let mut out: Option<Vec<u8>> = None;
    let mut copied = 0; // `line[..copied]` is in `out`, redacted
    let mut i = 0;
    while i < line.len() {
        if !line[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let end = line[i..].iter().position(|c| !c.is_ascii_alphanumeric()).map_or(line.len(), |n| i + n);
        let word = &line[i..end];
        let mut replace = |range: std::ops::Range<usize>, stand_in: &[u8]| {
            let out = out.get_or_insert_with(Vec::new);
            out.extend_from_slice(&line[copied..range.start]);
            out.extend_from_slice(stand_in);
            copied = range.end;
        };
        if word.len() == DEVICE_ID_CHARS && word.iter().all(|c| matches!(c, b'A'..=b'Z' | b'a'..=b'z' | b'2'..=b'7')) {
            let hint = word[..8].to_ascii_uppercase();
            let stand_in = [b"<id:", &hint[..4], b"-", &hint[4..], b">"].concat();
            replace(i..end, &stand_in);
        } else if word.len() == DEVICE_ID_CHARS && word.iter().all(|c| Z32.contains(&c.to_ascii_lowercase())) {
            replace(i..end, b"<redacted>");
        } else {
            // Hex digits, 64 or more in a row, wherever they sit in the word.
            let mut j = 0;
            while j < word.len() {
                let run = word[j..].iter().position(|c| !c.is_ascii_hexdigit()).unwrap_or(word.len() - j);
                if run >= 64 {
                    replace(i + j..i + j + run, b"<redacted>");
                }
                j += run.max(1);
            }
        }
        i = end;
    }
    match out {
        None => Cow::Borrowed(line),
        Some(mut out) => {
            out.extend_from_slice(&line[copied..]);
            Cow::Owned(out)
        }
    }
}

/// How many characters of base32 a Device ID is, as its own word.
const DEVICE_ID_CHARS: usize = crate::identity::DEVICE_ID_LEN;

thread_local! {
    /// Set while this thread is inside [`LogFiles`]'s `write`.
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

/// Held while this thread is in `write`; `enter` fails if it already is, which means something
/// logged from inside the write (a panic's hook, on a panic in it, says so while the lock is
/// still held, and would wait for itself).
struct Writing;

impl Writing {
    fn enter() -> Option<Self> {
        WRITING.with(|w| (!w.replace(true)).then_some(Self))
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        WRITING.with(|w| w.set(false));
    }
}

/// The log files in one folder: a new file each day (UTC), the last [`KEEP_FILES`] kept, and the
/// folder holding about [`MAX_LOG_BYTES`] at the most. Anything that looks like a Device ID is
/// redacted from a line on its way in (see [`redact_ids`]).
///
/// Writes go straight to the file, one `write` call per log line, under a lock, so that a line (a
/// panic's, say) is on its way to the disk before the call returns and an export reads
/// everything logged so far. A background writer would batch more, but there is no way to wait
/// for it short of dropping it, and a log line is small and rare next to what a Transfer does.
///
/// The cap is kept by looking at the folder every megabyte written: the oldest files go until
/// what is left fits, but today's file is never deleted. If today's file alone is over the cap,
/// the lines that follow are dropped (and not counted) until the next day, whose file is a new,
/// empty one. A file that cannot be written or pruned never makes a `write` fail: the line is
/// dropped, and it is tried again a minute (or a megabyte) later.
pub struct LogFiles {
    dir: PathBuf,
    cap: u64,
    /// The day today is, as it is in file names; a field so that tests can change the day.
    today: Box<dyn Fn() -> String + Send + Sync>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The day `file` is for, and the file: today's, once it has been opened.
    day: String,
    file: Option<File>,
    written_since_check: u64,
    /// While set, and in the future, lines are dropped.
    paused_until: Option<Instant>,
}

impl LogFiles {
    /// Opens today's file in `dir` (made if it is not there), and deletes what is over the
    /// limits.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::open_with(dir, MAX_LOG_BYTES, Box::new(today_utc))
    }

    fn open_with(dir: &Path, cap: u64, today: Box<dyn Fn() -> String + Send + Sync>) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let logs = Self { dir: dir.to_owned(), cap, today, state: Mutex::default() };
        let mut state = logs.lock();
        let day = (logs.today)();
        logs.begin_day(&mut state, day);
        if state.file.is_none() {
            return Err(io::Error::other("could not open today's log file"));
        }
        drop(state);
        Ok(logs)
    }

    /// Where the files are.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Makes sure everything logged so far is in the file. Writes are not buffered here, so
    /// this only has the file handle flushed; it is for what reads the files next.
    pub fn flush(&self) -> io::Result<()> {
        match self.lock().file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }

    /// Starts `day`: opens its file, which has room again, and prunes the folder.
    fn begin_day(&self, state: &mut State, day: String) {
        let name = file_name(&day);
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // The log is for the user and whoever they hand it to, not for the other users of the machine.
            options.mode(0o600);
        }
        state.day = day;
        state.written_since_check = 0;
        state.paused_until = None;
        state.file = options.open(self.dir.join(&name)).ok();
        if state.file.is_some() {
            self.check_size(state);
        } else {
            state.paused_until = Some(Instant::now() + PAUSE);
        }
    }

    /// Prunes the folder, and stops writing for a while if it is still full.
    fn check_size(&self, state: &mut State) {
        state.written_since_check = 0;
        state.paused_until = match prune(&self.dir, self.cap, &file_name(&state.day)) {
            Ok(total) if total > self.cap => Some(Instant::now() + PAUSE),
            // Whether it could be looked at or not, writing goes on: a folder that cannot be
            // pruned is tried again after the next megabyte.
            Ok(_) | Err(_) => None,
        };
    }

    fn write_line(&self, line: &[u8]) {
        let mut state = self.lock();
        let day = (self.today)();
        if state.day != day {
            self.begin_day(&mut state, day);
        } else if state.paused_until.is_some_and(|until| Instant::now() >= until) {
            if state.file.is_none() {
                let day = state.day.clone();
                self.begin_day(&mut state, day);
            } else {
                self.check_size(&mut state);
            }
        }
        if state.paused_until.is_some() {
            return;
        }
        let Some(file) = state.file.as_mut() else { return };
        if file.write_all(line).is_err() {
            state.paused_until = Some(Instant::now() + PAUSE);
            return;
        }
        state.written_since_check += line.len() as u64;
        if state.written_since_check >= CHECK_EVERY_BYTES {
            self.check_size(&mut state);
        }
    }
}

/// Written to as `&LogFiles`, so that an `Arc<LogFiles>` is a `MakeWriter` for the `fmt` layer.
impl Write for &LogFiles {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // From inside a write (see `Writing`): dropped.
        if let Some(_writing) = Writing::enter() {
            self.write_line(&redact_ids(buf));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        LogFiles::flush(self)
    }
}

fn file_name(day: &str) -> String {
    format!("{LOG_PREFIX}.{day}.{LOG_SUFFIX}")
}

/// Today, UTC, as `YYYY-MM-DD`.
fn today_utc() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_day(secs / 86_400)
}

/// The date `days` days after 1970-01-01, as `YYYY-MM-DD` (Howard Hinnant's `civil_from_days`).
fn civil_day(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The log files in `dir`, oldest first: their names hold the date, so by name is by day. A
/// folder that is not there holds none.
fn log_files(dir: &Path) -> io::Result<Vec<(PathBuf, u64)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // A file that went between listing and looking at it is not there.
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() && name.starts_with(&format!("{LOG_PREFIX}.")) && name.ends_with(&format!(".{LOG_SUFFIX}")) {
            files.push((entry.path(), meta.len()));
        }
    }
    files.sort();
    Ok(files)
}

/// Deletes the oldest log files in `dir` until at most [`KEEP_FILES`] are left and they hold at
/// most `cap` bytes, never the one called `current`, which is being written. Returns how many
/// bytes the files hold now.
fn prune(dir: &Path, cap: u64, current: &str) -> io::Result<u64> {
    let files = log_files(dir)?;
    let mut total: u64 = files.iter().map(|(_, len)| len).sum();
    let mut count = files.len();
    for (path, len) in files {
        if count <= KEEP_FILES && total <= cap {
            break;
        }
        if path.file_name().is_some_and(|name| name == current) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            // Gone already: not counted any more either.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        total -= len;
        count -= 1;
    }
    Ok(total)
}

/// Writes a zip of the log files in `logs` and `about` (as `about.txt`) to `dest`: all the user
/// has to hand over when something goes wrong. Nothing else from the machine goes in it. A file
/// that is pruned while the zip is made is left out. A partly written zip is deleted if this
/// fails.
pub(crate) fn write_diagnostics_zip(logs: Option<&Path>, dest: &Path, about: &str) -> io::Result<()> {
    let result = (|| {
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut zip = zip::ZipWriter::new(File::create(dest)?);
        zip.start_file("about.txt", options).map_err(io::Error::other)?;
        zip.write_all(about.as_bytes())?;
        for (path, _) in logs.map_or(Ok(Vec::new()), log_files)? {
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("log");
            zip.start_file(format!("logs/{name}"), options).map_err(io::Error::other)?;
            io::copy(&mut file, &mut zip)?;
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
    use std::{
        io::Read,
        sync::{Arc, OnceLock},
    };

    use tracing_subscriber::{EnvFilter, prelude::*};

    use super::*;

    const MIB: usize = 1 << 20;

    fn write_file(dir: &Path, name: &str, len: usize) {
        fs::write(dir.join(name), vec![b'x'; len]).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        log_files(dir).unwrap().into_iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    /// Logs whose day is whatever the test sets.
    fn logs_on(dir: &Path, cap: u64, day: &Arc<Mutex<String>>) -> LogFiles {
        let day = day.clone();
        LogFiles::open_with(dir, cap, Box::new(move || day.lock().unwrap().clone())).unwrap()
    }

    fn day(text: &str) -> Arc<Mutex<String>> {
        Arc::new(Mutex::new(text.to_owned()))
    }

    #[test]
    fn the_filter_keeps_dependencies_quieter_than_the_app_and_never_at_debug() {
        for (debug, own, others) in [(false, "info", "warn"), (true, "debug", "info")] {
            let filter = log_filter(debug);
            assert!(filter.starts_with(&format!("{others},")), "{filter}");
            for target in ["bhayanakshare_core", "bhayanakshare_lib", "bhayanakshare"] {
                assert!(filter.contains(&format!("{target}={own}")), "{filter}");
            }
            // Held down in both modes: what prints a whole ID at info and warn.
            assert!(filter.contains("iroh_mainline_address_lookup=warn"), "{filter}");
            assert!(filter.contains("iroh::address_lookup::pkarr=off"), "{filter}");
            // Nothing is ever asked for at debug but our own.
            assert!(!filter.replace(&format!("={own}"), "").contains("=debug"), "{filter}");
            EnvFilter::try_new(&filter).unwrap();
        }
    }

    /// What `log_filter` lets through, as lines "target level".
    fn passed(debug: bool, events: fn()) -> String {
        let lines = Arc::new(Mutex::new(Vec::new()));
        struct Collect(Arc<Mutex<Vec<String>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Collect {
            fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
                let meta = event.metadata();
                self.0.lock().unwrap().push(format!("{} {}", meta.target(), meta.level()));
            }
        }
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new(log_filter(debug)))
            .with(Collect(lines.clone()));
        tracing::subscriber::with_default(subscriber, events);
        let lines = lines.lock().unwrap().join("\n");
        lines
    }

    #[test]
    fn the_filter_lets_the_known_leakers_through_only_for_what_cannot_hold_an_id() {
        let events = || {
            tracing::info!(target: "iroh_mainline_address_lookup", "resolving an ID");
            tracing::warn!(target: "iroh_mainline_address_lookup", "pkarr publish error");
            tracing::warn!(target: "iroh::address_lookup::pkarr", "Failed to publish to pkarr");
            tracing::error!(target: "iroh::address_lookup::pkarr", "Failed to publish to pkarr");
            tracing::warn!(target: "iroh::socket", "something else");
            tracing::info!(target: "iroh::socket", "something else");
            tracing::debug!(target: "iroh::socket", "something else");
        };
        for debug in [false, true] {
            let lines = passed(debug, events);
            assert!(!lines.contains("pkarr"), "debug {debug}: {lines}");
            assert!(!lines.contains("iroh_mainline_address_lookup INFO"), "debug {debug}: {lines}");
            assert!(lines.contains("iroh_mainline_address_lookup WARN"), "debug {debug}: {lines}");
            assert!(lines.contains("iroh::socket WARN"), "debug {debug}: {lines}");
            assert!(!lines.contains("iroh::socket DEBUG"), "debug {debug}: {lines}");
            assert_eq!(lines.contains("iroh::socket INFO"), debug, "debug {debug}: {lines}");
        }
    }

    fn redacted(line: &str) -> String {
        String::from_utf8(redact_ids(line.as_bytes()).into_owned()).unwrap()
    }

    #[test]
    fn a_device_id_in_any_of_its_spellings_is_redacted_and_the_rest_of_the_line_is_not() {
        let base32 = "K3QF7XNABCDEFGHIJKLMNOPQRSTUVWXYZ234567ABCDEFGHIJKLM";
        assert_eq!(base32.len(), 52);
        let z32 = "ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1u";
        assert_eq!(z32.len(), 52);
        let hex = "58f2ccbf91ef9b2d1b19c6cc84df96146fbdba81ea1391c7c6e828e31390b6ef";
        assert_eq!(hex.len(), 64);

        assert_eq!(redacted(&format!("WARN iroh: no route to {base32}!")), "WARN iroh: no route to <id:K3QF-7XNA>!");
        assert_eq!(
            redacted(&format!("WARN to {}", base32.to_lowercase())),
            "WARN to <id:K3QF-7XNA>"
        );
        assert_eq!(
            redacted(&format!("Failed to publish: https://dns.iroh.link/pkarr/{z32}: timed out")),
            "Failed to publish: https://dns.iroh.link/pkarr/<redacted>: timed out"
        );
        assert_eq!(redacted(&format!("peer=PublicKey({hex}) hash={hex}")), "peer=PublicKey(<redacted>) hash=<redacted>");
        // Hex inside a longer word goes too, and so does a run longer than an ID.
        assert_eq!(redacted(&format!("x{hex}y")), "x<redacted>y");
        assert_eq!(redacted(&format!("{hex}{hex}")), "<redacted>");
    }

    #[test]
    fn a_fingerprint_a_transfer_id_and_an_ordinary_line_are_left_as_they_are() {
        for line in [
            "2026-10-07T04:29:54.621823Z  WARN bhayanakshare_core::receiver: Offer refused: Couldn't be sent peer=K3QF-7XNA\n",
            "transfer=a01666f34c396bea694590daf5d08906 control connection lost\n",
            // Words that are not 52 characters long (and not 64 or more hex digits) are not IDs.
            "KKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKK is 53 characters\n",
            "KKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKK is 51 characters\n",
            "no ID here: caf\u{e9} \u{1f980}\n",
            "",
        ] {
            assert_eq!(redacted(line), line);
        }
        // Borrowed, not copied, when there is nothing to change.
        assert!(matches!(redact_ids(b"just a line\n"), Cow::Borrowed(_)));
    }

    #[test]
    fn the_file_never_holds_a_device_id_whatever_wrote_the_line() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = LogFiles::open(tmp.path()).unwrap();
        let z32 = "ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1u";
        // What iroh's pkarr publisher logs while offline, at warn.
        let line = format!(
            "2026-10-07T04:29:54Z  WARN iroh::address_lookup::pkarr: Failed to publish to pkarr err=error sending request for url (https://dns.iroh.link/pkarr/{z32}) url=https://dns.iroh.link/pkarr\n"
        );
        (&logs).write_all(line.as_bytes()).unwrap();

        let (path, _) = log_files(tmp.path()).unwrap().remove(0);
        let written = fs::read_to_string(path).unwrap();
        assert!(!written.contains(z32), "{written}");
        assert!(written.contains("https://dns.iroh.link/pkarr/<redacted>)"), "{written}");
        assert!(written.contains("Failed to publish to pkarr"), "{written}");
    }

    #[test]
    fn days_are_worked_out_from_the_epoch() {
        assert_eq!(civil_day(0), "1970-01-01");
        assert_eq!(civil_day(10_957), "2000-01-01");
        assert_eq!(civil_day(11_017), "2000-03-01");
        assert_eq!(civil_day(19_782), "2024-02-29");
        assert_eq!(civil_day(20_368), "2025-10-07");
        assert_eq!(civil_day(20_733), "2026-10-07");
        assert_eq!(today_utc().len(), 10);
    }

    #[test]
    fn pruning_deletes_the_oldest_files_until_the_rest_fit() {
        let tmp = tempfile::tempdir().unwrap();
        for day in 1..=5 {
            write_file(tmp.path(), &format!("bhayanakshare.2026-10-0{day}.log"), 100);
        }
        write_file(tmp.path(), "notes.txt", 10_000);

        let total = prune(tmp.path(), 250, "bhayanakshare.2026-10-05.log").unwrap();

        assert_eq!(total, 200);
        assert_eq!(names(tmp.path()), ["bhayanakshare.2026-10-04.log", "bhayanakshare.2026-10-05.log"]);
        // What is not a log file is left alone, and not counted.
        assert!(tmp.path().join("notes.txt").exists());
    }

    #[test]
    fn pruning_keeps_seven_files_and_never_the_current_one_even_when_it_is_over_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        for day in 1..=9 {
            write_file(tmp.path(), &format!("bhayanakshare.2026-09-0{day}.log"), 10);
        }
        let current = "bhayanakshare.2026-09-03.log";
        prune(tmp.path(), u64::MAX, current).unwrap();
        // The two oldest that are not the current one go.
        assert_eq!(names(tmp.path()).len(), KEEP_FILES);
        assert!(!names(tmp.path()).contains(&"bhayanakshare.2026-09-01.log".to_owned()));
        assert!(names(tmp.path()).contains(&current.to_owned()));

        // Over the cap on its own: everything else goes, it stays.
        write_file(tmp.path(), current, 500);
        assert_eq!(prune(tmp.path(), 250, current).unwrap(), 500);
        assert_eq!(names(tmp.path()), [current]);
    }

    #[test]
    fn pruning_a_folder_that_is_not_there_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(prune(&tmp.path().join("nope"), 10, "x").unwrap(), 0);
    }

    #[test]
    fn opening_prunes_what_an_older_run_left_over_the_limits() {
        let tmp = tempfile::tempdir().unwrap();
        for n in 1..=9 {
            write_file(tmp.path(), &format!("bhayanakshare.2026-09-0{n}.log"), 100);
        }
        let logs = logs_on(tmp.path(), 450, &day("2026-09-10"));
        (&logs).write_all(b"hello\n").unwrap();

        let total: u64 = log_files(tmp.path()).unwrap().iter().map(|(_, len)| len).sum();
        assert!(total <= 450 + 6, "{total} bytes in {:?}", names(tmp.path()));
        assert_eq!(names(tmp.path()).last().unwrap(), "bhayanakshare.2026-09-10.log");
        assert!(names(tmp.path()).len() <= KEEP_FILES);
    }

    #[test]
    fn writing_past_the_cap_deletes_older_files_then_stops_writing() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "bhayanakshare.2000-01-01.log", 1000);
        let logs = logs_on(tmp.path(), 4 * MIB as u64, &day("2026-10-07"));
        let chunk = vec![b'y'; MIB];
        for _ in 0..10 {
            (&logs).write_all(&chunk).unwrap();
        }

        let files = log_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 1, "the old file is gone: {files:?}");
        // Stopped at the first look after the cap was passed, which is one megabyte on.
        assert_eq!(files[0].1, 5 * MIB as u64, "writing stopped just past the cap");
    }

    #[test]
    fn a_full_log_starts_a_new_file_when_the_day_changes_and_goes_on() {
        let tmp = tempfile::tempdir().unwrap();
        let today = day("2026-10-01");
        let logs = logs_on(tmp.path(), 2 * MIB as u64, &today);
        for _ in 0..3 {
            (&logs).write_all(&vec![b'y'; MIB]).unwrap();
        }
        (&logs).write_all(b"dropped, the log is full\n").unwrap();
        assert_eq!(names(tmp.path()), ["bhayanakshare.2026-10-01.log"]);

        *today.lock().unwrap() = "2026-10-02".to_owned();
        (&logs).write_all(b"first line of the next day\n").unwrap();

        assert_eq!(names(tmp.path()), ["bhayanakshare.2026-10-02.log"], "the full file went, to make room");
        let written = fs::read_to_string(tmp.path().join("bhayanakshare.2026-10-02.log")).unwrap();
        assert_eq!(written, "first line of the next day\n");
        // And it is not full any more.
        (&logs).write_all(b"second line\n").unwrap();
        let written = fs::read_to_string(tmp.path().join("bhayanakshare.2026-10-02.log")).unwrap();
        assert_eq!(written, "first line of the next day\nsecond line\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_pruned_does_not_make_a_write_fail_or_stop_the_log() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let logs = logs_on(tmp.path(), 2 * MIB as u64, &day("2026-10-07"));
        (&logs).write_all(b"first\n").unwrap();
        write_file(tmp.path(), "bhayanakshare.2000-01-01.log", 3 * MIB);
        // Nothing in it can be deleted now (the open file can still be written).
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o500)).unwrap();
        let cannot_delete = fs::remove_file(tmp.path().join("bhayanakshare.2000-01-01.log")).is_err();

        let mut results = Vec::new();
        for _ in 0..2 {
            results.push((&logs).write_all(&vec![b'y'; MIB]));
        }
        results.push((&logs).write_all(b"last\n"));
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();

        if cannot_delete {
            assert!(results.iter().all(Result::is_ok), "{results:?}");
            let written = fs::read(tmp.path().join("bhayanakshare.2026-10-07.log")).unwrap();
            assert!(written.ends_with(b"last\n"), "the log went on");
        }
    }

    #[test]
    fn a_line_logged_from_inside_a_write_is_dropped_and_does_not_wait_for_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let this: Arc<OnceLock<Arc<LogFiles>>> = Arc::default();
        let inside = this.clone();
        // The day is asked for while the lock is held: where a panic in the write would be.
        let logs = Arc::new(
            LogFiles::open_with(
                tmp.path(),
                MAX_LOG_BYTES,
                Box::new(move || {
                    if let Some(logs) = inside.get() {
                        (&**logs).write_all(b"logged from inside\n").unwrap();
                    }
                    "2026-10-07".to_owned()
                }),
            )
            .unwrap(),
        );
        this.set(logs.clone()).ok();

        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            (&*logs).write_all(b"outer\n").unwrap();
            done.send(()).unwrap();
        });
        finished.recv_timeout(Duration::from_secs(10)).expect("the write waits for itself");

        let written = fs::read_to_string(tmp.path().join("bhayanakshare.2026-10-07.log")).unwrap();
        assert_eq!(written, "outer\n");
    }

    #[test]
    fn the_log_files_are_for_their_owner_only() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().unwrap();
            let _logs = LogFiles::open(tmp.path()).unwrap();
            let (path, _) = log_files(tmp.path()).unwrap().remove(0);
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
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

        write_diagnostics_zip(Some(&logs), &dest, "App version: 1.2.3\n").unwrap();

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

    #[test]
    fn an_export_without_a_logs_folder_is_the_about_file() {
        let tmp = tempfile::tempdir().unwrap();
        for logs in [None, Some(tmp.path().join("not-made"))] {
            let dest = tmp.path().join("out.zip");
            write_diagnostics_zip(logs.as_deref(), &dest, "about").unwrap();
            let zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
            assert_eq!(zip.file_names().collect::<Vec<_>>(), ["about.txt"]);
        }
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
            assert!(write_diagnostics_zip(Some(&logs), &dest, "about").is_err());
            assert!(!dest.exists(), "a partly written zip is left behind");
        }
        // A destination that cannot be made is an error too.
        let missing = tmp.path().join("no-such-folder").join("out.zip");
        assert!(write_diagnostics_zip(Some(&logs), &missing, "about").is_err());
    }

    #[test]
    fn the_os_description_names_the_system_and_architecture() {
        let text = os_description();
        assert!(text.starts_with(std::env::consts::OS), "{text}");
        assert!(text.ends_with(std::env::consts::ARCH), "{text}");
    }
}
