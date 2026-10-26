//! Apps installed by hand: a launcher no package manager owns.

use crate::desktop::DesktopEntry;
use crate::exec;
use crate::model::ManualPlan;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Folders where hand-installed apps keep one sub-folder each.
pub fn app_roots(home: &Path) -> Vec<PathBuf> {
    ["/.local/opt", "/opt", "/Applications", "/.local/apps", "/apps"]
        .iter()
        .map(|r| home.join(r.trim_start_matches('/')))
        .collect()
}

/// Sub-folders that are part of one app's layout, never an app of their own, including the
/// runtimes apps bundle (`jre/bin/java` doesn't make a second app).
const LAYOUT_DIRS: &[&str] = &[
    "bin",
    "lib",
    "lib64",
    "libexec",
    "share",
    "resources",
    "usr",
    "opt",
    "jre",
    "jdk",
    "jbr",
    "java",
    "node",
    "nodejs",
    "python",
    "runtime",
    "plugins",
    "extensions",
    "locales",
];

/// Whether `dir` only groups several apps (e.g. `~/Applications/tools/{a,b}`) rather than being
/// one app's folder: another launcher's program lives in a different sub-folder than this
/// program's, or a sibling sub-folder holds a program of its own (an app nobody made a launcher
/// for). A folder that can't be read counts as a group, so less is removed rather than more.
fn groups_apps(dir: &Path, program: &Path, others: &[PathBuf]) -> bool {
    let Some(child) = program.strip_prefix(dir).ok().and_then(|r| r.components().next()) else { return false };
    let child = dir.join(child);
    if is_layout(&child) || !child.is_dir() {
        return false;
    }
    if others.iter().any(|o| o.starts_with(dir) && !o.starts_with(&child)) {
        return true;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return true };
    rd.flatten()
        .map(|e| e.path())
        .filter(|s| *s != child && !is_layout(s) && s.is_dir() && !s.is_symlink())
        .any(|s| has_program(&s) || has_program(&s.join("bin")))
}

fn is_layout(dir: &Path) -> bool {
    dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| LAYOUT_DIRS.contains(&n))
}

/// Whether `dir` directly holds a file anyone may run.
fn has_program(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| e.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
    })
}

/// The app's own folder under one of the roots: the first folder below the root that holds the
/// program and isn't just a group of several apps. `others` are the (resolved) programs of
/// every other launcher, so a shared folder is never taken for one app's.
pub fn appdir(program: &Path, roots: &[PathBuf], others: &[PathBuf]) -> Option<PathBuf> {
    let real = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    roots.iter().find_map(|root| {
        let rest = real.strip_prefix(root).ok()?;
        let comps: Vec<_> = rest.components().collect();
        let mut dir = root.clone();
        // the last component is the program file itself
        for c in &comps[..comps.len().saturating_sub(1)] {
            dir.push(c);
            if !groups_apps(&dir, &real, others) {
                // a source checkout with a launcher pointing into its build output is someone's
                // project, not an installed app, so it's never offered for the Trash as a whole
                if dir.join(".git").exists() {
                    return None;
                }
                return dir.is_dir().then_some(dir);
            }
        }
        None
    })
}

/// Folder names in `~/.local/share`, `~/.config` and `~/.cache` that belong to the desktop or to
/// many apps, never to one manual install.
const SHARED_DATA: &[&str] = &[
    "applications",
    "icons",
    "fonts",
    "mime",
    "themes",
    "autostart",
    "bin",
    "systemd",
    "flatpak",
    "Trash",
    "keyrings",
    "menus",
    "desktop-directories",
    "metainfo",
    "gnome-shell",
    "dconf",
    "pulse",
    "fontconfig",
    "environment.d",
    "gtk-3.0",
    "gtk-4.0",
    "nautilus",
    "user-places.xbel",
    "recently-used.xbel",
];

/// True if `text` mentions `dir` as a whole path (so `/opt/foo` doesn't match `/opt/foobar`).
fn mentions(text: &str, dir: &str) -> bool {
    text.match_indices(dir).any(|(i, _)| {
        text[i + dir.len()..].chars().next().is_none_or(|c| matches!(c, '/' | '"' | '\'' | ' ' | '\n' | '\t' | ';'))
    })
}

