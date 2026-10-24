//! Checking for and installing updates from every source at once.

use crate::packagekit;
use crate::system::Outcome;
use crate::ui::operation::{self, Operation};
use depot_core::backend::{flatpak, snap};
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
    let (native, snaps, flat_sys, flat_user) = futures_util::future::join4(native, snaps, flat_sys, flat_user).await;

    let mut out = Checked::default();
    match native {
        Ok(pk) => {
            let pk: Vec<(u32, String)> = pk.into_iter().map(|p| (p.info, p.id)).collect();
            out.updates.extend(updates::from_packagekit(&pk, inv));
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
    Flatpaks { system: bool, refs: Vec<String> },
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Split `list` into the batches that install it.
pub fn batches(list: &[Update]) -> Vec<Batch> {
    let mut out = Vec::new();
    let natives: Vec<String> = list.iter().filter(|u| u.kind.is_native()).map(|u| u.id.clone()).collect();
    if !natives.is_empty() {
        out.push(Batch { title: format!("System packages ({})", natives.len()), what: What::Native(natives) });
    }
    let snaps: Vec<String> = list.iter().filter(|u| u.kind == Kind::Snap).map(|u| u.id.clone()).collect();
    if !snaps.is_empty() {
        out.push(Batch { title: format!("Snaps ({})", snaps.len()), what: What::Snaps(snaps) });
    }
    for system in [true, false] {
        let refs: Vec<String> =
            list.iter().filter(|u| u.kind == Kind::Flatpak && u.system == system).map(|u| u.id.clone()).collect();
        if !refs.is_empty() {
            let whose = if system { "for everyone" } else { "for you" };
            out.push(Batch {
                title: format!("Flatpaks {whose} ({})", refs.len()),
                what: What::Flatpaks { system, refs },
            });
        }
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
            let r = operation::command(op, &cmd, false, &format!("Updating {what}…")).await;
            op.set_hint("");
            r
        }
    }
}

impl Batch {
    /// The PackageKit ids, for a batch of system packages.
    pub fn native_ids(&self) -> Option<&[String]> {
        if let What::Native(ids) = &self.what { Some(ids) } else { None }
    }
}

/// A hint shown while a batch runs that may ask for a password by itself.
pub fn may_ask(b: &Batch) -> bool {
    matches!(b.what, What::Flatpaks { system: true, .. })
}
