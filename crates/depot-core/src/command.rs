//! Running the package managers' own commands to read what they know.

use std::ffi::OsStr;
use std::process::Command;

/// A command that couldn't be run, or ran and failed.
#[derive(Debug)]
pub struct Failed {
    /// One line saying what went wrong, with the command's own message.
    pub message: String,
    /// What it printed on stderr; `None` when it didn't start at all.
    pub stderr: Option<String>,
    /// What it printed on stdout before failing. Some commands report findings with a non-zero
    /// exit code (`npm outdated`).
    pub stdout: String,
}

impl From<Failed> for String {
    fn from(f: Failed) -> String {
        f.message
    }
}

/// Run `program` with English messages and return what it printed. `Ok(None)`: the program
/// isn't installed, so there's nothing of its kind to read.
pub fn output<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Result<Option<String>, Failed> {
    let out = match Command::new(program).args(args).env("LC_ALL", "C.UTF-8").env("LANGUAGE", "C").output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Failed {
                message: format!("Couldn't run {program}: {e}"),
                stderr: None,
                stdout: String::new(),
            });
        }
    };
    if out.status.success() {
        return Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()));
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let what = args.first().map(|a| format!("{program} {}", a.as_ref().to_string_lossy())).unwrap_or(program.into());
    let message =
        if stderr.is_empty() { format!("{what} failed ({})", out.status) } else { format!("{what} failed: {stderr}") };
    Err(Failed { message, stderr: Some(stderr), stdout: String::from_utf8_lossy(&out.stdout).into_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_programs_mean_nothing_to_read() {
        assert!(matches!(output("depot-no-such-program", &["x"]), Ok(None)));
    }

    #[test]
    fn output_and_failures() {
        assert_eq!(output("sh", &["-c", "echo hi"]).unwrap().as_deref(), Some("hi\n"));
        let f = output("sh", &["-c", "echo found; echo oops >&2; exit 3"]).unwrap_err();
        assert_eq!(f.stdout, "found\n");
        assert_eq!(f.message, "sh -c failed: oops");
        assert_eq!(f.stderr.as_deref(), Some("oops"));
        let quiet = output("sh", &["-c", "exit 3"]).unwrap_err();
        assert!(quiet.message.starts_with("sh -c failed (exit status: 3"), "{}", quiet.message);
    }
}
