//! Checking for and installing updates from every source at once.

use crate::packagekit;
use crate::system::{Outcome, plural};
use crate::ui::operation::{self, Operation};
use depot_core::backend::{brew, cargo, flatpak, npm, snap};
use depot_core::{Kind, Package, Update, updates};
use gtk::gio;
use serde::Serialize;
use std::path::Path;
use std::rc::Rc;

#[derive(Debug, Default, Serialize)]
pub struct Checked {
    pub updates: Vec<Update>,
    /// Sources that couldn't be checked, in plain language.
    pub warnings: Vec<String>,
}

/// Ask every source what can be updated. With `refresh`, PackageKit downloads fresh package
/// lists first; Snap and Flatpak always ask their stores.
pub async fn check(refresh: bool, inv: &[Package]) -> Checked {
    let snaps = gio::spawn_blocking(|| snap::refreshable(Path::new(snap::SOCKET)));
    let flat_sys = gio::spawn_blocking(|| flatpak::updates(true));
    let flat_user = gio::spawn_blocking(|| flatpak::updates(false));
    let native = async {
        if !packagekit::available().await {
            return Ok(Vec::new());
        }
        if refresh {
            // a failed refresh (offline, mirror down) still leaves the last lists usable
            let _ = packagekit::refresh_cache().await;
        }
        packagekit::get_updates().await
    };
    // developer tools are only asked when something of theirs is installed
    let has = |k: Kind| inv.iter().any(|p| p.kind == k && p.blocked.is_none());
    let npm = has(Kind::Npm).then(|| gio::spawn_blocking(npm::outdated));
    let brew = has(Kind::Brew).then(|| gio::spawn_blocking(brew::outdated));
    let crates: Vec<(String, String)> =
        inv.iter().filter(|p| p.kind == Kind::Cargo).map(|p| (p.id.clone(), p.version.clone())).collect();
    let cargo = (!crates.is_empty())
        .then(|| gio::spawn_blocking(move || cargo::outdated(&cargo::home_dir(&crate::system::home()), &crates)));
    let tools = futures_util::future::join_all(
        [(Kind::Npm, "npm", npm), (Kind::Brew, "Homebrew", brew), (Kind::Cargo, "Cargo", cargo)]
            .into_iter()
            .filter_map(|(k, label, h)| h.map(|h| async move { (k, label, h.await) })),
    );
    let ((native, snaps, flat_sys, flat_user), tools) =
        futures_util::future::join(futures_util::future::join4(native, snaps, flat_sys, flat_user), tools).await;

    let mut out = Checked::default();
    match native {
        Ok(pk) => {
            let pk: Vec<(u32, String)> = pk.into_iter().map(|p| (p.info, p.id)).collect();
            let found = updates::from_packagekit(&pk, inv);
            if found.is_empty() && !pk.is_empty() {
                out.warnings.push(
                    "System packages: Depot couldn't read your installed packages, so their updates can't be shown."
                        .into(),
                );
            }
            out.updates.extend(found);
        }
        Err(e) => out.warnings.push(format!("System packages: {e}")),
    }
    match snaps.unwrap_or_else(|_| Err("crashed".into())) {
        Ok(s) => out.updates.extend(updates::from_snaps(&s, inv)),
        Err(e) => out.warnings.push(format!("Snaps: {e}")),
    }
    for f in [flat_sys, flat_user] {
        match f.unwrap_or_else(|_| Err("crashed".into())) {
            Ok(f) => out.updates.extend(updates::from_flatpaks(&f, inv)),
            Err(e) => out.warnings.push(format!("Flatpaks: {e}")),
        }
    }
    for (kind, label, r) in tools {
        match r.unwrap_or_else(|_| Err("crashed".into())) {
            Ok(found) => out.updates.extend(updates::from_tool(kind, &found, inv)),
            Err(e) => out.warnings.push(format!("{label}: {e}")),
        }
    }
    out.updates.sort_by_cached_key(|u| u.title.to_lowercase());
    out
}

/// Packages that updating these native packages would remove, found with a dry run.
pub async fn removals(ids: &[String]) -> Result<Vec<String>, Outcome> {
    let pk = packagekit::simulate_update(ids).await.map_err(packagekit::PkError::outcome)?;
    Ok(pk
        .iter()
        .filter(|p| matches!(p.info, updates::info::REMOVING | updates::info::OBSOLETING))
        .map(|p| p.name().to_string())
        .collect())
}

/// Updates installed together: one PackageKit transaction, one `snap refresh`, or one
/// `flatpak update` per installation.
pub struct Batch {
    /// What the sheet's checklist calls it.
    pub title: String,
    what: What,
}

enum What {
    Native(Vec<String>),
    Snaps(Vec<String>),
    Flatpaks {
        system: bool,
        refs: Vec<String>,
    },
    /// The whole command, from a developer tool.
    Tool(Vec<String>),
}

