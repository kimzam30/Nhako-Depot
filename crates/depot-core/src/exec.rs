//! Work out what program a launcher starts.

use std::path::{Path, PathBuf};

/// Commands that run another command; the real program follows them.
const WRAPPERS: &[&str] = &[
    "env",
    "pkexec",
    "sudo",
    "nice",
    "ionice",
    "nohup",
    "setsid",
    "gksu",
    "gksudo",
    "kdesu",
    "torsocks",
    "prime-run",
    "gamemoderun",
    "mangohud",
    "optirun",
    "primusrun",
];

/// Wrapper options whose value is the next argument (`env -u VAR`, `nice -n 10`, `ionice -c 3`).
fn takes_value(wrapper: &str, opt: &str) -> bool {
    let opts: &[&str] = match wrapper {
        "env" => &["-u", "--unset", "-C", "--chdir"],
        "nice" => &["-n", "--adjustment"],
        "ionice" => &["-c", "--class", "-n", "--classdata", "-p", "--pid", "-P", "--pgid", "-u", "--uid"],
        "sudo" => &["-u", "--user", "-g", "--group", "-D", "--chdir", "-p", "--prompt"],
        "pkexec" => &["--user"],
        _ => &[],
    };
    opts.contains(&opt)
}

/// Programs that run something else (interpreters, openers, containers). Owning one of
/// these says nothing about who installed the launcher, so they never decide ownership.
/// Anything starting with `python` or `java` counts too, see [`is_generic`].
const GENERIC: &[&str] = &[
    "sh",
    "bash",
    "dash",
    "zsh",
    "fish",
    "perl",
    "ruby",
    "node",
    "nodejs",
    "wine",
    "wine64",
    "mono",
    "dotnet",
    "xdg-open",
    "gio",
    "gtk-launch",
    "gapplication",
    "electron",
    "busctl",
    "dbus-send",
    "appimage-run",
    "steam-run",
    "xterm",
    "gnome-terminal",
    "x-terminal-emulator",
    "konsole",
    "distrobox-enter",
    "distrobox",
    "toolbox",
    "podman",
    "docker",
    "firejail",
    "bwrap",
    "flatpak-spawn",
    "x-www-browser",
    "sensible-browser",
];

/// Options of `flatpak run` and `snap run` whose value can follow as its own argument.
const RUN_VALUE_OPTS: &[&str] = &["--command", "--branch", "--arch", "--runtime", "--runtime-version", "--cwd"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Flatpak(String),
    Snap(String),
    Program {
        /// The program as found on disk, if it exists.
        path: Option<PathBuf>,
        /// An interpreter or opener rather than the app itself.
        generic: bool,
        /// First absolute file argument (e.g. the script an interpreter runs).
        script: Option<PathBuf>,
    },
}

