//! Running package-manager commands, how operations end, and small helpers for showing paths.

use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::{Path, PathBuf};

/// How an operation ended.
#[derive(Clone, Debug)]
pub enum Outcome {
    Done,
    /// The password prompt was dismissed or refused, or the user pressed Cancel.
    Cancelled,
    /// `summary` is one plain sentence; `details` is what the package manager said.
    Failed {
        summary: String,
        details: String,
    },
}

impl Outcome {
    pub fn failed(details: impl Into<String>) -> Self {
        let details = details.into();
        Outcome::Failed { summary: plain(&details, None), details }
    }
}

/// One sentence a newcomer can act on, from a package manager's error output (and PackageKit's
/// error code, if there is one).
pub fn plain(details: &str, pk_code: Option<u32>) -> String {
    let d = details.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| d.contains(w));
    use crate::packagekit::{
        ERROR_CANNOT_FETCH_SOURCES, ERROR_CANNOT_GET_LOCK, ERROR_CANNOT_REMOVE_SYSTEM_PACKAGE,
        ERROR_DEP_RESOLUTION_FAILED, ERROR_LOCK_REQUIRED, ERROR_NO_NETWORK, ERROR_NO_SPACE_ON_DEVICE,
    };
    match pk_code {
        Some(ERROR_CANNOT_GET_LOCK | ERROR_LOCK_REQUIRED) => return BUSY.into(),
        Some(ERROR_NO_NETWORK | ERROR_CANNOT_FETCH_SOURCES) => return OFFLINE.into(),
        Some(ERROR_NO_SPACE_ON_DEVICE) => return NO_SPACE.into(),
        Some(ERROR_CANNOT_REMOVE_SYSTEM_PACKAGE) => return depot_core::remove::SYSTEM_NEEDS_IT.into(),
        Some(ERROR_DEP_RESOLUTION_FAILED) => {
            return "The package manager couldn't find a safe way to do this without breaking other software.".into();
        }
        _ => {}
    }
    if has(&[
        "could not get lock",
        "unable to lock",
        "unable to acquire",
        "database is locked",
        "is locked",
        "another process",
    ]) {
        BUSY.into()
    } else if has(&[
        "temporary failure resolving",
        "could not resolve",
        "network is unreachable",
        "no network",
        "unable to connect",
    ]) {
        OFFLINE.into()
    } else if has(&["no space left", "not enough space", "insufficient disk space"]) {
        NO_SPACE.into()
    } else if has(&["no authentication agent", "no polkit agent"]) {
        NO_AGENT.into()
    } else if has(&["is required by", "because it is required", "needed by"]) {
        "Other software still needs it, so it was left installed.".into()
    } else if has(&["eacces", "permission denied", "operation not permitted"]) {
        "Depot isn't allowed to change these files. They may have been installed for everyone, which needs an administrator.".into()
    } else if has(&["has running apps", "is running", "close the app"]) {
        "The app is still open. Close it, then try again.".into()
    } else if has(&["not found", "no such file", "unable to locate"]) {
        "Depot couldn't find what it needed on your computer. It may have been removed already.".into()
    } else {
        "Something went wrong. The details below say what the package manager reported.".into()
    }
}

const BUSY: &str =
    "Another app is installing or updating software right now. Wait until it's finished, then try again.";
const OFFLINE: &str = "Depot couldn't reach the internet. Check your connection, then try again.";
const NO_SPACE: &str = "There isn't enough free disk space. Free up some space, then try again.";
const NO_AGENT: &str = "Depot couldn't ask for your password because your desktop's password helper isn't running. Log out and back in, then try again.";

/// What a running command reports while it runs.
pub enum Step<'a> {
    /// The password was accepted (only reported when the command runs as root).
    Authorized,
    Line(&'a str),
}

/// Printed by the root shell before the real command starts: seeing it means the password worked.
const AUTH_MARKER: &str = "@@depot-authorized@@";

