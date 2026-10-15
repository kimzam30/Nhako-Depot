//! Available updates, matched to what's installed so they can be shown with a name, icon and version.

use crate::backend::dpkg;
use crate::backend::flatpak::FlatpakUpdate;
use crate::model::{Kind, Package, Role};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize)]
pub struct Update {
    pub kind: Kind,
    /// What the updater needs: PackageKit package id, snap name or Flatpak ref.
    pub id: String,
    pub name: String,
    pub title: String,
    pub from: String,
    pub to: String,
    pub icon: Option<String>,
    /// `App` updates are listed one by one; everything else is grouped as a system update.
    pub role: Role,
    pub security: bool,
    /// Flatpak: system-wide installation.
    pub system: bool,
}

/// PackageKit's `Info` values that mean "an update of this kind is available".
pub mod info {
    pub const LOW: u32 = 3;
    pub const ENHANCEMENT: u32 = 4;
    pub const NORMAL: u32 = 5;
    pub const BUGFIX: u32 = 6;
    pub const IMPORTANT: u32 = 7;
    pub const SECURITY: u32 = 8;
    pub const BLOCKED: u32 = 9;
    pub const REMOVING: u32 = 13;
    pub const OBSOLETING: u32 = 15;
    pub const CRITICAL: u32 = 26;
}

/// Split a PackageKit package id, `name;version;arch;data`.
pub fn pk_split(id: &str) -> Option<(&str, &str, &str, &str)> {
    let mut it = id.splitn(4, ';');
    Some((it.next()?, it.next()?, it.next()?, it.next().unwrap_or("")))
}

/// Installed packages of one kind by the key updates refer to them with (snap name, otherwise id).
fn index(inv: &[Package], kind: Kind) -> HashMap<&str, &Package> {
    let mut m = HashMap::new();
    for p in inv.iter().filter(|p| p.kind == kind) {
        m.entry(if kind == Kind::Snap { p.name.as_str() } else { p.id.as_str() }).or_insert(p);
    }
    m
}

fn from_installed(kind: Kind, id: String, name: &str, to: String, p: Option<&Package>) -> Update {
    Update {
        kind,
        id,
        name: name.to_string(),
        title: p.map_or_else(|| name.to_string(), |p| p.title.clone()),
        from: p.map(|p| p.version.clone()).unwrap_or_default(),
        to,
        icon: p.and_then(|p| p.icon.clone()),
        role: p.map_or(Role::Tool, |p| p.role),
        security: false,
        system: p.is_some_and(|p| p.system),
    }
}

/// Native package updates from PackageKit's `GetUpdates`, as `(info, package id)`.
/// Held-back (`BLOCKED`) updates are skipped since they can't be installed.
pub fn from_packagekit(pk: &[(u32, String)], inv: &[Package]) -> Vec<Update> {
    let debs = index(inv, Kind::Deb);
    let native = dpkg::native_arch();
    pk.iter()
        .filter(|(i, _)| *i != info::BLOCKED)
        .filter_map(|(i, id)| {
            let (name, ver, arch, _) = pk_split(id)?;
            let key = if arch.is_empty() || arch == "all" || arch == native {
                name.to_string()
            } else {
                format!("{name}:{arch}")
            };
            let mut u = from_installed(Kind::Deb, id.clone(), name, ver.to_string(), debs.get(key.as_str()).copied());
            u.security = matches!(*i, info::SECURITY | info::CRITICAL);
            Some(u)
        })
        .collect()
}

pub fn from_snaps(refresh: &[(String, String)], inv: &[Package]) -> Vec<Update> {
    let snaps = index(inv, Kind::Snap);
    refresh
        .iter()
        .map(|(name, to)| from_installed(Kind::Snap, name.clone(), name, to.clone(), snaps.get(name.as_str()).copied()))
        .collect()
}

pub fn from_flatpaks(ups: &[FlatpakUpdate], inv: &[Package]) -> Vec<Update> {
    let flats = index(inv, Kind::Flatpak);
    ups.iter()
        .map(|f| {
            let p = flats.get(f.r#ref.as_str()).copied();
            let mut u = from_installed(Kind::Flatpak, f.r#ref.clone(), &f.appid, f.version.clone(), p);
            if f.runtime {
                u.role = Role::Runtime;
            }
            u.system = f.system;
            u
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deb(id: &str, title: &str, ver: &str, role: Role) -> Package {
        let mut p = Package::new(Kind::Deb, id);
        p.title = title.into();
        p.version = ver.into();
        p.role = role;
        p
    }

    #[test]
    fn packagekit_updates_match_installed() {
        let inv = vec![deb("firefox", "Firefox", "1.0", Role::App), deb("libc6", "libc6", "2.39", Role::Tool)];
        let arch = dpkg::native_arch();
        let pk = vec![
            (info::NORMAL, format!("firefox;2.0;{arch};noble-updates")),
            (info::SECURITY, format!("libc6;2.40;{arch};noble-security")),
            (info::BLOCKED, format!("held;1;{arch};x")),
        ];
        let u = from_packagekit(&pk, &inv);
        assert_eq!(u.len(), 2);
        assert_eq!((u[0].title.as_str(), u[0].from.as_str(), u[0].to.as_str()), ("Firefox", "1.0", "2.0"));
        assert_eq!(u[0].role, Role::App);
        assert!(u[1].security && u[1].role == Role::Tool);
    }

    #[test]
    fn flatpak_runtime_updates_are_system_updates() {
        let mut app = Package::new(Kind::Flatpak, "com.discordapp.Discord/x86_64/stable");
        app.title = "Discord".into();
        app.role = Role::App;
        let ups = vec![
            FlatpakUpdate {
                r#ref: app.id.clone(),
                appid: "com.discordapp.Discord".into(),
                version: "2".into(),
                runtime: false,
                system: true,
            },
            FlatpakUpdate {
                r#ref: "org.gnome.Platform/x86_64/51".into(),
                appid: "org.gnome.Platform".into(),
                version: String::new(),
                runtime: true,
                system: false,
            },
        ];
        let u = from_flatpaks(&ups, &[app]);
        assert_eq!(u[0].title, "Discord");
        assert_eq!(u[0].role, Role::App);
        assert_eq!(u[1].role, Role::Runtime);
    }

    #[test]
    fn split_ids() {
        assert_eq!(pk_split("a;1;amd64;installed"), Some(("a", "1", "amd64", "installed")));
        assert_eq!(pk_split("a"), None);
    }
}
