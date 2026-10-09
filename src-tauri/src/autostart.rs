//! Start at login.
//!
//! This is the `auto-launch` crate the Tauri autostart plugin wraps, used directly because the
//! plugin hands the crate the program path as it is, and on Windows the crate writes it unquoted
//! into the Run key (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`): a path with a space,
//! such as `C:\Users\Test User\AppData\Local\BhayanakShare\bhayanakshare.exe`, is then cut at the
//! space and nothing starts at login.

use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use tauri::{AppHandle, Runtime};

/// The login entry of this install, managed by the app.
pub struct Autostart(AutoLaunch);

impl Autostart {
    /// The entry for the running program, which starts it with `args`. Its name is the product
    /// name, which is also the name of the Run value the Windows uninstaller removes (it deletes
    /// the value named after `productName` in `tauri.conf.json`).
    pub fn new<R: Runtime>(app: &AppHandle<R>, args: &[&str]) -> Result<Self, Box<dyn std::error::Error>> {
        let exe = std::env::current_exe()?.display().to_string();
        // An AppImage is started through its own file, not the binary inside its mount.
        #[cfg(target_os = "linux")]
        let exe = tauri::Manager::env(app).appimage.and_then(|path| path.to_str().map(str::to_owned)).unwrap_or(exe);
        Ok(Self(entry(&app.package_info().name, &exe, args)?))
    }

    pub fn enable(&self) -> auto_launch::Result<()> {
        self.0.enable()
    }

    pub fn disable(&self) -> auto_launch::Result<()> {
        self.0.disable()
    }

    pub fn is_enabled(&self) -> auto_launch::Result<bool> {
        self.0.is_enabled()
    }
}

/// The entry `name` that starts the program at `exe` with `args`.
fn entry(name: &str, exe: &str, args: &[&str]) -> auto_launch::Result<AutoLaunch> {
    let mut builder = AutoLaunchBuilder::new();
    builder.set_app_name(name).set_app_path(&program(exe)).set_args(args);
    // This is a per-user install: the entry is the user's own (HKCU), as the uninstaller expects,
    // and never in the machine-wide key, which the default would use when run as administrator.
    #[cfg(windows)]
    builder.set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser);
    builder.build()
}

/// The program path as `auto-launch` is to be given it: on Windows quoted, as it goes into a
/// command line; elsewhere as it is (the crate quotes it where it needs to).
fn program(exe: &str) -> String {
    if cfg!(windows) { quoted(exe) } else { exe.to_owned() }
}

/// `path` as one word of a Windows command line. A path cannot hold a quote there, so wrapping
/// it is all it takes.
fn quoted(path: &str) -> String {
    format!("\"{path}\"")
}

/// What `auto-launch` writes into the Run key for a program at `exe` started with `args`: the
/// program, quoted, then each argument, after a space.
#[cfg(test)]
fn run_value(exe: &str, args: &[&str]) -> String {
    format!("{} {}", quoted(exe), args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::BACKGROUND_FLAG;

    const SPACED: &str = r"C:\Users\Test User\AppData\Local\BhayanakShare\bhayanakshare.exe";

    #[test]
    fn the_run_value_quotes_the_path_and_keeps_the_background_flag() {
        assert_eq!(
            run_value(SPACED, &[BACKGROUND_FLAG]),
            r#""C:\Users\Test User\AppData\Local\BhayanakShare\bhayanakshare.exe" --background"#
        );
        assert_eq!(run_value(r"C:\app.exe", &[BACKGROUND_FLAG]), r#""C:\app.exe" --background"#);
        // Nothing is quoted twice.
        assert_eq!(quoted(r"C:\a b\c.exe"), r#""C:\a b\c.exe""#);
    }

    #[test]
    fn the_entry_is_given_the_quoted_path_on_windows_and_the_plain_one_elsewhere() {
        let login = entry("BhayanakShare", SPACED, &[BACKGROUND_FLAG]).unwrap();
        let expected = if cfg!(windows) { quoted(SPACED) } else { SPACED.to_owned() };
        assert_eq!(login.get_app_path(), expected);
        assert_eq!(login.get_args(), [BACKGROUND_FLAG]);
    }

    /// Through the real crate and the real Run key: the value is quoted, so a profile folder
    /// with a space starts at login. The entry has a name of its own and is removed again, so no
    /// real entry is touched.
    #[cfg(windows)]
    #[test]
    fn the_run_key_holds_the_quoted_path_and_is_cleaned_up() {
        use windows_registry::CURRENT_USER;

        const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
        const APPROVED: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
        let name = format!("BhayanakShare-test-{}", std::process::id());
        let login = entry(&name, SPACED, &[BACKGROUND_FLAG]).unwrap();

        let result = std::panic::catch_unwind(|| {
            login.enable().unwrap();
            let value = CURRENT_USER.open(RUN).unwrap().get_string(&name).unwrap();
            println!("Run value read back: {value}");
            assert_eq!(value, run_value(SPACED, &[BACKGROUND_FLAG]));
            assert!(login.is_enabled().unwrap());
        });

        login.disable().unwrap();
        // Task Manager's mark for the entry, which `enable` writes when the key exists.
        if let Ok(key) = CURRENT_USER.options().write().open(APPROVED) {
            let _ = key.remove_value(&name);
        }
        assert!(CURRENT_USER.open(RUN).unwrap().get_string(&name).is_err(), "the Run value is left behind");
        assert!(!login.is_enabled().unwrap());
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