/// Run a command, as root through pkexec when `root` is set, reporting progress to `on`.
/// `cancel` stops it while it can still be stopped: always for user commands (with Ctrl+C, so
/// Flatpak and Snap can stop cleanly), and for root ones only before the password is entered
/// (after that the command belongs to root).
pub async fn run(cmd: &[String], root: bool, cancel: &gio::Cancellable, on: impl Fn(Step<'_>)) -> Outcome {
    if cmd.is_empty() {
        return Outcome::failed("Nothing to run.");
    }
    let mut argv: Vec<String> = Vec::new();
    if root {
        // the marker line tells the password prompt apart from the work, which pkexec alone can't
        argv.extend(
            [
                "pkexec",
                "/bin/sh",
                "-c",
                &format!("echo {AUTH_MARKER}; exec env DEBIAN_FRONTEND=noninteractive LC_ALL=C.UTF-8 \"$@\""),
                "depot",
            ]
            .map(String::from),
        );
    }
    argv.extend(cmd.iter().cloned());
    let os: Vec<&std::ffi::OsStr> = argv.iter().map(|a| a.as_ref()).collect();
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE);
    // English output, so `plain()` can recognise the errors (the root shell sets this itself)
    launcher.setenv("LC_ALL", "C.UTF-8", true);
    let proc = match launcher.spawn(&os) {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("Couldn't start {}: {}", cmd[0], e.message())),
    };
    let stop = {
        let proc = proc.clone();
        // a user command gets Ctrl+C, which Flatpak and Snap stop cleanly on; pkexec still waiting
        // for the password has changed nothing yet, so it's simply ended
        cancel.connect_cancelled_local(move |_| if root { proc.force_exit() } else { proc.send_signal(2) })
    };
    let mut output = String::new();
    let mut authorized = !root;
    if let Some(out) = proc.stdout_pipe() {
        let lines = gio::DataInputStream::new(&out);
        // raw bytes: one line that isn't UTF-8 must not stop the reading, or the command would
        // block on a full pipe and never finish
        while let Ok(Some(bytes)) = lines.read_line_future(glib::Priority::DEFAULT).await {
            let line = String::from_utf8_lossy(&bytes);
            if !authorized && line.trim() == AUTH_MARKER {
                authorized = true;
                on(Step::Authorized);
                continue;
            }
            on(Step::Line(&line));
            output.push_str(&line);
            output.push('\n');
        }
    }
    let _ = proc.wait_future().await;
    if let Some(id) = stop {
        cancel.disconnect_cancelled(id);
    }
    if cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    match proc.has_exited().then(|| proc.exit_status()) {
        Some(0) => Outcome::Done,
        // pkexec: 126 = the prompt was dismissed, 127 = not authorised. Once the marker was
        // printed they come from the command itself (127 = program not found). With no password
        // helper running nobody was asked at all, which isn't the user's choice.
        Some(126 | 127) if !authorized && !output.to_lowercase().contains("authentication agent") => Outcome::Cancelled,
        Some(code) => {
            let details = tail(&output, 8);
            let details =
                if details.is_empty() { format!("{} stopped with exit code {code}.", cmd[0]) } else { details };
            Outcome::failed(details)
        }
        None => Outcome::failed(format!("{} was stopped.", cmd[0])),
    }
}

/// The last `n` lines of output, which is where package managers put the reason they failed.
fn tail(output: &str, n: usize) -> String {
    let lines: Vec<&str> = output.trim().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// "1 package", "3 packages".
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// PackageKit's percentage as a fraction, `None` while it doesn't know yet.
pub fn fraction(pct: u32) -> Option<f64> {
    (pct <= 100).then(|| f64::from(pct) / 100.0)
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// A path with the home folder written as `~`.
pub fn short(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(r) if !h.as_os_str().is_empty() => format!("~/{}", r.display()),
        _ => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_language_failures() {
        assert_eq!(plain("E: Could not get lock /var/lib/dpkg/lock-frontend", None), BUSY);
        assert_eq!(plain("whatever", Some(crate::packagekit::ERROR_NO_NETWORK)), OFFLINE);
        assert!(plain("error: snap \"x\" has running apps (x)", None).contains("Close it"));
        assert!(plain("weird", None).starts_with("Something went wrong"));
        assert!(plain("npm error code EACCES", None).contains("administrator"));
        assert!(
            plain("Error: Refusing to uninstall pcre2 because it is required by git", None).contains("still needs")
        );
        assert_eq!(plain("Error executing command as another user: No authentication agent found.", None), NO_AGENT);
    }

    fn sh(script: &str) -> Vec<String> {
        ["sh", "-c", script].map(String::from).to_vec()
    }

    /// A line that isn't UTF-8 followed by more output than the pipe holds used to stop the
    /// reading, so the command blocked forever and the sheet never finished.
    #[test]
    fn odd_output_never_stalls_a_command() {
        let cancel = gio::Cancellable::new();
        let lines = std::cell::Cell::new(0);
        let o =
            glib::MainContext::new().block_on(run(&sh("printf 'caf\\351\\n'; seq 1 30000"), false, &cancel, |_| {
                lines.set(lines.get() + 1)
            }));
        assert!(matches!(o, Outcome::Done), "{o:?}");
        assert_eq!(lines.get(), 30001);
    }

    #[test]
    fn cancel_stops_a_user_command() {
        let cancel = gio::Cancellable::new();
        let c = cancel.clone();
        let started = std::time::Instant::now();
        let o = glib::MainContext::new().block_on(async move {
            let press = async move {
                glib::timeout_future(std::time::Duration::from_millis(200)).await;
                c.cancel();
                std::future::pending::<Outcome>().await
            };
            let cmd = ["sleep".to_string(), "30".into()];
            let work = run(&cmd, false, &cancel, |_| {});
            futures_util::future::select(Box::pin(work), Box::pin(press)).await.factor_first().0
        });
        assert!(started.elapsed().as_secs() < 5, "Cancel must stop it, not wait it out");
        assert!(matches!(o, Outcome::Cancelled), "{o:?}");
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail("", 2), "");
    }
}
