//! RPM packages (Fedora, openSUSE, RHEL and friends), from one `rpm -qa` call.
//! The database itself is SQLite, so it's read through rpm rather than directly.

use crate::model::{Kind, Package};
use crate::owners;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Packages that must never be removed from Depot, on top of dnf's `protected.d` lists.
const ALWAYS_PROTECTED: &[&str] =
    &["kernel-core", "systemd", "glibc", "rpm", "dnf", "dnf5", "zypper", "filesystem", "setup", "sudo"];

const RECORD: &str = "@@depot@@";

/// Query format: a marker line, one tab-separated line of fields, then the description.
const FORMAT: &str = "@@depot@@\\n%{NAME}\\t%{VERSION}-%{RELEASE}\\t%{ARCH}\\t%{SIZE}\\t%{URL}\\t%{GROUP}\\t%{VENDOR}\\t%{SUMMARY}\\n%{DESCRIPTION}\\n";

/// The architecture rpm calls this machine.
pub fn native_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "i686",
        "powerpc64" => "ppc64le",
        other => other,
    }
}

/// The name dnf accepts for a package: bare for native and `noarch`, `name.arch` otherwise.
pub fn pkg_id(name: &str, arch: &str) -> String {
    if arch.is_empty() || arch == "noarch" || arch == native_arch() {
        name.to_string()
    } else {
        format!("{name}.{arch}")
    }
}

fn rpm_value(v: &str) -> &str {
    if v == "(none)" { "" } else { v }
}

/// Packages from `rpm -qa --queryformat FORMAT` output. `protected` names packages the system needs.
pub fn parse(text: &str, protected: &HashSet<String>) -> Vec<Package> {
    let mut out: Vec<Package> = Vec::new();
    // several versions of one package can be installed side by side (kernels); they're one card
    let mut seen: HashMap<String, usize> = HashMap::new();
    for rec in text.split(RECORD).map(|r| r.trim_start_matches('\n')).filter(|r| !r.is_empty()) {
        let (head, desc) = rec.split_once('\n').unwrap_or((rec, ""));
        let f: Vec<&str> = head.split('\t').map(rpm_value).collect();
        let [name, version, arch, size, url, group, vendor, summary, ..] = f[..] else { continue };
        // the keys rpm uses to check signatures show up as packages; they aren't software
        if name.is_empty() || name == "gpg-pubkey" {
            continue;
        }
        let id = pkg_id(name, arch);
        if let Some(&i) = seen.get(&id) {
            let q = &mut out[i];
            q.version = format!("{}, {version}", q.version);
            q.size += size.parse::<u64>().unwrap_or(0);
            continue;
        }
        seen.insert(id.clone(), out.len());
        let mut p = Package::new(Kind::Rpm, id);
        p.name = name.to_string();
        p.arch = arch.to_string();
        p.version = version.to_string();
        p.size = size.parse().unwrap_or(0);
        p.homepage = url.to_string();
        p.section = if group == "Unspecified" { String::new() } else { group.to_string() };
        p.publisher = vendor.to_string();
        p.summary = summary.to_string();
        p.description = rpm_value(desc.trim()).to_string();
        if protected.contains(name) || ALWAYS_PROTECTED.contains(&name) {
            p.blocked = Some("This package is protected: your system needs it to start or run.".into());
        }
        out.push(p);
    }
    out
}

