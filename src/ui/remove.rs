//! Uninstall flow: check what would happen, confirm, run, report.

use super::widgets::Progress;
use super::{State, reload};
use crate::packagekit::{self, PkPackage};
use adw::prelude::*;
use depot_core::backend::{dpkg, manual};
use depot_core::remove::{self as removal, DebPlan};
use depot_core::updates::info;
use depot_core::{Kind, Package};
use gtk::{gio, glib};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

struct Outcome {
    code: i32,
    output: String,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn short(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(r) if !h.as_os_str().is_empty() => format!("~/{}", r.display()),
        _ => p.display().to_string(),
    }
}

/// Ask apt what removing this package would take with it, and what it would leave unused.
fn simulate_apt(id: &str) -> DebPlan {
    let sim = |autoremove: bool| {
        let mut c = Command::new("apt-get");
        c.args(["-s", "remove"]);
        if autoremove {
            c.arg("--autoremove");
        }
        c.arg(id).env("LC_ALL", "C.UTF-8").env("LANGUAGE", "C").output()
    };
    let (Ok(without), Ok(with)) = (sim(false), sim(true)) else {
        return DebPlan { blocked: Some("apt-get isn't available.".into()), ..Default::default() };
    };
    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }
    if let Some(b) = removal::apt_blocked(&text(&without.stdout), &text(&without.stderr), without.status.success()) {
        return DebPlan { blocked: Some(b), ..Default::default() };
    }
    removal::plan_from_apt(id, &text(&with.stdout), &text(&without.stdout))
}

/// Dry-run the removal with PackageKit, or apt when PackageKit isn't installed.
async fn plan_deb(p: &Package) -> DebPlan {
    if packagekit::available().await
        && let Ok(found) = packagekit::resolve_installed(std::slice::from_ref(&p.name)).await
    {
        let arch = p.id.split_once(':').map_or(dpkg::native_arch(), |(_, a)| a);
        let target = found.iter().find(|k| {
            let mut f = k.id.split(';');
            f.next() == Some(p.name.as_str()) && matches!(f.nth(1), Some(a) if a == arch || a == "all")
        });
        if let Some(target) = target {
            let ids = vec![target.id.clone()];
            // two dry runs tell apart what must go with it and what is merely left unused
            let with = packagekit::remove(&ids, true, true, |_, _| {}).await;
            let without = packagekit::remove(&ids, true, false, |_, _| {}).await;
            let pairs = |l: Vec<PkPackage>| l.into_iter().map(|k| (k.info, k.id)).collect::<Vec<_>>();
            match (with, without) {
                (Ok(with), Ok(without)) => {
                    return removal::plan_from_packagekit(&target.id, &pairs(with), &pairs(without));
                }
                (_, Err(e))
                    if matches!(
                        e.code,
                        packagekit::ERROR_DEP_RESOLUTION_FAILED | packagekit::ERROR_CANNOT_REMOVE_SYSTEM_PACKAGE
                    ) =>
                {
                    return DebPlan { blocked: Some(removal::SYSTEM_NEEDS_IT.into()), ..Default::default() };
                }
                _ => {}
            }
        }
    }
    let id = p.id.clone();
    gio::spawn_blocking(move || simulate_apt(&id)).await.unwrap_or_default()
}

pub fn start(st: &Rc<State>, p: &Rc<Package>, button: &gtk::Button) {
    button.set_sensitive(false);
    let (st, p, button) = (st.clone(), p.clone(), button.clone());
    glib::spawn_future_local(async move {
        let plan = if p.kind == Kind::Deb { plan_deb(&p).await } else { DebPlan::default() };
        button.set_sensitive(true);
        confirm(&st, &p, plan);
    });
}

fn listing(lines: &[String]) -> gtk::ScrolledWindow {
    let l = gtk::Label::builder().label(lines.join("\n")).xalign(0.0).selectable(true).build();
    l.add_css_class("monospace");
    let sc = gtk::ScrolledWindow::builder()
        .child(&l)
        .min_content_height(40)
        .max_content_height(160)
        .propagate_natural_height(true)
        .build();
    sc.add_css_class("card");
    sc
}