/// Work out exactly which files belong to a manual install.
/// `icon_users` counts how many launchers use each icon name, so shared icons are never removed.
/// `others` are the resolved programs of every other launcher (see [`appdir`]).
pub fn plan(
    entry: &DesktopEntry,
    program: Option<&Path>,
    home: &Path,
    icon_users: &HashMap<String, usize>,
    others: &[PathBuf],
) -> ManualPlan {
    let roots = app_roots(home);
    let appdir = program.and_then(|p| appdir(p, &roots, others));
    let mut files = vec![entry.path.clone()];
    let mut data = Vec::new();
    if let Some(dir) = &appdir {
        files.push(dir.clone());
        files.extend(belongings(entry, dir, home, icon_users));
        // settings folders are only guessed when the launcher's name is the app's own name
        let base = entry.base_id();
        let lower = base.to_lowercase();
        let named = |p: Option<&std::ffi::OsStr>| p.and_then(|n| n.to_str()).is_some_and(|n| n.to_lowercase() == lower);
        if named(dir.file_name()) || named(program.and_then(Path::file_stem)) {
            data = guess_data(home, base);
        }
    }
    ManualPlan { appdir, files, data }
}

/// Files outside the app's own folder or file (`target`) that came with it: an autostart entry
/// and command-line links that start it, its software-centre metadata and its icons, unless
/// another launcher uses the same icon. `entry` is the app's launcher.
pub(crate) fn belongings(
    entry: &DesktopEntry,
    target: &Path,
    home: &Path,
    icon_users: &HashMap<String, usize>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let base = entry.base_id();
    let autostart = home.join(".config/autostart").join(&entry.id);
    if let Some(a) = DesktopEntry::load(&autostart, entry.id.clone())
        && let Some(exec::Target::Program { path, script, .. }) = exec::target(&a.argv(), &[])
        && path.iter().chain(&script).any(|p| p.starts_with(target))
    {
        files.push(autostart);
    }
    for name in [format!("{base}.metainfo.xml"), format!("{base}.appdata.xml")] {
        let m = home.join(".local/share/metainfo").join(name);
        if m.exists() {
            files.push(m);
        }
    }
    files.extend(bin_links(home, target));
    let icon = entry.icon.as_str();
    if !icon.is_empty() && icon_users.get(icon).copied().unwrap_or(0) <= 1 {
        if !icon.contains('/') {
            find_icons(&home.join(".local/share/icons"), icon, 0, &mut files);
        } else if Path::new(icon).starts_with(home.join(".local/share/icons")) && Path::new(icon).is_file() {
            files.push(PathBuf::from(icon));
        }
    }
    files
}

/// Commands in `~/.local/bin` that start `target`: links into it, or small scripts naming it.
fn bin_links(home: &Path, target: &Path) -> Vec<PathBuf> {
    let target_s = target.to_string_lossy();
    let Ok(rd) = std::fs::read_dir(home.join(".local/bin")) else { return Vec::new() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|b| b != target)
        .filter(|b| {
            std::fs::canonicalize(b).is_ok_and(|r| r.starts_with(target))
                || (std::fs::symlink_metadata(b).is_ok_and(|m| m.is_file() && m.len() < 8192)
                    && std::fs::read(b).is_ok_and(|t| mentions(&String::from_utf8_lossy(&t), &target_s)))
        })
        .collect()
}

/// Settings and data folders named after the app (`base`), in `~/.local/share`, `~/.config` and
/// `~/.cache`. Names too short or shared by the desktop are never guessed.
pub(crate) fn guess_data(home: &Path, base: &str) -> Vec<PathBuf> {
    if base.len() < 3 || SHARED_DATA.iter().any(|s| s.eq_ignore_ascii_case(base)) {
        return Vec::new();
    }
    [".local/share", ".config", ".cache"].iter().map(|d| home.join(d).join(base)).filter(|p| p.exists()).collect()
}

/// Icon files named exactly `<icon>.<ext>` inside an `apps` folder.
fn find_icons(dir: &Path, icon: &str, depth: u8, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if depth < 5 {
                find_icons(&p, icon, depth + 1, out);
            }
        } else if p.file_stem().is_some_and(|s| s == icon)
            && p.extension().is_some_and(|x| matches!(x.to_str(), Some("png" | "svg" | "xpm")))
            && dir.file_name().is_some_and(|d| d == "apps")
        {
            out.push(p);
        }
    }
}

/// Total size of a folder, without following symlinks.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_respects_boundaries() {
        assert!(mentions("exec /opt/foo/bin/x \"$@\"", "/opt/foo"));
        assert!(mentions("cd '/opt/foo'", "/opt/foo"));
        assert!(!mentions("exec /opt/foobar/bin/x", "/opt/foo"));
    }
}