fn base(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn is_generic(b: &str) -> bool {
    GENERIC.contains(&b) || b.starts_with("python") || b.starts_with("java")
}

/// Find the program behind a launcher's arguments.
pub fn target(argv: &[String], search: &[PathBuf]) -> Option<Target> {
    let mut i = 0;
    while i < argv.len() && WRAPPERS.contains(&base(&argv[i])) {
        let wrapper = base(&argv[i]);
        i += 1;
        while i < argv.len() && (argv[i].starts_with('-') || argv[i].contains('=')) {
            i += if takes_value(wrapper, &argv[i]) { 2 } else { 1 };
        }
    }
    let prog = argv.get(i)?;
    let rest = &argv[i + 1..];
    let b = base(prog);
    // the first plain argument after `run`, skipping options and the values of options that
    // take one as a separate argument (`flatpak run --command foo org.x.Y`)
    let after_run = || {
        let r = rest.iter().position(|a| a == "run")?;
        let mut args = rest[r + 1..].iter();
        while let Some(a) = args.next() {
            if !a.starts_with('-') {
                return Some(a.clone());
            }
            if RUN_VALUE_OPTS.contains(&a.as_str()) {
                args.next();
            }
        }
        None
    };
    if b == "flatpak"
        && let Some(app) = after_run()
    {
        return Some(Target::Flatpak(app.split("//").next().unwrap_or(&app).to_string()));
    }
    if b == "snap"
        && let Some(app) = after_run()
    {
        return Some(Target::Snap(app.split('.').next().unwrap_or(&app).to_string()));
    }
    if let Some(name) = prog.strip_prefix("/snap/bin/") {
        return Some(Target::Snap(name.split('.').next().unwrap_or(name).to_string()));
    }
    let script = rest.iter().map(Path::new).find(|p| p.is_absolute() && p.is_file()).map(Path::to_path_buf);
    Some(Target::Program { path: resolve(prog, search), generic: is_generic(b), script })
}

/// Look a program up the way a shell would.
pub fn resolve(prog: &str, search: &[PathBuf]) -> Option<PathBuf> {
    if prog.contains('/') {
        let p = Path::new(prog);
        return (p.is_absolute() && p.is_file()).then(|| p.to_path_buf());
    }
    search.iter().map(|d| d.join(prog)).find(|p| p.is_file())
}

/// Spellings a package database might use for this file: as found, fully resolved,
/// and with or without the `/usr` prefix on merged-/usr systems.
pub fn path_variants(p: &Path) -> Vec<PathBuf> {
    let mut v = vec![p.to_path_buf()];
    if let Ok(c) = std::fs::canonicalize(p)
        && c != p
    {
        v.push(c);
    }
    let mut extra = Vec::new();
    for q in &v {
        let s = q.to_string_lossy();
        for (a, b) in
            [("/usr/bin/", "/bin/"), ("/usr/sbin/", "/sbin/"), ("/usr/lib/", "/lib/"), ("/usr/lib64/", "/lib64/")]
        {
            if let Some(r) = s.strip_prefix(a) {
                extra.push(PathBuf::from(format!("{b}{r}")));
            } else if let Some(r) = s.strip_prefix(b) {
                extra.push(PathBuf::from(format!("{a}{r}")));
            }
        }
    }
    for e in extra {
        if !v.contains(&e) {
            v.push(e);
        }
    }
    v
}

/// The PATH used to resolve launcher commands.
pub fn search_path(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        std::env::var("PATH").unwrap_or_default().split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    for d in ["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin", "/usr/games", "/snap/bin"]
    {
        let d = PathBuf::from(d);
        if !v.contains(&d) {
            v.push(d);
        }
    }
    // Depot started from the app menu may have a shorter PATH than the user's terminal, so add the
    // usual personal folders too; otherwise their launchers would look broken
    for d in [".local/bin", "bin", ".cargo/bin", "go/bin", ".nix-profile/bin", ".local/share/flatpak/exports/bin"] {
        let d = home.join(d);
        if !v.contains(&d) {
            v.push(d);
        }
    }
    for d in ["/var/lib/flatpak/exports/bin", "/nix/var/nix/profiles/default/bin", "/usr/local/games"] {
        let d = PathBuf::from(d);
        if !v.contains(&d) {
            v.push(d);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn wrappers_and_sandboxes() {
        assert_eq!(
            target(&a(&["flatpak", "run", "--branch=stable", "org.x.Y//stable"]), &[]),
            Some(Target::Flatpak("org.x.Y".into()))
        );
        assert_eq!(
            target(&a(&["env", "BAMF=1", "/snap/bin/code.url-handler"]), &[]),
            Some(Target::Snap("code".into()))
        );
        assert_eq!(target(&a(&["snap", "run", "spotify"]), &[]), Some(Target::Snap("spotify".into())));
        assert_eq!(
            target(&a(&["flatpak", "run", "--command", "tool", "org.x.Y"]), &[]),
            Some(Target::Flatpak("org.x.Y".into()))
        );
        assert_eq!(
            target(&a(&["env", "-u", "GTK_THEME", "nice", "-n", "5", "snap", "run", "x"]), &[]),
            Some(Target::Snap("x".into()))
        );
        match target(&a(&["pkexec", "python3", "/nonexistent/x.py"]), &[]) {
            Some(Target::Program { generic, .. }) => assert!(generic),
            t => panic!("{t:?}"),
        }
    }

    #[test]
    fn usrmerge_variants() {
        let v = path_variants(Path::new("/nonexistent-usr/usr/bin/x"));
        assert_eq!(v.len(), 1);
        let v = path_variants(Path::new("/usr/bin/depot-test-nonexistent"));
        assert!(v.contains(&PathBuf::from("/bin/depot-test-nonexistent")));
    }
}