fn confirm(st: &Rc<State>, p: &Rc<Package>, plan: DebPlan) {
    let DebPlan { deps: extra, orphans, orphan_pk_ids, blocked, pk_id } = plan;
    if let Some(reason) = blocked {
        let d = adw::AlertDialog::builder().heading("Can't uninstall").body(reason).build();
        d.add_response("close", "Close");
        d.present(Some(&st.win));
        return;
    }
    let plan = p.manual.clone().unwrap_or_default();
    let full_manual = p.kind == Kind::Manual && plan.appdir.is_some();
    let heading = match (p.kind, full_manual, p.broken) {
        (Kind::Manual, _, true) => format!("Remove the leftover {} launcher?", p.title),
        (Kind::Manual, false, _) => format!("Remove the {} launcher?", p.title),
        _ => format!("Uninstall {}?", p.title),
    };
    let body = match p.kind {
        Kind::Manual if p.broken => {
            "The program it started is already gone. The launcher will be moved to the Trash.".to_string()
        }
        Kind::Manual if full_manual => "These files will be moved to the Trash:".to_string(),
        Kind::Manual => {
            format!("Only the launcher is moved to the Trash. The program itself ({}) is left untouched.", p.location)
        }
        Kind::Snap | Kind::Flatpak => "This removes the app from your computer.".to_string(),
        Kind::Deb if extra.is_empty() => "This removes it from your computer.".to_string(),
        Kind::Deb => format!("This removes it from your computer, along with {} package(s) that need it:", extra.len()),
    };
    let d = adw::AlertDialog::builder().heading(heading).body(body).build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let lines: Vec<String> = if p.kind == Kind::Manual {
        plan.files.iter().map(|f| short(f)).collect()
    } else {
        let mut e = extra.clone();
        e.sort();
        e
    };
    if !lines.is_empty() && (p.kind != Kind::Manual || full_manual) {
        content.append(&listing(&lines));
    }
    let clean = gtk::CheckButton::builder()
        .label(format!(
            "Also remove {} package{} that nothing else needs any more",
            orphans.len(),
            if orphans.len() == 1 { "" } else { "s" }
        ))
        .active(true)
        .tooltip_text("These were installed automatically for other software that is no longer here.")
        .build();
    if !orphans.is_empty() {
        content.append(&clean);
        content.append(&listing(&orphans));
    }
    let purge = gtk::CheckButton::with_label(match p.kind {
        Kind::Snap => "Don't keep a backup snapshot of its data",
        // apt's purge removes the package's files in /etc, not anything in your home folder
        Kind::Deb => "Also delete its system-wide settings files",
        _ => "Also delete settings and data",
    });
    if p.kind == Kind::Manual {
        let data: Vec<String> = plan.data.iter().map(|d| short(d)).collect();
        purge.set_label(Some(&format!("Also move its settings and data to the Trash ({})", data.join(", "))));
    }
    if p.kind != Kind::Manual || !plan.data.is_empty() {
        content.append(&purge);
    }
    d.set_extra_child(Some(&content));
    d.add_response("cancel", "Cancel");
    let (kind, deps_n, orphans_n) = (p.kind, extra.len(), orphans.len());
    let action = move |with_orphans: bool| {
        let n = 1 + deps_n + if with_orphans { orphans_n } else { 0 };
        match kind {
            Kind::Manual => "Move to Trash".to_string(),
            Kind::Deb if n > 1 => format!("Uninstall {n} packages"),
            _ => "Uninstall".to_string(),
        }
    };
    d.add_response("remove", &action(clean.is_active()));
    {
        let d = d.clone();
        clean.connect_toggled(move |c| d.set_response_label("remove", &action(c.is_active())));
    }
    d.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.present(Some(&st.win));
    let (st, p) = (st.clone(), p.clone());
    d.connect_response(None, move |_, r| {
        if r == "remove" {
            // only the leftovers that were listed, and only if the box was shown and ticked
            let (orphans, orphan_pk_ids) =
                if clean.is_active() { (orphans.clone(), orphan_pk_ids.clone()) } else { (Vec::new(), Vec::new()) };
            run(&st, &p, purge.is_active(), orphans, orphan_pk_ids, pk_id.clone());
        }
    });
}

async fn trash_manual(p: &Package, with_data: bool) -> Outcome {
    let plan = p.manual.clone().unwrap_or_default();
    let h = home();
    let shared = manual::app_roots(&h);
    let mut errors = Vec::new();
    for t in plan.files.iter().chain(if with_data { plan.data.iter() } else { [].iter() }) {
        if std::fs::symlink_metadata(t).is_err() {
            continue;
        }
        if !removal::safe_target(t, &h, &shared) {
            errors.push(format!("Refused to touch {}", t.display()));
            continue;
        }
        if let Err(e) = gio::File::for_path(t).trash_future(glib::Priority::DEFAULT).await {
            errors.push(format!("{}: {}", short(t), e.message()));
        }
    }
    Outcome { code: if errors.is_empty() { 0 } else { 1 }, output: errors.join("\n") }
}

