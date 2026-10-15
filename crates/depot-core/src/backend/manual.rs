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

/// The app's own folder: `<root>/<first component>` for a program living under one of the roots.
pub fn appdir(program: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let real = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    roots.iter().find_map(|root| {
        let rest = real.strip_prefix(root).ok()?;
        let first = rest.components().next()?;
        let dir = root.join(first);
        (dir != *root && dir.is_dir()).then_some(dir)
    })
}

/// True if `text` mentions `dir` as a whole path (so `/opt/foo` doesn't match `/opt/foobar`).
fn mentions(text: &str, dir: &str) -> bool {
    text.match_indices(dir).any(|(i, _)| {
        text[i + dir.len()..].chars().next().is_none_or(|c| matches!(c, '/' | '"' | '\'' | ' ' | '\n' | '\t' | ';'))
    })
}

/// Work out exactly which files belong to a manual install.
/// `icon_users` counts how many launchers use each icon name, so shared icons are never removed.
pub fn plan(
    entry: &DesktopEntry,
    program: Option<&Path>,
    home: &Path,
    icon_users: &HashMap<String, usize>,
) -> ManualPlan {
    let roots = app_roots(home);
    let appdir = program.and_then(|p| appdir(p, &roots));
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
        if base.len() >= 3 {
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
