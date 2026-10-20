//! Arch Linux packages, read straight from pacman's local database (`/var/lib/pacman/local`).

use crate::model::{Kind, Package};
use crate::owners;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Packages that must never be removed from Depot, on top of pacman.conf's `HoldPkg`.
const ALWAYS_HELD: &[&str] = &["base", "filesystem", "glibc", "pacman", "systemd", "linux", "linux-lts", "linux-zen"];

/// Sections of a pacman `desc` file: `%KEY%` followed by value lines, ended by a blank line.
fn sections(text: &str) -> HashMap<&str, Vec<&str>> {
    let mut out: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut key = None;
    for line in text.lines() {
        if let Some(k) = line.strip_prefix('%').and_then(|l| l.strip_suffix('%')) {
            key = Some(k);
            out.entry(k).or_default();
        } else if line.is_empty() {
            key = None;
        } else if let Some(k) = key {
            out.entry(k).or_default().push(line);
        }
    }
    out
}

/// One package from its `desc` file. `held` names packages the system can't run without.
pub fn parse_desc(text: &str, held: &HashSet<String>, foreign: &HashSet<String>) -> Option<Package> {
    let s = sections(text);
    let first = |k: &str| s.get(k).and_then(|v| v.first()).copied().unwrap_or("");
    let name = first("NAME");
    if name.is_empty() {
        return None;
    }
    let mut p = Package::new(Kind::Pacman, name);
    p.version = first("VERSION").to_string();
    p.summary = first("DESC").to_string();
    p.homepage = first("URL").to_string();
    p.arch = first("ARCH").to_string();
    p.publisher = first("PACKAGER").to_string();
    p.section = s.get("GROUPS").map(|g| g.join(", ")).unwrap_or_default();
    p.size = first("SIZE").parse().unwrap_or(0);
    // %REASON% 1 means it was pulled in as a dependency; no reason means installed explicitly
    p.user_installed = first("REASON") != "1";
    p.foreign = foreign.contains(name);
    if p.foreign {
        p.channel = "AUR or another source outside your repositories".into();
    }
    if held.contains(name) || ALWAYS_HELD.contains(&name) {
        p.blocked = Some("Your system needs this package to start or run.".into());
    }
    Some(p)
}

/// Names listed on `HoldPkg` lines of pacman.conf.
pub fn parse_holdpkg(conf: &str) -> HashSet<String> {
    conf.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == "HoldPkg").then_some(v)
        })
        .flat_map(|v| v.split_whitespace().map(str::to_string))
        .collect()
}

/// Packages not found in any configured repository (`pacman -Qmq`), usually AUR builds.
fn foreign_names() -> HashSet<String> {
    Command::new("pacman")
        .args(["-Qmq"])
        .env("LC_ALL", "C")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Package folders in the local database.
fn entries(local: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(local)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("desc").is_file()).collect())
        .unwrap_or_default()
}

/// Installed packages. `ask_pacman` also asks pacman which ones came from outside the repositories.
pub fn load(dir: &Path, conf: &Path, ask_pacman: bool) -> Result<Vec<Package>, String> {
    let local = dir.join("local");
    if !local.is_dir() {
        return Ok(Vec::new());
    }
    let held = std::fs::read_to_string(conf).map(|t| parse_holdpkg(&t)).unwrap_or_default();
    let foreign = if ask_pacman { foreign_names() } else { HashSet::new() };
    let mut out: Vec<Package> = entries(&local)
        .iter()
        .filter_map(|e| parse_desc(&std::fs::read_to_string(e.join("desc")).ok()?, &held, &foreign))
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Package name from a database folder name: `<name>-<version>-<release>`.
fn folder_name(dir: &Path) -> Option<String> {
    let f = dir.file_name()?.to_str()?;
    let mut parts = f.rsplitn(3, '-');
    let (_rel, _ver, name) = (parts.next()?, parts.next()?, parts.next()?);
    Some(name.to_string())
}

/// Find which package owns each wanted path by scanning the `files` lists in parallel.
/// Paths in those lists have no leading slash.
pub fn scan_owners(local: &Path, wanted: &HashSet<PathBuf>) -> HashMap<PathBuf, String> {
    if wanted.is_empty() {
        return HashMap::new();
    }
    let keys: HashSet<String> =
        wanted.iter().filter_map(|p| p.to_str()?.strip_prefix('/').map(str::to_string)).collect();
    let mut found = owners::parallel(&entries(local), |e| {
        let (Some(name), Ok(text)) = (folder_name(e), std::fs::read_to_string(e.join("files"))) else {
            return Vec::new();
        };
        let files = sections(&text).remove("FILES").unwrap_or_default();
        files
            .into_iter()
            .filter(|f| keys.contains(*f))
            .map(|f| (PathBuf::from(format!("/{f}")), name.clone()))
            .collect()
    });
    found.sort();
    let mut out = HashMap::new();
    for (p, o) in found {
        out.entry(p).or_insert(o);
    }
    out
}

/// [`scan_owners`] with an on-disk cache that stays valid until the local database changes.
pub fn owners(dir: &Path, cache: Option<&Path>, wanted: &HashSet<PathBuf>) -> HashMap<PathBuf, String> {
    let local = dir.join("local");
    let stamp = format!("depot-pacman-owners-v1 {}", owners::mtime_ns(&local));
    owners::cached(cache, &stamp, wanted, |misses| scan_owners(&local, misses))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = "%NAME%\nfirefox\n\n%VERSION%\n130.0-1\n\n%DESC%\nFast web browser\n\n%URL%\nhttps://mozilla.org\n\n%ARCH%\nx86_64\n\n%SIZE%\n250000000\n\n%GROUPS%\nweb\n";

    #[test]
    fn desc_parsing() {
        let none = HashSet::new();
        let p = parse_desc(DESC, &none, &none).unwrap();
        assert_eq!((p.id.as_str(), p.version.as_str(), p.arch.as_str()), ("firefox", "130.0-1", "x86_64"));
        assert_eq!(p.size, 250_000_000);
        assert!(p.user_installed && !p.foreign && p.blocked.is_none());
        let dep = parse_desc(&format!("{DESC}\n%REASON%\n1\n"), &none, &none).unwrap();
        assert!(!dep.user_installed);
        let aur: HashSet<String> = ["firefox".to_string()].into();
        assert!(parse_desc(DESC, &none, &aur).unwrap().foreign);
        let held = parse_holdpkg("[options]\nHoldPkg     = pacman firefox\n#HoldPkg = nope\n");
        assert!(held.contains("firefox") && !held.contains("nope"));
        assert!(parse_desc(DESC, &held, &none).unwrap().blocked.is_some());
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name(Path::new("/x/lib32-gcc-libs-14.2.1-1")).as_deref(), Some("lib32-gcc-libs"));
        assert_eq!(folder_name(Path::new("/x/firefox-1:130.0-1")).as_deref(), Some("firefox"));
    }
}
