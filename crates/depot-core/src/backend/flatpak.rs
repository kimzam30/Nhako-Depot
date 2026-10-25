//! Flatpak apps and runtimes, from one `flatpak list` call.

use crate::model::{Kind, Package, Role};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn load() -> Result<Vec<Package>, String> {
    let out = match Command::new("flatpak")
        .args(["list", "--columns=ref,name,version,description,origin,installation,size,options"])
        .env("LC_ALL", "C.UTF-8")
        .env("LANGUAGE", "C")
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Couldn't run flatpak: {e}")),
    };
    if !out.status.success() {
        return Err(format!("flatpak list failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse(text: &str) -> Vec<Package> {
    text.lines()
        .filter_map(|line| {
            let c: Vec<&str> = line.split('\t').map(str::trim).collect();
            let [r#ref, name, ver, desc, origin, inst, size, ..] = c[..] else { return None };
            let options = c.get(7).copied().unwrap_or("");
            // `ref` is printed without its app/ or runtime/ prefix; the options column says which.
            let r#ref =
                r#ref.split_once('/').filter(|(t, _)| matches!(*t, "app" | "runtime")).map_or(r#ref, |(_, r)| r);
            let mut parts = r#ref.split('/');
            let (appid, _arch, branch) = (parts.next()?, parts.next()?, parts.next().unwrap_or(""));
            let runtime = options.split(',').any(|o| o == "runtime");
            let mut p = Package::new(Kind::Flatpak, r#ref);
            p.name = appid.to_string();
            p.title = if name.is_empty() { appid.to_string() } else { name.to_string() };
            p.version = ver.to_string();
            p.summary = desc.to_string();
            p.channel = origin.to_string();
            p.section = inst.to_string();
            p.system = inst != "user";
            p.current = options.split(',').any(|o| o == "current");
            p.revision = branch.to_string();
            p.size = parse_size(size);
            if let Some((_, eol)) = options.split_once("eol=") {
                p.description = format!("⚠ End of life: {eol}");
            }
            if runtime {
                p.role = Role::Runtime;
                p.blocked = Some(
                    "Runtimes are shared by Flatpak apps. To remove the ones no app needs, use Clean up on the Runtimes & bases page."
                        .into(),
                );
            } else {
                p.user_installed = true;
            }
            Some(p)
        })
        .collect()
}

/// What keeps runtimes installed: the runtimes apps run on, and refs pinned with `flatpak pin`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Runtime refs (`id/arch/branch`) that an installed app runs on, in either installation.
    pub runtimes: HashSet<String>,
    /// Pin patterns (`runtime/id/arch/branch`, may contain `*`) and whether they're system-wide.
    pub pins: Vec<(String, bool)>,
}

/// Read what keeps runtimes installed. Flatpak missing or failing just means nothing is known,
/// so nothing will be offered for clean-up.
pub fn usage() -> Option<Usage> {
    let run = |args: &[&str]| {
        let out = Command::new("flatpak").args(args).env("LC_ALL", "C.UTF-8").env("LANGUAGE", "C").output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let apps = run(&["list", "--app", "--columns=runtime"])?;
    let mut u = Usage { runtimes: parse_app_runtimes(&apps), pins: Vec::new() };
    for system in [true, false] {
        // an installation that doesn't exist yet has no pins, but then it has no runtimes either
        let text = run(&["pin", if system { "--system" } else { "--user" }]).unwrap_or_default();
        u.pins.extend(parse_pins(&text).into_iter().map(|p| (p, system)));
    }
    Some(u)
}

pub fn parse_app_runtimes(text: &str) -> HashSet<String> {
    text.lines().map(str::trim).filter(|l| l.split('/').count() == 3).map(str::to_string).collect()
}

pub fn parse_pins(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| l.starts_with("runtime/")).map(str::to_string).collect()
}

/// `*` matches any run of characters, as in `flatpak pin` patterns.
fn glob(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => text
            .strip_prefix(head)
            .is_some_and(|rest| (0..=rest.len()).any(|i| rest.is_char_boundary(i) && glob(tail, &rest[i..]))),
    }
}

/// Runtimes that `flatpak uninstall --unused` would remove, as indices into `pkgs`: shown
/// before cleaning up, while Flatpak itself still decides. When in doubt a runtime counts as
/// used, so the list may be shorter than Flatpak's but never names one it would keep.
///
/// A runtime is unused when it isn't pinned and either
/// - it's a base (`….Platform`, `….Sdk`, `….BaseApp`) no app runs on, or
/// - it extends another installed ref (`org.freedesktop.Platform.GL.default` extends
///   `org.freedesktop.Platform`) whose matching branch is installed and itself unused.
pub fn unused(pkgs: &[Package], usage: &Usage) -> Vec<usize> {
    let flats: Vec<usize> = (0..pkgs.len()).filter(|&i| pkgs[i].kind == Kind::Flatpak).collect();
    let pinned = |p: &Package| {
        let r = format!("runtime/{}", p.id);
        usage.pins.iter().any(|(pat, system)| *system == p.system && glob(pat, &r))
    };
    // the longest installed id this one extends, e.g. `org.freedesktop.Platform` for `….Platform.GL.default`
    fn parent<'a>(p: &'a Package, pkgs: &[Package], flats: &[usize]) -> Option<&'a str> {
        flats
            .iter()
            .map(|&j| &pkgs[j])
            .filter(|q| p.name.strip_prefix(q.name.as_str()).is_some_and(|r| r.starts_with('.')))
            .map(|q| q.name.len())
            .max()
            .map(|n| &p.name[..n])
    }
    let unused_base = |p: &Package| {
        let base = p.name.rsplit('.').next().is_some_and(|s| matches!(s, "Platform" | "Sdk" | "BaseApp"));
        p.role == Role::Runtime
            && base
            && parent(p, pkgs, &flats).is_none()
            && !usage.runtimes.contains(&p.id)
            && !pinned(p)
    };
    // `25.08-extra` and `24.08extra` are built for `25.08` and `24.08`, but `6.10` isn't for `6.1`
    let built_for = |ext: &str, branch: &str| {
        ext.strip_prefix(branch).is_some_and(|r| r.chars().next().is_none_or(|c| !c.is_ascii_digit() && c != '.'))
    };
    flats
        .iter()
        .copied()
        .filter(|&i| {
            let p = &pkgs[i];
            if p.role != Role::Runtime || pinned(p) {
                return false;
            }
            match parent(p, pkgs, &flats) {
                Some(name) => {
                    let mut copies = flats
                        .iter()
                        .map(|&j| &pkgs[j])
                        .filter(|q| q.name == name && q.system == p.system && built_for(&p.revision, &q.revision))
                        .peekable();
                    copies.peek().is_some() && copies.all(unused_base)
                }
                None => unused_base(p),
            }
        })
        .collect()
}

