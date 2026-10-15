//! Uninstall flow: check what would happen, confirm, run, report.

use super::{State, reload};
use adw::prelude::*;
use depot_core::backend::manual;
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

/// Ask apt what removing this package would also take with it.
fn simulate_deb(id: &str) -> (Vec<String>, Option<String>) {
    let r = Command::new("apt-get").args(["-s", "remove", id]).env("LC_ALL", "C.UTF-8").env("LANGUAGE", "C").output();
    let Ok(r) = r else { return (Vec::new(), Some("apt-get isn't available.".into())) };
    let out = String::from_utf8_lossy(&r.stdout);
    let base = id.split(':').next().unwrap_or(id);
    let extra = out
        .lines()
        .filter_map(|l| l.strip_prefix("Remv ")?.split_whitespace().next())
        .filter(|m| m.split(':').next() != Some(base))
        .map(str::to_string)
        .collect();
    let blocked = if out.contains("essential packages will be removed") {
        Some("Removing this would also remove essential system packages.".into())
    } else if !r.status.success() {
        let err = String::from_utf8_lossy(&r.stderr);
        let text = if err.trim().is_empty() { out.to_string() } else { err.to_string() };
        Some(text.trim().lines().last().unwrap_or("apt refused.").to_string())
    } else {
        None
    };
    (extra, blocked)
}

pub fn start(st: &Rc<State>, p: &Rc<Package>, button: &gtk::Button) {
    button.set_sensitive(false);
    let (st, p, button) = (st.clone(), p.clone(), button.clone());
    glib::spawn_future_local(async move {
        let (extra, blocked) = if p.kind == Kind::Deb {
            let id = p.id.clone();
            gio::spawn_blocking(move || simulate_deb(&id)).await.unwrap_or_default()
        } else {
            (Vec::new(), None)
        };
        button.set_sensitive(true);
        confirm(&st, &p, extra, blocked);
    });
}

fn confirm(st: &Rc<State>, p: &Rc<Package>, extra: Vec<String>, blocked: Option<String>) {
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
        Kind::Deb if extra.is_empty() => "The package will be removed with apt.".to_string(),
        Kind::Deb => format!(
            "The package will be removed with apt.\n\nThis will also remove {} package(s) that depend on it:",
            extra.len()
        ),
    };
    let d = adw::AlertDialog::builder().heading(heading).body(body).build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let listing: Vec<String> = if p.kind == Kind::Manual {
        plan.files.iter().map(|f| short(f)).collect()
    } else {
        let mut e = extra.clone();
        e.sort();
        e
    };
    if !listing.is_empty() && (p.kind != Kind::Manual || full_manual) {
        let l = gtk::Label::builder().label(listing.join("\n")).xalign(0.0).selectable(true).build();
        l.add_css_class("monospace");
        let sc = gtk::ScrolledWindow::builder()
            .child(&l)
            .min_content_height(60)
            .max_content_height(180)
            .propagate_natural_height(true)
            .build();
        sc.add_css_class("card");
        content.append(&sc);
    }
    let purge = gtk::CheckButton::with_label(match p.kind {
        Kind::Snap => "Don't keep a backup snapshot of its data",
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
    let action = if !extra.is_empty() && p.kind == Kind::Deb {
        format!("Uninstall {} packages", 1 + extra.len())
    } else if p.kind == Kind::Manual {
        "Move to Trash".into()
    } else {
        "Uninstall".into()
    };
    d.add_response("remove", &action);
    d.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.present(Some(&st.win));
    let (st, p) = (st.clone(), p.clone());
    d.connect_response(None, move |_, r| {
        if r == "remove" {
            run(&st, &p, purge.is_active());
        }
    });
}

/// A path we are willing to trash: inside home, not home itself, not a shared app folder.
fn safe_target(t: &Path, home: &Path) -> bool {
    let roots = manual::app_roots(home);
    t.is_absolute()
        && t.starts_with(home)
        && t != home
        && !roots.iter().any(|r| r == t)
        && !t.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

async fn trash_manual(p: &Package, with_data: bool) -> Outcome {
    let plan = p.manual.clone().unwrap_or_default();
    let h = home();
    let mut errors = Vec::new();
    for t in plan.files.iter().chain(if with_data { plan.data.iter() } else { [].iter() }) {
        if std::fs::symlink_metadata(t).is_err() {
            continue;
        }
        if !safe_target(t, &h) {
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

fn run(st: &Rc<State>, p: &Rc<Package>, purge: bool) {
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
        Kind::Deb => {
            (vec!["apt-get".into(), if purge { "purge" } else { "remove" }.into(), "-y".into(), p.id.clone()], true)
        }
    };
    let busy = busy_dialog(st, p, root);
    let (st, p) = (st.clone(), p.clone());
    glib::spawn_future_local(async move {
        let started = std::time::Instant::now();
        let r = if p.kind == Kind::Manual {
            trash_manual(&p, purge).await
        } else {
            gio::spawn_blocking(move || privileged(cmd, root))
                .await
                .unwrap_or(Outcome { code: 1, output: "Crashed".into() })
        };
        // a dialog closed while it is still opening stays open; give it a moment
        if let Some(wait) = std::time::Duration::from_millis(400).checked_sub(started.elapsed()) {
            glib::timeout_future(wait).await;
        }
        busy.force_close();
        finish(&st, &p, r);
    });
}

fn busy_dialog(st: &State, p: &Package, root: bool) -> adw::Dialog {
    let busy = adw::Dialog::builder().content_width(300).can_close(false).build();
    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(14)
        .margin_top(28)
        .margin_bottom(28)
        .margin_start(24)
        .margin_end(24)
        .build();
    bx.append(&gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build());
    let verb = if p.kind == Kind::Manual { "Moving" } else { "Removing" };
    let mut text = format!("{verb} {}…", p.title);
    if root {
        text.push_str("\nApprove the password prompt if it appears.");
    }
    bx.append(&gtk::Label::builder().label(text).justify(gtk::Justification::Center).build());
    busy.set_child(Some(&bx));
    busy.present(Some(&st.win));
    busy
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
