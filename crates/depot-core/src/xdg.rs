//! Launcher discovery following the XDG precedence rules.

use crate::desktop::DesktopEntry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The winning launcher for one desktop file ID, plus lower-precedence copies it hides.
#[derive(Clone, Debug)]
pub struct Launcher {
    pub entry: DesktopEntry,
    pub shadowed: Vec<PathBuf>,
}

/// `applications` folders in precedence order (user first), including the
/// Snap and Flatpak export folders even when `XDG_DATA_DIRS` leaves them out.
pub fn app_dirs(home: &Path) -> Vec<PathBuf> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let data_home = var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let mut roots = vec![data_home];
    let dirs = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    roots.extend(dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    roots.extend([
        home.join(".local/share/flatpak/exports/share"),
        PathBuf::from("/var/lib/flatpak/exports/share"),
        PathBuf::from("/var/lib/snapd/desktop"),
        // Nix adds these to XDG_DATA_DIRS in its own shell setup, which the desktop may not run
        home.join(".nix-profile/share"),
        PathBuf::from("/nix/var/nix/profiles/default/share"),
    ]);
    let mut out: Vec<PathBuf> = Vec::new();
    for r in roots {
        let d = r.components().collect::<PathBuf>().join("applications");
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Read every launcher. The first file with a given ID wins, as in the app menu,
/// so a `Hidden=true` copy in `~/.local` hides the system one.
pub fn collect(dirs: &[PathBuf]) -> Vec<Launcher> {
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Launcher> = Vec::new();
    for d in dirs {
        let mut files = Vec::new();
        walk(d, d, 0, &mut files);
        files.sort();
        for (id, path) in files {
            if let Some(&i) = by_id.get(&id) {
                out[i].shadowed.push(path);
                continue;
            }
            let Some(entry) = DesktopEntry::load(&path, id.clone()) else { continue };
            by_id.insert(id, out.len());
            out.push(Launcher { entry, shadowed: Vec::new() });
        }
    }
    out
}

fn walk(root: &Path, dir: &Path, depth: u8, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for ent in rd.flatten() {
        let path = ent.path();
        let is_dir = ent.file_type().map(|t| t.is_dir() || (t.is_symlink() && path.is_dir())).unwrap_or(false);
        if is_dir {
            if depth < 3 {
                walk(root, &path, depth + 1, out);
            }
            continue;
        }
        if path.extension().is_some_and(|e| e == "desktop")
            && let Ok(rel) = path.strip_prefix(root)
        {
            let id = rel.to_string_lossy().replace('/', "-");
            out.push((id, path));
        }
    }
}
