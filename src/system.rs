//! Running package-manager commands, and small helpers for showing paths.

use std::path::{Path, PathBuf};
use std::process::Command;

/// How a command ended.
pub struct Outcome {
    pub code: i32,
    pub output: String,
}

impl Outcome {
    pub fn ok() -> Self {
        Outcome { code: 0, output: String::new() }
    }

    pub fn failed(output: impl Into<String>) -> Self {
        Outcome { code: 1, output: output.into() }
    }

    /// pkexec exits with 126 when the password prompt was dismissed and 127 when not authorised.
    pub fn cancelled(&self) -> bool {
        matches!(self.code, 126 | 127)
    }

    /// The last `n` lines of output, which is where package managers put the reason they failed.
    pub fn tail(&self, n: usize) -> String {
        let lines: Vec<&str> = self.output.trim().lines().collect();
        lines[lines.len().saturating_sub(n)..].join("\n")
    }
}

/// Run a command, as root through pkexec when `root` is set. Blocks until it finishes.
pub fn run(cmd: &[String], root: bool) -> Outcome {
    let Some((prog, args)) = cmd.split_first() else { return Outcome::failed("Nothing to run.") };
    let mut c = if root { Command::new("pkexec") } else { Command::new(prog) };
    if root {
        c.args(["env", "DEBIAN_FRONTEND=noninteractive", "LC_ALL=C.UTF-8"]).arg(prog);
    }
    c.args(args);
    match c.output() {
        Ok(o) => Outcome {
            code: o.status.code().unwrap_or(1),
            output: format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
        },
        Err(e) => Outcome::failed(e.to_string()),
    }
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