/// Split `list` into the batches that install it.
pub fn batches(list: &[Update]) -> Vec<Batch> {
    // every kind is sorted here, so a new kind can't be quietly left out
    let (mut natives, mut snaps, mut flats_system, mut flats_user) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut crates, mut npms, mut brews) = (Vec::new(), Vec::new(), Vec::new());
    for u in list {
        match u.kind {
            Kind::Deb | Kind::Rpm | Kind::Pacman => natives.push(u.id.clone()),
            Kind::Snap => snaps.push(u.id.clone()),
            Kind::Flatpak if u.system => flats_system.push(u.id.clone()),
            Kind::Flatpak => flats_user.push(u.id.clone()),
            Kind::Cargo => crates.push(u.id.clone()),
            Kind::Npm => npms.push(format!("{}@latest", u.id)),
            Kind::Brew => brews.push(u.id.clone()),
            // nothing tells Depot about newer versions of these, so there are no updates to install
            Kind::AppImage | Kind::Manual | Kind::Pipx | Kind::Nix => {}
        }
    }
    let mut out = Vec::new();
    if !natives.is_empty() {
        out.push(Batch { title: format!("System packages ({})", natives.len()), what: What::Native(natives) });
    }
    if !snaps.is_empty() {
        out.push(Batch { title: format!("Snaps ({})", snaps.len()), what: What::Snaps(snaps) });
    }
    for (system, refs) in [(true, flats_system), (false, flats_user)] {
        if !refs.is_empty() {
            let whose = if system { "for everyone" } else { "for you" };
            out.push(Batch {
                title: format!("Flatpaks {whose} ({})", refs.len()),
                what: What::Flatpaks { system, refs },
            });
        }
    }
    // cargo builds each program again from source, one at a time
    let cargo_home = cargo::home_dir(&crate::system::home());
    for name in crates {
        out.push(Batch { title: name.clone(), what: What::Tool(cargo::reinstall_args(&cargo_home, &name)) });
    }
    let tool = |title: &str, base: &[&str], ids: Vec<String>| {
        let mut cmd: Vec<String> = base.iter().map(|a| a.to_string()).collect();
        let n = ids.len();
        cmd.extend(ids);
        Batch { title: format!("{title} ({n})"), what: What::Tool(cmd) }
    };
    if !npms.is_empty() {
        out.push(tool("npm packages", &["npm", "install", "-g", "--"], npms));
    }
    if !brews.is_empty() {
        out.push(tool("Homebrew packages", &["brew", "upgrade", "--"], brews));
    }
    out
}

/// Install one batch, showing its progress in `op`.
pub async fn apply(b: &Batch, op: &Rc<Operation>) -> Outcome {
    match &b.what {
        What::Native(ids) => {
            op.progress("Preparing system updates…", None);
            let cancel = gio::Cancellable::new();
            let report = operation::pk_reporter(op, &cancel, |n| format!("Updating {n}…"));
            match packagekit::update(ids, false, &cancel, report).await {
                Ok(_) => Outcome::Done,
                Err(e) => e.outcome(),
            }
        }
        What::Snaps(names) => {
            let mut cmd = vec!["snap".to_string(), "refresh".into(), "--".into()];
            cmd.extend(names.iter().cloned());
            let what = plural(names.len(), "snap", "snaps");
            operation::command(op, &cmd, true, &format!("Updating {what}…")).await
        }
        What::Flatpaks { system, refs } => {
            // Flatpak asks for the password itself for apps installed for everyone, so no pkexec
            let mut cmd: Vec<String> =
                ["flatpak", "update", "-y", "--noninteractive", if *system { "--system" } else { "--user" }, "--"]
                    .map(String::from)
                    .to_vec();
            cmd.extend(refs.iter().cloned());
            let what = plural(refs.len(), "Flatpak item", "Flatpak items");
            operation::command(op, &cmd, false, &format!("Updating {what}…")).await
        }
        // developer tools install for you alone, so no password
        What::Tool(cmd) => operation::command(op, cmd, false, &format!("Updating {}…", b.title)).await,
    }
}

impl Batch {
    /// The PackageKit ids, for a batch of system packages.
    pub fn native_ids(&self) -> Option<&[String]> {
        if let What::Native(ids) = &self.what { Some(ids) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(kind: Kind, id: &str) -> Update {
        Update {
            kind,
            id: id.into(),
            name: id.into(),
            title: id.into(),
            from: String::new(),
            to: String::new(),
            icon: None,
            role: depot_core::Role::Tool,
            security: false,
            system: false,
        }
    }

    #[test]
    fn tool_updates_get_their_own_batches() {
        let b =
            batches(&[up(Kind::Npm, "typescript"), up(Kind::Npm, "eslint"), up(Kind::Brew, "gh"), up(Kind::Pipx, "x")]);
        let cmds: Vec<String> =
            b.iter().map(|b| if let What::Tool(c) = &b.what { c.join(" ") } else { String::new() }).collect();
        assert_eq!(cmds, ["npm install -g -- typescript@latest eslint@latest", "brew upgrade -- gh"]);
        assert_eq!(b[0].title, "npm packages (2)");
    }
}