fn privileged(cmd: Vec<String>, root: bool) -> Outcome {
    let mut c = if root {
        let mut c = Command::new("pkexec");
        c.args(["env", "DEBIAN_FRONTEND=noninteractive", "LC_ALL=C.UTF-8"]).args(&cmd);
        c
    } else {
        let mut c = Command::new(&cmd[0]);
        c.args(&cmd[1..]);
        c
    };
    match c.output() {
        Ok(o) => Outcome {
            code: o.status.code().unwrap_or(1),
            output: format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
        },
        Err(e) => Outcome { code: 1, output: e.to_string() },
    }
}

/// Remove exactly these packages (and what depends on them, which the user was shown).
async fn remove_with_packagekit(ids: Vec<String>, progress: Rc<Progress>) -> Outcome {
    let r = packagekit::remove(&ids, false, false, move |pct, pkg: Option<&PkPackage>| {
        let frac = (pct <= 100).then(|| pct as f64 / 100.0);
        match pkg {
            Some(k) if k.info == info::REMOVING => progress.set(&format!("Removing {}…", k.name()), frac),
            _ => progress.set("", frac),
        }
    })
    .await;
    match r {
        Ok(_) => Outcome { code: 0, output: String::new() },
        Err(e) if e.cancelled => Outcome { code: 126, output: e.details },
        Err(e) => Outcome { code: 1, output: e.details },
    }
}

/// Remove `p`, plus exactly the `orphans` the user agreed to (apt names, with matching PackageKit ids).
fn run(
    st: &Rc<State>,
    p: &Rc<Package>,
    purge: bool,
    orphans: Vec<String>,
    orphan_pk_ids: Vec<String>,
    pk_id: Option<String>,
) {
    let (cmd, root): (Vec<String>, bool) = match p.kind {
        Kind::Manual => (Vec::new(), false),
        Kind::Flatpak => {
            let mut c =
                vec!["flatpak", "uninstall", "-y", "--noninteractive", if p.system { "--system" } else { "--user" }];
            if purge {
                c.push("--delete-data");
            }
            (c.into_iter().map(String::from).chain([p.id.clone()]).collect(), p.system)
        }
        Kind::Snap => {
            let mut c = vec!["snap".to_string(), "remove".into()];
            if purge {
                c.push("--purge".into());
            }
            c.push(p.id.clone());
            (c, true)
        }
        Kind::Deb => (removal::apt_command(&p.id, purge, &orphans), true),
    };
    // PackageKit can't purge config files, so purging always goes through apt
    let pk_id = pk_id.filter(|_| !purge);
    let verb = if p.kind == Kind::Manual { "Moving" } else { "Removing" };
    let hint = if root || pk_id.is_some() { "Approve the password prompt if it appears." } else { "" };
    let opened = std::time::Instant::now();
    let progress = Progress::show(&st.win, &format!("{verb} {}", p.title), hint);
    let (st, p) = (st.clone(), p.clone());
    glib::spawn_future_local(async move {
        let r = if p.kind == Kind::Manual {
            trash_manual(&p, purge).await
        } else if let Some(id) = pk_id {
            let ids = std::iter::once(id).chain(orphan_pk_ids).collect();
            remove_with_packagekit(ids, progress.clone()).await
        } else {
            gio::spawn_blocking(move || privileged(cmd, root))
                .await
                .unwrap_or(Outcome { code: 1, output: "Crashed".into() })
        };
        progress.close(opened).await;
        finish(&st, &p, r);
    });
}

fn finish(st: &Rc<State>, p: &Package, r: Outcome) {
    match r.code {
        0 => {
            let what = if p.kind == Kind::Manual { "moved to the Trash" } else { "uninstalled" };
            st.toast(&format!("{} was {what}", p.title));
            st.nav.pop_to_page(&st.main_page);
            reload(st);
        }
        126 | 127 => st.toast("Cancelled: authentication was not granted"),
        code => {
            let tail: Vec<&str> = r.output.trim().lines().rev().take(8).collect();
            let body = if tail.is_empty() {
                format!("Exit code {code}")
            } else {
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            };
            let d = adw::AlertDialog::builder().heading("Uninstall failed").body(body).build();
            d.add_response("close", "Close");
            d.present(Some(&st.win));
        }
    }
}
