//! Arch Linux packages, read straight from pacman's local database (`/var/lib/pacman/local`).

use crate::command;
use crate::model::{Kind, Package};
use crate::owners;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

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
/// Without downloaded repository lists every package looks foreign, so then nothing is.
fn foreign_names(dir: &Path) -> HashSet<String> {
    let synced = std::fs::read_dir(dir.join("sync"))
        .is_ok_and(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "db")));
    if !synced {
        return HashSet::new();
    }
    // exit code 1 with no output just means there are none
    let args = [OsStr::new("--dbpath"), dir.as_os_str(), OsStr::new("-Qmq")];
    let text = command::output("pacman", &args).ok().flatten().unwrap_or_default();
    text.lines().map(str::to_string).collect()
}

/// Package folders in the local database.
fn entries(local: &Path) -> std::io::Result<Vec<PathBuf>> {
    Ok(std::fs::read_dir(local)?.flatten().map(|e| e.path()).filter(|p| p.join("desc").is_file()).collect())
}

/// Installed packages. `ask_pacman` also asks pacman which ones came from outside the repositories.
pub fn load(dir: &Path, conf: &Path, ask_pacman: bool) -> Result<Vec<Package>, String> {
    let local = dir.join("local");
    if !local.is_dir() {
        return Ok(Vec::new());
    }
    let held = std::fs::read_to_string(conf).map(|t| parse_holdpkg(&t)).unwrap_or_default();
    let foreign = if ask_pacman { foreign_names(dir) } else { HashSet::new() };
    let mut out: Vec<Package> = entries(&local)
        .map_err(|e| format!("Couldn't read {}: {e}", local.display()))?
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

/// Paths in a `files` list (no leading slash), as raw bytes: file names needn't be UTF-8.
fn file_list(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut lines = bytes.split(|b| *b == b'\n');
    lines.by_ref().find(|l| *l == b"%FILES%");
    lines.take_while(|l| !l.is_empty())
}

/// Find which package owns each wanted path by scanning the `files` lists in parallel. A list
/// that can't be read leaves out only that package's files.
pub fn scan_owners(local: &Path, wanted: &HashSet<PathBuf>) -> owners::Found {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    if wanted.is_empty() {
        return (HashMap::new(), None);
    }
    let keys: HashSet<&[u8]> = wanted.iter().filter_map(|p| p.as_os_str().as_bytes().strip_prefix(b"/")).collect();
    let dirs = match entries(local) {
        Ok(d) => d,
        Err(e) => return (HashMap::new(), Some(format!("Couldn't read {}: {e}", local.display()))),
    };
    let (found, error) = owners::parallel(&dirs, |e| {
        let Some(name) = folder_name(e) else { return Ok(Vec::new()) };
        let bytes = match std::fs::read(e.join("files")) {
            Ok(b) => b,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err("Some of pacman's file lists couldn't be read.".into()),
        };
        Ok(file_list(&bytes)
            .filter(|f| keys.contains(f))
            .map(|f| (PathBuf::from(std::ffi::OsString::from_vec([b"/", f].concat())), name.clone()))
            .collect())
    });
    (owners::first_owner(found), error)
}

/// [`scan_owners`] with an on-disk cache that stays valid until the local database changes.
pub fn owners(
    dir: &Path,
    cache: Option<&Path>,
    wanted: &HashSet<PathBuf>,
) -> (HashMap<PathBuf, String>, Option<String>) {
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
