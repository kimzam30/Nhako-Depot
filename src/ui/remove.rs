//! Uninstall flow: check what would happen, confirm, run, report.

use super::widgets::Progress;
use super::{State, reload};
use crate::packagekit::{self, PkPackage};
use crate::system::{self, Outcome, home, short};
use adw::prelude::*;
use depot_core::backend::manual;
use depot_core::remove::{self as removal, RemovePlan};
use depot_core::updates::info;
use depot_core::{Kind, Package};
use gtk::{gio, glib};
use std::process::Command;
use std::rc::Rc;

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn blocked(reason: impl Into<String>) -> RemovePlan {
    RemovePlan { blocked: Some(reason.into()), ..Default::default() }
}

/// Run a dry run twice, without and with "also remove what's left unused", and build a plan
/// from the two outputs. `plan` gets (with, without).
fn dry_runs(
    prog: &str,
    without: &[&str],
    with: &[&str],
    id: &str,
    plan: fn(&str, &str, &str) -> RemovePlan,
) -> RemovePlan {
    let sim =
        |args: &[&str]| Command::new(prog).args(args).arg(id).env("LC_ALL", "C.UTF-8").env("LANGUAGE", "C").output();
    let (Ok(a), Ok(b)) = (sim(without), sim(with)) else { return blocked(format!("{prog} isn't available.")) };
    if let Some(r) = removal::refusal(&text(&a.stdout), &text(&a.stderr), a.status.success()) {
        return blocked(r);
    }
    plan(id, &text(&b.stdout), &text(&a.stdout))
}

/// Ask the package manager itself, for systems without PackageKit.
fn simulate(kind: Kind, id: &str) -> RemovePlan {
    match kind {
        Kind::Deb => {
            dry_runs("apt-get", &["-s", "remove"], &["-s", "remove", "--autoremove"], id, removal::plan_from_apt)
        }
        Kind::Pacman => dry_runs(
            "pacman",
            &["-Rcp", "--print-format", "%n"],
            &["-Rcsp", "--print-format", "%n"],
            id,
            removal::plan_from_pacman,
        ),
        _ => blocked("Removing system packages here needs PackageKit, which isn't installed."),
    }
}

/// Architectures that fit any machine.
fn neutral(arch: &str) -> bool {
    matches!(arch, "" | "all" | "noarch" | "any")
}

/// Dry-run the removal with PackageKit, or the package manager itself when PackageKit isn't installed.
async fn plan_native(p: &Package) -> RemovePlan {
    if packagekit::available().await
        && let Ok(found) = packagekit::resolve_installed(std::slice::from_ref(&p.name)).await
    {
        let target = found.iter().find(|k| {
            let mut f = k.id.split(';');
            f.next() == Some(p.name.as_str())
                && matches!(f.nth(1), Some(a) if a == p.arch || neutral(a) || neutral(&p.arch))
        });
        if let Some(target) = target {
            let ids = vec![target.id.clone()];
            // two dry runs tell apart what must go with it and what is merely left unused
            let with = packagekit::remove(&ids, true, true, |_, _| {}).await;
            let without = packagekit::remove(&ids, true, false, |_, _| {}).await;
            let pairs = |l: Vec<PkPackage>| l.into_iter().map(|k| (k.info, k.id)).collect::<Vec<_>>();
            match (with, without) {
                (Ok(with), Ok(without)) => {
                    return removal::plan_from_packagekit(p.kind, &target.id, &pairs(with), &pairs(without));
                }
                (_, Err(e))
                    if matches!(
                        e.code,
                        packagekit::ERROR_DEP_RESOLUTION_FAILED | packagekit::ERROR_CANNOT_REMOVE_SYSTEM_PACKAGE
                    ) =>
                {
                    return blocked(removal::SYSTEM_NEEDS_IT);
                }
                _ => {}
            }
        }
    }
    let (kind, id) = (p.kind, p.id.clone());
    gio::spawn_blocking(move || simulate(kind, &id)).await.unwrap_or_default()
}

