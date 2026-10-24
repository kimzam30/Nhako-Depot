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

// PackageKit error codes with a plain explanation (PkErrorEnum)
const PK_NO_NETWORK: u32 = 2;
const PK_DEP_RESOLUTION_FAILED: u32 = 13;
const PK_CANNOT_REMOVE_SYSTEM_PACKAGE: u32 = 20;
const PK_CANNOT_GET_LOCK: u32 = 26;
const PK_NO_SPACE_ON_DEVICE: u32 = 46;
const PK_CANNOT_FETCH_SOURCES: u32 = 64;
const PK_LOCK_REQUIRED: u32 = 67;

/// One sentence a newcomer can act on, from a package manager's error output (and PackageKit's
/// error code, if there is one).
pub fn plain(details: &str, pk_code: Option<u32>) -> String {
    let d = details.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| d.contains(w));
    match pk_code {
        Some(PK_CANNOT_GET_LOCK | PK_LOCK_REQUIRED) => return BUSY.into(),
        Some(PK_NO_NETWORK | PK_CANNOT_FETCH_SOURCES) => return OFFLINE.into(),
        Some(PK_NO_SPACE_ON_DEVICE) => return NO_SPACE.into(),
        Some(PK_CANNOT_REMOVE_SYSTEM_PACKAGE) => return depot_core::remove::SYSTEM_NEEDS_IT.into(),
        Some(PK_DEP_RESOLUTION_FAILED) => {
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

/// What a running command reports while it runs.
pub enum Step<'a> {
    /// The password was accepted (only reported when the command runs as root).
    Authorized,
    Line(&'a str),
}

/// Printed by the root shell before the real command starts: seeing it means the password worked.
const AUTH_MARKER: &str = "@@depot-authorized@@";

/// Run a command, as root through pkexec when `root` is set, reporting progress to `on`.
/// `cancel` stops it while it can still be stopped: always for user commands, and for root ones
/// only before the password is entered (after that the command belongs to root).
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
    let flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE;
    let proc = match gio::Subprocess::newv(&os, flags) {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("Couldn't start {}: {}", cmd[0], e.message())),
    };
    let stop = {
        let proc = proc.clone();
        cancel.connect_cancelled_local(move |_| proc.force_exit())
    };
    let mut output = String::new();
    let mut authorized = !root;
    if let Some(out) = proc.stdout_pipe() {
        let lines = gio::DataInputStream::new(&out);
        while let Ok(Some(line)) = lines.read_line_utf8_future(glib::Priority::DEFAULT).await {
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
        // printed they come from the command itself (127 = program not found).
        Some(126 | 127) if !authorized => Outcome::Cancelled,
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
        assert_eq!(plain("whatever", Some(PK_NO_NETWORK)), OFFLINE);
        assert!(plain("error: snap \"x\" has running apps (x)", None).contains("Close it"));
        assert!(plain("weird", None).starts_with("Something went wrong"));
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail("", 2), "");
    }
}
