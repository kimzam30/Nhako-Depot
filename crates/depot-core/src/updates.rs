//! Available updates, matched to what's installed so they can be shown with a name, icon and version.

use crate::backend;
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

/// The PackageKit `Info` values Depot looks at.
pub mod info {
    pub const NORMAL: u32 = 5;
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
/// Held-back (`BLOCKED`) updates are skipped since they can't be installed. With no system
/// packages read there's nothing to match them to (nor to tell deb from rpm), so there are none.
pub fn from_packagekit(pk: &[(u32, String)], inv: &[Package]) -> Vec<Update> {
    let Some(kind) = backend::native_kind(inv) else { return Vec::new() };
    let natives = index(inv, kind);
    pk.iter()
        .filter(|(i, _)| *i != info::BLOCKED)
        .filter_map(|(i, id)| {
            let (name, ver, arch, _) = pk_split(id)?;
            let key = backend::native_id(kind, name, arch);
            let mut u = from_installed(kind, id.clone(), name, ver.to_string(), natives.get(key.as_str()).copied());
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
    // a ref can be installed both system-wide and per user; match the installation being updated
    let mut flats: HashMap<(&str, bool), &Package> = HashMap::new();
    for p in inv.iter().filter(|p| p.kind == Kind::Flatpak) {
        flats.entry((p.id.as_str(), p.system)).or_insert(p);
    }
    ups.iter()
        .map(|f| {
            let p = flats.get(&(f.r#ref.as_str(), f.system)).copied();
            let mut u = from_installed(Kind::Flatpak, f.r#ref.clone(), &f.appid, f.version.clone(), p);
            if f.runtime {
                u.role = Role::Runtime;
            }
            u.system = f.system;
            u
        })
        .collect()
}

/// Updates a developer tool reported as `(name, newest version)`. Packages Depot can't change
/// (npm packages installed for everyone) get none, since installing them needs an administrator.
pub fn from_tool(kind: Kind, found: &[(String, String)], inv: &[Package]) -> Vec<Update> {
    let installed = index(inv, kind);
    found
        .iter()
        .filter_map(|(name, to)| {
            let p = installed.get(name.as_str()).copied().filter(|p| p.blocked.is_none())?;
            Some(from_installed(kind, name.clone(), &p.name, to.clone(), Some(p)))
        })
        .collect()
}

/// Whether version `a` is newer than `b`, comparing the dotted numbers (`1.10.0` is newer than
/// `1.9.2`). A pre-release (`2.0.0-rc.1`) is older than the release itself.
pub fn newer(a: &str, b: &str) -> bool {
    let split = |v: &str| {
        let (core, pre) = v.split_once(['-', '+']).map_or((v, None), |(c, p)| (c, Some(p.to_string())));
        let nums: Vec<u64> = core.split('.').map(|n| n.trim().parse().unwrap_or(0)).collect();
        (nums, pre)
    };
    let ((mut x, xp), (mut y, yp)) = (split(a), split(b));
    let len = x.len().max(y.len());
    x.resize(len, 0);
    y.resize(len, 0);
    match x.cmp(&y) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => xp.is_none() && yp.is_some(),
    }
}

/// Drop updates for software that's no longer installed, so the list stays right after an
/// uninstall without asking every store again.
pub fn retain_installed(ups: &mut Vec<Update>, inv: &[Package]) {
    // a Flatpak can be installed for everyone and for you; each copy has its own update
    ups.retain(|u| {
        inv.iter().any(|p| p.kind == u.kind && p.name == u.name && (u.kind != Kind::Flatpak || p.system == u.system))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::dpkg;

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
        let mut user_copy = app.clone();
        user_copy.title = "Discord (only you)".into();
        app.system = true;
        let u = from_flatpaks(&ups, &[user_copy, app]);
        assert_eq!(u[0].title, "Discord", "matched to the system-wide copy being updated");
        assert_eq!(u[0].role, Role::App);
        assert_eq!(u[1].role, Role::Runtime);
    }

    #[test]
    fn updates_of_removed_software_are_dropped() {
        let inv = vec![deb("firefox", "Firefox", "1.0", Role::App)];
        let arch = dpkg::native_arch();
        let pk = vec![(info::NORMAL, format!("firefox;2.0;{arch};x")), (info::NORMAL, format!("gone;2;{arch};x"))];
        let mut u = from_packagekit(&pk, &inv);
        retain_installed(&mut u, &inv);
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].name, "firefox");
    }

    #[test]
    fn no_system_packages_read_means_no_guessing() {
        let arch = dpkg::native_arch();
        let pk = vec![(info::NORMAL, format!("firefox;2.0;{arch};x"))];
        assert!(from_packagekit(&pk, &[]).is_empty(), "never labelled as debs by default");
    }

    #[test]
    fn version_order() {
        assert!(newer("1.10.0", "1.9.2"));
        assert!(newer("2.0.0", "2.0.0-rc.1"));
        assert!(!newer("2.0.0-rc.1", "2.0.0"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(!newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn tool_updates_skip_what_needs_an_administrator() {
        let mut ts = Package::new(Kind::Npm, "typescript");
        ts.version = "5.6.2".into();
        let mut eslint = Package::new(Kind::Npm, "eslint");
        eslint.blocked = Some("for everyone".into());
        let found = vec![("typescript".to_string(), "5.7.1".to_string()), ("eslint".to_string(), "9.1".to_string())];
        let u = from_tool(Kind::Npm, &found, &[ts, eslint]);
        assert_eq!(u.len(), 1);
        assert_eq!((u[0].from.as_str(), u[0].to.as_str()), ("5.6.2", "5.7.1"));
    }

    #[test]
    fn split_ids() {
        assert_eq!(pk_split("a;1;amd64;installed"), Some(("a", "1", "amd64", "installed")));
        assert_eq!(pk_split("a"), None);
    }
}