pub fn start(st: &Rc<State>, p: &Rc<Package>, button: &gtk::Button) {
    button.set_sensitive(false);
    let (st, p, button) = (st.clone(), p.clone(), button.clone());
    glib::spawn_future_local(async move {
        let plan = if p.kind.is_native() { plan_native(&p).await } else { RemovePlan::default() };
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

fn confirm(st: &Rc<State>, p: &Rc<Package>, plan: RemovePlan) {
    let RemovePlan { deps: extra, orphans, orphan_pk_ids, blocked, pk_id } = plan;
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
        _ if extra.is_empty() => "This removes it from your computer.".to_string(),
        _ => format!("This removes it from your computer, along with {} package(s) that need it:", extra.len()),
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
        Kind::Pacman => "Don't keep backups of system-wide settings files you changed",
        _ => "Also delete settings and data",
    });
    if p.kind == Kind::Manual {
        let data: Vec<String> = plan.data.iter().map(|d| short(d)).collect();
        purge.set_label(Some(&format!("Also move its settings and data to the Trash ({})", data.join(", "))));
    }
    // dnf has no way to delete settings files along with a package
    let offers_purge = match p.kind {
        Kind::Manual => !plan.data.is_empty(),
        Kind::Rpm => false,
        _ => true,
    };
    if offers_purge {
        content.append(&purge);
    }
    d.set_extra_child(Some(&content));
    d.add_response("cancel", "Cancel");
    let (kind, deps_n, orphans_n) = (p.kind, extra.len(), orphans.len());
    let action = move |with_orphans: bool| {
        let n = 1 + deps_n + if with_orphans { orphans_n } else { 0 };
        match kind {
            Kind::Manual => "Move to Trash".to_string(),
            k if k.is_native() && n > 1 => format!("Uninstall {n} packages"),
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
            let purge = offers_purge && purge.is_active();
            run(&st, &p, purge, &extra, orphans, orphan_pk_ids, pk_id.clone());
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
    if errors.is_empty() { Outcome::ok() } else { Outcome::failed(errors.join("\n")) }
}

/// Remove exactly these packages (and what depends on them, which the user was shown).
async fn remove_with_packagekit(ids: Vec<String>, progress: Rc<Progress>) -> Outcome {
    let r = packagekit::remove(&ids, false, false, move |pct, pkg: Option<&PkPackage>| {
        let frac = system::fraction(pct);
        match pkg {
            Some(k) if k.info == info::REMOVING => progress.set(&format!("Removing {}…", k.name()), frac),
            _ => progress.set("", frac),
        }
    })
    .await;
    match r {
        Ok(_) => Outcome::ok(),
        Err(e) if e.cancelled => Outcome { code: 126, output: e.details },
        Err(e) => Outcome::failed(e.details),
    }
}

/// Remove `p` and the `deps` that need it, plus exactly the `orphans` the user agreed to
/// (package-manager ids, with matching PackageKit ids).
fn run(
    st: &Rc<State>,
    p: &Rc<Package>,
    purge: bool,
    deps: &[String],
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
        Kind::Pacman => (removal::pacman_command(&p.id, purge, deps, &orphans), true),
        // only ever removed through PackageKit (see `simulate`)
        Kind::Rpm => (Vec::new(), true),
    };
    // PackageKit can't purge settings files, so purging goes through apt or pacman
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
            gio::spawn_blocking(move || system::run(&cmd, root))
                .await
                .unwrap_or_else(|_| Outcome::failed("The uninstaller crashed."))
        };
        progress.close(opened).await;
        finish(&st, &p, r);
    });
}

fn finish(st: &Rc<State>, p: &Package, r: Outcome) {
    if r.code == 0 {
        let what = if p.kind == Kind::Manual { "moved to the Trash" } else { "uninstalled" };
        st.toast(&format!("{} was {what}", p.title));
        st.nav.pop_to_page(&st.main_page);
        reload(st);
    } else if r.cancelled() {
        st.toast("Cancelled: authentication was not granted");
    } else {
        let tail = r.tail(8);
        let body = if tail.is_empty() { format!("Exit code {}", r.code) } else { tail };
        let d = adw::AlertDialog::builder().heading("Uninstall failed").body(body).build();
        d.add_response("close", "Close");
        d.present(Some(&st.win));
    }
}
