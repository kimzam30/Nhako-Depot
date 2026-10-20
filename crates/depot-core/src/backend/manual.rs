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

/// Sub-folders that are part of one app's layout, never an app of their own.
const LAYOUT_DIRS: &[&str] = &["bin", "lib", "lib64", "libexec", "share", "resources", "usr", "opt"];

/// Whether `dir` only groups several apps (e.g. `~/Applications/tools/{a,b}`) rather than being
/// one app's folder: it holds no files of its own, and another launcher's program lives in a
/// different sub-folder than this program's.
fn groups_apps(dir: &Path, program: &Path, others: &[PathBuf]) -> bool {
    let Some(child) = program.strip_prefix(dir).ok().and_then(|r| r.components().next()) else { return false };
    let child = dir.join(child);
    let layout = child.file_name().and_then(|n| n.to_str()).is_some_and(|n| LAYOUT_DIRS.contains(&n));
    if layout || !child.is_dir() {
        return false;
    }
    let other_app = others.iter().any(|o| o.starts_with(dir) && !o.starts_with(&child));
    let has_files =
        std::fs::read_dir(dir).map(|rd| rd.flatten().any(|e| e.file_type().is_ok_and(|t| !t.is_dir()))).unwrap_or(true);
    other_app && !has_files
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
        let base = entry.id.trim_end_matches(".desktop");
        let autostart = home.join(".config/autostart").join(&entry.id);
        if let Some(a) = DesktopEntry::load(&autostart, entry.id.clone())
            && let Some(exec::Target::Program { path: Some(p), .. }) = exec::target(&a.argv(), &[])
            && p.starts_with(dir)
        {
            files.push(autostart);
        }
        for name in [format!("{base}.metainfo.xml"), format!("{base}.appdata.xml")] {
            let m = home.join(".local/share/metainfo").join(name);
            if m.exists() {
                files.push(m);
            }
        }
        let dir_s = dir.to_string_lossy();
        if let Ok(rd) = std::fs::read_dir(home.join(".local/bin")) {
            for b in rd.flatten().map(|e| e.path()) {
                let hit = std::fs::canonicalize(&b).is_ok_and(|r| r.starts_with(dir))
                    || (std::fs::symlink_metadata(&b).is_ok_and(|m| m.is_file() && m.len() < 8192)
                        && std::fs::read(&b).is_ok_and(|t| mentions(&String::from_utf8_lossy(&t), &dir_s)));
                if hit {
                    files.push(b);
                }
            }
        }
        let icon = entry.icon.as_str();
        if !icon.is_empty() && !icon.contains('/') && icon_users.get(icon).copied().unwrap_or(0) <= 1 {
            find_icons(&home.join(".local/share/icons"), icon, 0, &mut files);
        }
        // settings folders are only guessed when the launcher's name is the app's own name
        let lower = base.to_lowercase();
        let named = |p: Option<&std::ffi::OsStr>| p.and_then(|n| n.to_str()).is_some_and(|n| n.to_lowercase() == lower);
        let own_name = named(dir.file_name()) || named(program.and_then(Path::file_stem));
        if base.len() >= 3 && own_name && !SHARED_DATA.iter().any(|s| s.eq_ignore_ascii_case(base)) {
            for d in [".local/share", ".config", ".cache"] {
                let p = home.join(d).join(base);
                if p.exists() {
                    data.push(p);
                }
            }
        }
    }
    ManualPlan { appdir, files, data }
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

/// Version written by tarball installers that leave a marker file in the app folder.
pub fn version(appdir: &Path) -> String {
    std::fs::read_to_string(appdir.join(".blip-tarball-install")).map(|s| s.trim().to_string()).unwrap_or_default()
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