/// Names in dnf's `protected.d/*.conf` files: one per line, `#` starts a comment.
pub fn parse_protected(text: &str) -> HashSet<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn protected_names() -> HashSet<String> {
    ["/etc/dnf/protected.d", "/etc/dnf/dnf5-protected.d", "/usr/share/dnf5/dnf5-protected.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .flat_map(|t| parse_protected(&t))
        .collect()
}

fn rpm(args: &[&str]) -> Result<String, String> {
    match Command::new("rpm").args(args).env("LC_ALL", "C.UTF-8").output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(format!("rpm failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("Couldn't run rpm: {e}")),
    }
}

/// Installed packages. `db` is the rpm database folder; nothing is read if it doesn't exist.
pub fn load(db: &Path) -> Result<Vec<Package>, String> {
    if !db.is_dir() {
        return Ok(Vec::new());
    }
    let text = rpm(&["-qa", "--queryformat", FORMAT])?;
    Ok(parse(&text, &protected_names()))
}

/// Owners of the wanted paths from `rpm -qa` file-list output (`path<TAB>name` per line).
pub fn parse_file_owners(text: &str, wanted: &HashSet<PathBuf>) -> HashMap<PathBuf, String> {
    let keys: HashSet<&str> = wanted.iter().filter_map(|p| p.to_str()).collect();
    owners::first_owner(
        text.lines()
            .filter_map(|l| l.split_once('\t'))
            .filter(|(p, _)| keys.contains(p))
            .map(|(p, n)| (PathBuf::from(p), n.to_string()))
            .collect(),
    )
}

/// Which package owns each wanted path. rpm can only list every file of every package, which
/// takes a second or two, so the answers are cached until the database changes.
pub fn owners(
    db: &Path,
    cache: Option<&Path>,
    wanted: &HashSet<PathBuf>,
) -> (HashMap<PathBuf, String>, Option<String>) {
    owners::cached(cache, &stamp(db), wanted, |misses| {
        // owners are package names, not dnf ids; multilib copies of one name own the same files
        rpm(&["-qa", "--queryformat", "[%{FILENAMES}\\t%{NAME}\\n]"]).map(|t| parse_file_owners(&t, misses))
    })
}

/// Identifies the database state. Fedora's rpm writes `rpmdb.sqlite`; openSUSE's ndb backend
/// rewrites `Packages.db` in place, so the folder's own time doesn't change there.
fn stamp(db: &Path) -> String {
    let times: Vec<String> = ["", "rpmdb.sqlite", "rpmdb.sqlite-wal", "Packages.db", "Packages"]
        .iter()
        .map(|f| owners::mtime_ns(&db.join(f)).to_string())
        .collect();
    format!("depot-rpm-owners-v2 {}", times.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing() {
        let arch = native_arch();
        let t = format!(
            "@@depot@@\nfirefox\t130.0-1.fc40\t{arch}\t250000000\thttps://mozilla.org\tUnspecified\tFedora Project\tWeb browser\nMozilla Firefox is a browser.\nSecond line.\n\
             @@depot@@\nglibc\t2.39-1\ti686\t100\t(none)\tSystem\tFedora Project\tC library\n(none)\n\
             @@depot@@\ngpg-pubkey\t1-1\t(none)\t0\t(none)\t(none)\t(none)\tkey\n\n"
        );
        let protected = parse_protected("# keep these\nsudo\nglibc  # libc\n");
        let p = parse(&t, &protected);
        assert_eq!(p.len(), 2, "signing keys aren't packages");
        assert_eq!((p[0].id.as_str(), p[0].version.as_str()), ("firefox", "130.0-1.fc40"));
        assert_eq!(p[0].description, "Mozilla Firefox is a browser.\nSecond line.");
        assert!(p[0].section.is_empty() && p[0].blocked.is_none());
        if arch != "i686" {
            assert_eq!(p[1].id, "glibc.i686");
        }
        assert!(p[1].homepage.is_empty() && p[1].description.is_empty());
        assert!(p[1].blocked.is_some());
    }

    #[test]
    fn several_versions_are_one_package() {
        let arch = native_arch();
        let t = format!(
            "@@depot@@\nkernel\t6.8-1\t{arch}\t10\t\t\t\tLinux\n\n@@depot@@\nkernel\t6.9-1\t{arch}\t20\t\t\t\tLinux\n\n"
        );
        let p = parse(&t, &HashSet::new());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].version, "6.8-1, 6.9-1");
        assert_eq!(p[0].size, 30);
    }

    #[test]
    fn file_owner_parsing() {
        let wanted: HashSet<PathBuf> = [PathBuf::from("/usr/bin/firefox"), PathBuf::from("/nope")].into();
        let o = parse_file_owners("/usr/bin/firefox\tfirefox\n/usr/lib/x\tother\n", &wanted);
        assert_eq!(o.len(), 1);
        assert_eq!(o[Path::new("/usr/bin/firefox")], "firefox");
    }
}
