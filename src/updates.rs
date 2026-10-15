//! Checking for and installing updates from every source at once.

use crate::packagekit::{self, PkPackage};
use depot_core::backend::{flatpak, snap};
use depot_core::{Kind, Package, Update, updates};
use gtk::gio;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

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

/// How an update run ended.
pub enum Done {
    Ok,
    Cancelled,
    Failed(String),
}

/// Packages that updating these native packages would remove, found with a dry run.
pub async fn removals(ids: &[String]) -> Vec<String> {
    match packagekit::update(ids, true, |_, _| {}).await {
        Ok(pk) => pk
            .iter()
            .filter(|p| matches!(p.info, updates::info::REMOVING | updates::info::OBSOLETING))
            .map(|p| p.name().to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn cli(cmd: Vec<String>, root: bool) -> Result<(), (i32, String)> {
    let mut c = if root { Command::new("pkexec") } else { Command::new(&cmd[0]) };
    if root {
        c.args(["env", "DEBIAN_FRONTEND=noninteractive", "LC_ALL=C.UTF-8"]).args(&cmd);
    } else {
        c.args(&cmd[1..]);
    }
    match c.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let tail: Vec<&str> = text.trim().lines().rev().take(6).collect();
            Err((o.status.code().unwrap_or(1), tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
        }
        Err(e) => Err((1, e.to_string())),
    }
}

async fn blocking(cmd: Vec<String>, root: bool) -> Result<(), Done> {
    match gio::spawn_blocking(move || cli(cmd, root)).await {
        Ok(Ok(())) => Ok(()),
        // pkexec: 126 = dismissed, 127 = not authorised
        Ok(Err((126 | 127, _))) => Err(Done::Cancelled),
        Ok(Err((_, msg))) => Err(Done::Failed(msg)),
        Err(_) => Err(Done::Failed("The updater crashed.".into())),
    }
}

/// Install the given updates. `status` gets a line of text and a fraction (`None` = unknown).
pub async fn apply(list: &[Update], status: impl Fn(&str, Option<f64>) + Clone + 'static) -> Done {
    let debs: Vec<&Update> = list.iter().filter(|u| u.kind == Kind::Deb).collect();
    let snaps: Vec<String> = list.iter().filter(|u| u.kind == Kind::Snap).map(|u| u.id.clone()).collect();
    let flats: Vec<&Update> = list.iter().filter(|u| u.kind == Kind::Flatpak).collect();

    if !debs.is_empty() {
        if packagekit::available().await {
            let ids: Vec<String> = debs.iter().map(|u| u.id.clone()).collect();
            let s = status.clone();
            s("Preparing system updates…", None);
            let r = packagekit::update(&ids, false, move |pct, pkg: Option<&PkPackage>| {
                let frac = (pct <= 100).then(|| pct as f64 / 100.0);
                match pkg {
                    Some(p) => s(&format!("Updating {}…", p.name()), frac),
                    None => s("", frac),
                }
            })
            .await;
            match r {
                Ok(_) => {}
                Err(e) if e.cancelled => return Done::Cancelled,
                Err(e) => return Done::Failed(e.details),
            }
        } else {
            status("Updating system packages…", None);
            let mut cmd = vec!["apt-get".to_string(), "install".into(), "--only-upgrade".into(), "-y".into()];
            cmd.extend(debs.iter().map(|u| u.name.clone()));
            if let Err(d) = blocking(cmd, true).await {
                return d;
            }
        }
    }
    if !snaps.is_empty() {
        status(&format!("Updating {}…", snaps.join(", ")), None);
        let mut cmd = vec!["snap".to_string(), "refresh".into()];
        cmd.extend(snaps);
        if let Err(d) = blocking(cmd, true).await {
            return d;
        }
    }
    for system in [true, false] {
        let refs: Vec<String> = flats.iter().filter(|u| u.system == system).map(|u| u.id.clone()).collect();
        if refs.is_empty() {
            continue;
        }
        status(&format!("Updating {} Flatpak item(s)…", refs.len()), None);
        // Flatpak asks polkit itself for system-wide installs, so no pkexec here
        let mut cmd: Vec<String> =
            ["flatpak", "update", "-y", "--noninteractive", if system { "--system" } else { "--user" }]
                .iter()
                .map(|s| s.to_string())
                .collect();
        cmd.extend(refs);
        if let Err(d) = blocking(cmd, false).await {
            return d;
        }
    }
    Done::Ok
}
