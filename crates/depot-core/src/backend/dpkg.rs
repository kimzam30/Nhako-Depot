//! Debian packages, read straight from dpkg's database files.

use crate::model::{Kind, Package};
use crate::owners;
use std::collections::{HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub fn native_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "x86" => "i386",
        "aarch64" => "arm64",
        "arm" => "armhf",
        "powerpc64" => "ppc64el",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        "loongarch64" => "loong64",
        other => other,
    }
}

/// The name apt and dpkg accept for a package: bare for native and `all`, `name:arch` otherwise.
pub fn pkg_id(name: &str, arch: &str) -> String {
    if arch.is_empty() || arch == "all" || arch == native_arch() { name.to_string() } else { format!("{name}:{arch}") }
}

/// Packages apt marked as automatically installed, as `name:arch`.
pub fn parse_extended_states(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for stanza in text.split("\n\n") {
        let (mut name, mut arch, mut auto) = ("", "", false);
        for line in stanza.lines() {
            if let Some((k, v)) = line.split_once(':') {
                match k {
                    "Package" => name = v.trim(),
                    "Architecture" => arch = v.trim(),
                    "Auto-Installed" => auto = v.trim() == "1",
                    _ => {}
                }
            }
        }
        if auto && !name.is_empty() {
            out.insert(format!("{name}:{arch}"));
        }
    }
    out
}

/// Installed packages from `/var/lib/dpkg/status`.
pub fn parse_status(text: &str, auto: &HashSet<String>) -> Vec<Package> {
    let native = native_arch();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for stanza in text.split("\n\n") {
        let mut f: HashMap<&str, &str> = HashMap::new();
        let mut desc = String::new();
        let mut in_desc = false;
        for line in stanza.lines() {
            if let Some(cont) = line.strip_prefix(' ') {
                if in_desc {
                    desc.push('\n');
                    desc.push_str(if cont.trim() == "." { "" } else { cont.trim() });
                }
                continue;
            }
            in_desc = false;
            if let Some((k, v)) = line.split_once(':') {
                let v = v.trim();
                if k == "Description" {
                    in_desc = true;
                    desc.push_str(v);
                }
                f.insert(k, v);
            }
        }
        let status = f.get("Status").copied().unwrap_or("");
        let mut st = status.split(' ');
        let (want, state) = (st.next().unwrap_or(""), st.nth(1).unwrap_or(""));
        if state != "installed" || !matches!(want, "install" | "hold") {
            continue;
        }
        let Some(&name) = f.get("Package") else { continue };
        let arch = f.get("Architecture").copied().unwrap_or("");
        let id = pkg_id(name, arch);
        if !seen.insert(id.clone()) {
            continue;
        }
        let (summary, long) = desc.split_once('\n').unwrap_or((&desc, ""));
        let mut p = Package::new(Kind::Deb, id);
        p.name = name.to_string();
        p.arch = arch.to_string();
        p.version = f.get("Version").unwrap_or(&"").to_string();
        p.size = f.get("Installed-Size").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 1024;
        p.summary = summary.to_string();
        p.description = long.trim().to_string();
        p.section = f.get("Section").unwrap_or(&"").to_string();
        p.homepage = f.get("Homepage").unwrap_or(&"").to_string();
        p.publisher = f.get("Maintainer").unwrap_or(&"").to_string();
        let a = if arch == "all" { native } else { arch };
        p.user_installed = !auto.contains(&format!("{name}:{a}")) && !auto.contains(&format!("{name}:{arch}"));
        if f.get("Essential") == Some(&"yes") {
            p.blocked = Some("This package is essential for the system.".into());
        } else if f.get("Protected") == Some(&"yes") {
            p.blocked = Some("This package is protected: your system needs it to start or run.".into());
        }
        out.push(p);
    }
    out
}

pub fn load(dpkg_dir: &Path, apt_states: &Path) -> Result<Vec<Package>, String> {
    let status = dpkg_dir.join("status");
    if !status.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read(&status).map_err(|e| format!("Couldn't read {}: {e}", status.display()))?;
    let auto =
        std::fs::read(apt_states).map(|b| parse_extended_states(&String::from_utf8_lossy(&b))).unwrap_or_default();
    Ok(parse_status(&String::from_utf8_lossy(&text), &auto))
}

/// Find which package owns each wanted path by scanning dpkg's file lists in parallel.
pub fn scan_owners(info_dir: &Path, wanted: &HashSet<PathBuf>) -> HashMap<PathBuf, String> {
    let lists: Vec<PathBuf> = match std::fs::read_dir(info_dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "list")).collect(),
        Err(_) => return HashMap::new(),
    };
    if wanted.is_empty() {
        return HashMap::new();
    }
    let keys: HashSet<&[u8]> = wanted.iter().map(|p| p.as_os_str().as_bytes()).collect();
    let native = format!(":{}", native_arch());
    let mut found = owners::parallel(&lists, |f| {
        let Ok(bytes) = std::fs::read(f) else { return Vec::new() };
        let stem = f.file_stem().unwrap_or_default().to_string_lossy();
        let owner = stem.strip_suffix(native.as_str()).unwrap_or(&stem).to_string();
        bytes
            .split(|b| *b == b'\n')
            .filter(|line| keys.contains(line))
            .map(|line| (PathBuf::from(std::ffi::OsStr::from_bytes(line)), owner.clone()))
            .collect()
    });
    // Directories and diverted files can appear in several lists; make the pick stable.
    found.sort();
    let mut out = HashMap::new();
    for (p, o) in found {
        out.entry(p).or_insert(o);
    }
    out
}

/// [`scan_owners`] with an on-disk cache that stays valid until dpkg's database changes.
pub fn owners(dpkg_dir: &Path, cache: Option<&Path>, wanted: &HashSet<PathBuf>) -> HashMap<PathBuf, String> {
    let info = dpkg_dir.join("info");
    let stamp =
        format!("depot-dpkg-owners-v1 {} {}", owners::mtime_ns(&dpkg_dir.join("status")), owners::mtime_ns(&info));
    owners::cached(cache, &stamp, wanted, |misses| scan_owners(&info, misses))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "Package: nuvio\nStatus: install ok installed\nArchitecture: amd64\nVersion: 1.1.26-1\nInstalled-Size: 300\nDescription: Nuvio\n\nPackage: libc6\nStatus: install ok installed\nArchitecture: amd64\nEssential: yes\nVersion: 2.39\nDescription: GNU C Library\n shared libraries\n .\n more\n\nPackage: gone\nStatus: deinstall ok config-files\nArchitecture: amd64\n\nPackage: held\nStatus: hold ok installed\nArchitecture: i386\nDescription: x\n";

    #[test]
    fn status_parsing() {
        let auto = parse_extended_states("Package: libc6\nArchitecture: amd64\nAuto-Installed: 1\n");
        let p = parse_status(STATUS, &auto);
        let ids: Vec<_> = p.iter().map(|p| p.id.as_str()).collect();
        if native_arch() == "amd64" {
            assert_eq!(ids, ["nuvio", "libc6", "held:i386"]);
        }
        assert_eq!(p[0].size, 300 * 1024);
        assert!(p[0].user_installed);
        assert!(!p[1].user_installed);
        assert!(p[1].blocked.is_some());
        assert_eq!(p[1].summary, "GNU C Library");
        assert_eq!(p[1].description, "shared libraries\n\nmore");
    }
}
