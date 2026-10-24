//! AppImages: an app packed into one file that runs without being installed.

use crate::desktop::DesktopEntry;
use crate::model::{Kind, ManualPlan, Package, Role};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Whether `path` is an AppImage: an ELF program carrying the AppImage mark (`AI` and the type
/// number at byte 8), or, when the file can't be read, one named `*.AppImage`.
pub fn is_appimage(path: &Path) -> bool {
    let mut head = [0u8; 11];
    let read = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head));
    match read {
        Ok(()) => head.starts_with(b"\x7fELF") && head[8..10] == *b"AI" && matches!(head[10], 1 | 2),
        Err(_) => path.is_file() && has_extension(path),
    }
}

fn has_extension(path: &Path) -> bool {
    path.extension().is_some_and(|x| x.eq_ignore_ascii_case("appimage"))
}

/// Parts of a file name that say what computer it's built for, not what the app is called.
const PLATFORM: &[&str] = &[
    "x86_64", "x86", "x64", "64", "amd64", "i386", "i686", "aarch64", "arm64", "armhf", "linux", "linux64", "appimage",
];

/// The app's name and version from an AppImage's file name: `Obsidian-1.5.3.AppImage` is
/// Obsidian 1.5.3, `Foo_x86_64.AppImage` is Foo with no version.
pub fn parse_name(file_name: &str) -> (String, String) {
    let stem = match file_name.rsplit_once('.') {
        Some((s, x)) if x.eq_ignore_ascii_case("appimage") => s,
        _ => file_name,
    };
    // `x86_64` would otherwise be split in two at its underscore
    let stem = stem.replace("x86_64", "x86-64");
    let words: Vec<String> = stem
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| w.replace("x86-64", "x86_64"))
        .filter(|w| !PLATFORM.iter().any(|p| p.eq_ignore_ascii_case(w)))
        .collect();
    let digit = |w: &String| {
        let w = w.strip_prefix(['v', 'V']).unwrap_or(w);
        w.starts_with(|c: char| c.is_ascii_digit())
    };
    // the first word is always part of the name, even `7zip`
    let split = words.iter().skip(1).position(digit).map_or(words.len(), |i| i + 1);
    let title = words[..split].join(" ");
    let version = words.get(split).map(|v| v.strip_prefix(['v', 'V']).unwrap_or(v).to_string()).unwrap_or_default();
    if title.is_empty() { (stem.to_string(), version) } else { (title, version) }
}

/// Folders people keep AppImages in, with how many folder levels to look below each.
fn folders(home: &Path) -> Vec<(PathBuf, u8)> {
    [("Applications", 1), ("AppImages", 1), (".local/bin", 0), ("bin", 0), ("Downloads", 0), ("Desktop", 0)]
        .iter()
        .map(|(d, depth)| (home.join(d), *depth))
        .collect()
}

fn find(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            if depth > 0 {
                find(&p, depth - 1, out);
            }
        } else if t.is_file() && is_appimage(&p) {
            out.push(p);
        }
    }
}

fn package(id: String, file: &Path, home: &Path) -> Package {
    let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (title, version) = parse_name(&file_name);
    let mut p = Package::new(Kind::AppImage, id);
    p.role = Role::App;
    p.user_installed = true;
    p.name = title.to_lowercase();
    p.title = title;
    p.version = version;
    p.size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    p.location = file.to_string_lossy().into_owned();
    if !file.starts_with(home) {
        p.blocked = Some("This AppImage is in a system folder, so removing it needs an administrator.".into());
    }
    p
}

/// An AppImage started by launcher `entry`. `icon_users` counts the launchers using each icon,
/// so an icon another launcher needs is never removed.
pub fn from_launcher(entry: &DesktopEntry, file: &Path, home: &Path, icon_users: &HashMap<String, usize>) -> Package {
    let base = entry.id.strip_suffix(".desktop").unwrap_or(&entry.id);
    let mut p = package(base.to_string(), file, home);
    if !entry.name.is_empty() {
        p.title = entry.name.clone();
    }
    p.summary = if entry.comment.is_empty() { entry.generic_name.clone() } else { entry.comment.clone() };
    p.icon = (!entry.icon.is_empty()).then(|| entry.icon.clone());
    p.desktop = Some(entry.path.clone());
    if p.blocked.is_none() && !entry.path.starts_with(home) {
        p.blocked = Some(
            "This launcher is in a system folder and wasn't installed by a package manager, so Depot can't remove it safely."
                .into(),
        );
    }
    let mut files = vec![entry.path.clone(), file.to_path_buf()];
    files.extend(super::manual::belongings(entry, file, home, icon_users));
    p.manual = Some(ManualPlan { appdir: None, files, data: super::manual::guess_data(home, &p.name) });
    p
}

/// AppImages in the usual folders that no launcher starts. `claimed` are the (resolved) files
/// launchers already start.
pub fn loose(home: &Path, claimed: &HashSet<PathBuf>) -> Vec<Package> {
    let mut found = Vec::new();
    for (dir, depth) in folders(home) {
        find(&dir, depth, &mut found);
    }
    let mut seen = HashSet::new();
    found
        .into_iter()
        .filter(|f| {
            let real = std::fs::canonicalize(f).unwrap_or_else(|_| f.clone());
            !claimed.contains(&real) && seen.insert(real)
        })
        .map(|f| {
            let mut p = package(f.to_string_lossy().into_owned(), &f, home);
            p.summary = "Not in your app menu.".into();
            let links = super::manual::belongings(&DesktopEntry::default(), &f, home, &HashMap::new());
            let mut files = vec![f.clone()];
            files.extend(links);
            p.manual = Some(ManualPlan { appdir: None, files, data: super::manual::guess_data(home, &p.name) });
            p
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> (String, String) {
        parse_name(s)
    }

    #[test]
    fn names_and_versions() {
        assert_eq!(n("Obsidian-1.5.3.AppImage"), ("Obsidian".into(), "1.5.3".into()));
        assert_eq!(n("Foo_x86_64.AppImage"), ("Foo".into(), "".into()));
        assert_eq!(n("Bar-v2.0-x86_64.AppImage"), ("Bar".into(), "2.0".into()));
        assert_eq!(n("Kdenlive-24.08.1-x86_64.appimage"), ("Kdenlive".into(), "24.08.1".into()));
        assert_eq!(n("balena-etcher-1.18.11-x64.AppImage"), ("balena etcher".into(), "1.18.11".into()));
        assert_eq!(n("7zip.AppImage"), ("7zip".into(), "".into()));
    }

    #[test]
    fn recognises_the_appimage_mark() {
        let dir = std::env::temp_dir().join(format!("depot-appimage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("a");
        std::fs::write(&mark, b"\x7fELF\x02\x01\x01\x00AI\x02rest").unwrap();
        let plain = dir.join("b.AppImage");
        std::fs::write(&plain, b"\x7fELF\x02\x01\x01\x00\x00\x00\x00rest").unwrap();
        let short = dir.join("c.AppImage");
        std::fs::write(&short, b"#!").unwrap();
        assert!(is_appimage(&mark));
        assert!(!is_appimage(&plain), "a plain program named .AppImage isn't one");
        assert!(is_appimage(&short), "too short to tell: the name decides");
        assert!(!is_appimage(&dir.join("missing.AppImage")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