/// A Flatpak ref with a newer commit on its remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlatpakUpdate {
    pub r#ref: String,
    pub appid: String,
    pub version: String,
    pub runtime: bool,
    pub system: bool,
}

/// Refs with updates in one installation. Contacts the remotes, so it can be slow.
pub fn updates(system: bool) -> Result<Vec<FlatpakUpdate>, String> {
    let out = match Command::new("flatpak")
        .args([
            "remote-ls",
            if system { "--system" } else { "--user" },
            "--updates",
            "--columns=ref,application,version",
        ])
        .env("LC_ALL", "C.UTF-8")
        .env("LANGUAGE", "C")
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Couldn't run flatpak: {e}")),
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // an installation with no remotes isn't a problem
        if err.contains("No remote") || err.trim().is_empty() {
            return Ok(Vec::new());
        }
        return Err(format!("Flatpak couldn't check for updates: {}", err.trim()));
    }
    Ok(parse_updates(&String::from_utf8_lossy(&out.stdout), system))
}

pub fn parse_updates(text: &str, system: bool) -> Vec<FlatpakUpdate> {
    text.lines()
        .filter_map(|line| {
            let mut c = line.split('\t').map(str::trim);
            let full = c.next().filter(|r| !r.is_empty())?;
            let appid = c.next().unwrap_or("").to_string();
            let version = c.next().unwrap_or("").to_string();
            let runtime = full.starts_with("runtime/");
            let r#ref = full.split_once('/').filter(|(t, _)| matches!(*t, "app" | "runtime")).map_or(full, |(_, r)| r);
            Some(FlatpakUpdate { r#ref: r#ref.to_string(), appid, version, runtime, system })
        })
        .collect()
}

/// Parse sizes like `1.2 GB` or `512.0 kB`.
pub fn parse_size(s: &str) -> u64 {
    let s = s.replace('\u{a0}', " ");
    let mut it = s.split_whitespace();
    let (Some(n), Some(unit)) = (it.next(), it.next()) else { return 0 };
    let Ok(n) = n.replace(',', "").parse::<f64>() else { return 0 };
    let mult = match unit {
        "bytes" => 1.0,
        "kB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return 0,
    };
    (n * mult) as u64
}

/// Exported icon file for an app ID, largest first.
pub fn icon(share_dirs: &[PathBuf], appid: &str) -> Option<String> {
    for base in share_dirs {
        for sub in ["scalable", "512x512", "256x256", "128x128", "64x64", "48x48"] {
            for ext in ["svg", "png"] {
                let f = base.join(format!("icons/hicolor/{sub}/apps/{appid}.{ext}"));
                if f.exists() {
                    return Some(f.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

pub fn is_export(p: &Path) -> bool {
    p.to_string_lossy().contains("/flatpak/exports/share/applications/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list() {
        let t = "org.gimp.GIMP/x86_64/stable\tGIMP\t2.10\tImage editor\tflathub\tsystem\t1.2\u{a0}GB\tsystem,current\n\
                 org.gnome.Platform/x86_64/46\tGNOME\t\tShared\tflathub\tuser\t512.0 MB\tuser,runtime,eol=old, update\n";
        let p = parse(t);
        assert_eq!(p[0].name, "org.gimp.GIMP");
        assert_eq!(p[0].size, 1_200_000_000);
        assert!(p[0].system && p[0].blocked.is_none());
        assert_eq!(p[1].role, Role::Runtime);
        assert!(!p[1].system);
        assert_eq!(p[1].description, "⚠ End of life: old, update");
        assert_eq!(p[0].id, "org.gimp.GIMP/x86_64/stable");
    }

    #[test]
    fn parse_remote_updates() {
        let t = "app/com.discordapp.Discord/x86_64/stable\tcom.discordapp.Discord\t1.0.161\n\
                 runtime/org.gnome.Platform/x86_64/51\torg.gnome.Platform\t\n";
        let u = parse_updates(t, true);
        assert_eq!(u[0].r#ref, "com.discordapp.Discord/x86_64/stable");
        assert!(!u[0].runtime && u[0].system);
        assert!(u[1].runtime);
        assert_eq!(u[1].version, "");
    }
}
