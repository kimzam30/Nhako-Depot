//! Uninstall flow for one app or several: check what would happen, confirm, run in the progress
//! sheet, and say how it went.

use super::operation::{self, Choice, Ending, ItemState, Operation};
use super::{State, reload, select, widgets};
use crate::packagekit;
use crate::system::{Outcome, home, short};
use adw::prelude::*;
use depot_core::backend::manual;
use depot_core::remove::{self as removal, RemovePlan};
use depot_core::{Kind, Package};
use gtk::{gio, glib};
use std::collections::BTreeMap;
use std::process::Command;
use std::rc::Rc;

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn blocked(reason: impl Into<String>) -> RemovePlan {
    RemovePlan { blocked: Some(reason.into()), ..Default::default() }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Run a dry run twice, without and with "also remove what's left unused", and build a plan
/// from the two outputs.
fn dry_runs(kind: Kind, prog: &str, without: &[&str], with: &[&str], ids: &[String]) -> RemovePlan {
    let sim = |args: &[&str]| {
        Command::new(prog).args(args).arg("--").args(ids).env("LC_ALL", "C.UTF-8").env("LANGUAGE", "C").output()
    };
    let (Ok(a), Ok(b)) = (sim(without), sim(with)) else { return blocked(format!("{prog} isn't available.")) };
    for o in [&a, &b] {
        if let Some(r) = removal::refusal(&text(&o.stdout), &text(&o.stderr), o.status.success()) {
            return blocked(r);
        }
    }
    removal::plan_from_dry_runs(kind, ids, &text(&b.stdout), &text(&a.stdout))
}

/// Ask the package manager itself, for systems without PackageKit.
fn simulate(kind: Kind, ids: &[String]) -> RemovePlan {
    match kind {
        Kind::Deb => dry_runs(kind, "apt-get", &["-s", "remove"], &["-s", "remove", "--autoremove"], ids),
        Kind::Pacman => {
            dry_runs(kind, "pacman", &["-Rcp", "--print-format", "%n"], &["-Rcsp", "--print-format", "%n"], ids)
        }
        _ => blocked("Removing system packages here needs PackageKit, which isn't installed."),
    }
}

/// Architectures that fit any machine.
fn neutral(arch: &str) -> bool {
    matches!(arch, "" | "all" | "noarch" | "any")
}

/// PackageKit's id for an installed package, matched by name and architecture.
fn pk_target<'a>(found: &'a [packagekit::PkPackage], p: &Package) -> Option<&'a str> {
    found
        .iter()
        .find(|k| {
            let mut f = k.id.split(';');
            f.next() == Some(p.name.as_str())
                && matches!(f.nth(1), Some(a) if a == p.arch || neutral(a) || neutral(&p.arch))
        })
        .map(|k| k.id.as_str())
}

/// Dry-run removing these native packages with PackageKit, or with the package manager itself
/// when PackageKit isn't installed.
async fn plan_native(pkgs: &[Package]) -> RemovePlan {
    let kind = pkgs[0].kind;
    let names: Vec<String> = pkgs.iter().map(|p| p.name.clone()).collect();
    if packagekit::available().await
        && let Ok(found) = packagekit::resolve_installed(&names).await
    {
        let targets: Option<Vec<String>> = pkgs.iter().map(|p| pk_target(&found, p).map(str::to_string)).collect();
        if let Some(targets) = targets {
            // two dry runs tell apart what must go with them and what is merely left unused
            let with = packagekit::simulate_remove(&targets, true).await;
            let without = packagekit::simulate_remove(&targets, false).await;
            let pairs = |l: Vec<packagekit::PkPackage>| l.into_iter().map(|k| (k.info, k.id)).collect::<Vec<_>>();
            match (with, without) {
                (Ok(with), Ok(without)) => {
                    return removal::plan_from_packagekit(kind, &targets, &pairs(with), &pairs(without));
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
    let ids: Vec<String> = pkgs.iter().map(|p| p.id.clone()).collect();
    gio::spawn_blocking(move || simulate(kind, &ids)).await.unwrap_or_else(|_| blocked("The check crashed."))
}

/// Show a button as busy ("Checking…") until the returned guard is dropped.
struct Busy(gtk::Button, Option<gtk::Widget>);

impl Busy {
    fn new(b: &gtk::Button) -> Busy {
        let old = b.child();
        let bx = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
        bx.append(&gtk::Spinner::builder().spinning(true).build());
        bx.append(&gtk::Label::new(Some("Checking…")));
        b.set_child(Some(&bx));
        b.set_sensitive(false);
        Busy(b.clone(), old)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.set_child(self.1.as_ref());
        self.0.set_sensitive(true);
    }
}

/// Start uninstalling `pkgs`: work out what would happen, then ask. `button` shows the check.
pub fn start(st: &Rc<State>, pkgs: Vec<Package>, button: Option<&gtk::Button>) {
    if pkgs.is_empty() {
        return;
    }
    let busy = button.map(Busy::new);
    let st = st.clone();
    glib::spawn_future_local(async move {
        let natives: Vec<Package> = pkgs.iter().filter(|p| p.kind.is_native()).cloned().collect();
        let plan = if natives.is_empty() { RemovePlan::default() } else { plan_native(&natives).await };
        drop(busy);
        confirm(&st, Rc::new(pkgs), plan);
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

/// Where Depot remembers that the first-uninstall note was shown.
fn notice_flag() -> std::path::PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(Into::into).unwrap_or_else(|| home().join(".config"));
    config.join("depot/removal-notice-shown")
}

/// What the user chose in the confirmation.
#[derive(Clone, Default)]
struct Choices {
    orphans: bool,
    purge: Vec<Kind>,
    unused_runtimes: bool,
}

fn group_name(k: Kind) -> &'static str {
    match k {
        Kind::Deb | Kind::Rpm | Kind::Pacman => "System packages",
        Kind::Snap => "Snaps",
        Kind::Flatpak => "Flatpaks",
        Kind::AppImage | Kind::Manual => "Moved to the Trash",
    }
}

fn confirm(st: &Rc<State>, pkgs: Rc<Vec<Package>>, plan: RemovePlan) {
    if let Some(reason) = &plan.blocked {
        let heading = if pkgs.len() == 1 { "Can't uninstall" } else { "Can't uninstall these apps" };
        let body = if pkgs.len() == 1 {
            reason.clone()
        } else {
            format!("{reason}\n\nUntick the system apps in your selection and try again.")
        };
        let d = adw::AlertDialog::builder().heading(heading).body(body).build();
        d.add_response("close", "Close");
        d.present(Some(&st.win));
        return;
    }
    let single = pkgs.len() == 1;
    let p = &pkgs[0];
    let mplan = p.manual.clone().unwrap_or_default();
    let full_manual = p.removes_program();
    let extra = &plan.deps;
    let heading = if !single {
        format!("Uninstall {} apps?", pkgs.len())
    } else {
        match (p.kind, full_manual, p.broken) {
            (Kind::Manual, _, true) => format!("Remove the leftover {} launcher?", p.title),
            (Kind::Manual, false, _) => format!("Remove the {} launcher?", p.title),
            _ => format!("Uninstall {}?", p.title),
        }
    };
    let body = if !single {
        "Here's everything that will change. Nothing happens until you press the button below.".to_string()
    } else {
        match p.kind {
            Kind::Manual if p.broken => {
                "The program it started is already gone. The launcher will be moved to the Trash.".to_string()
            }
            Kind::AppImage | Kind::Manual if full_manual => "These files will be moved to the Trash:".to_string(),
            Kind::Manual => {
                format!(
                    "Only the launcher is moved to the Trash. The program itself ({}) is left untouched.",
                    p.location
                )
            }
            Kind::Snap | Kind::Flatpak => "This removes the app from your computer.".to_string(),
            _ if extra.is_empty() => "This removes it from your computer.".to_string(),
            _ => format!(
                "This removes it from your computer, along with {} that need{} it:",
                plural(extra.len(), "package", "packages"),
                if extra.len() == 1 { "s" } else { "" }
            ),
        }
    };
    let d = adw::AlertDialog::builder().heading(heading).body(body).build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    if single {
        let lines: Vec<String> = if p.kind.trashes() {
            mplan.files.iter().map(|f| short(f)).collect()
        } else {
            let mut e = extra.clone();
            e.sort();
            e
        };
        if !lines.is_empty() && (!p.kind.trashes() || full_manual) {
            content.append(&listing(&lines));
        }
    } else {
        let mut by_group: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for q in pkgs.iter() {
            let mut line = q.title.clone();
            if q.kind.trashes()
                && let Some(m) = &q.manual
            {
                for f in &m.files {
                    line.push_str(&format!("\n  {}", short(f)));
                }
            }
            by_group.entry(group_name(q.kind)).or_default().push(line);
        }
        for (g, lines) in by_group {
            content.append(&widgets::label(&format!("{g} ({})", lines.len()), &["heading"]));
            content.append(&listing(&lines));
        }
        if !extra.is_empty() {
            content.append(&widgets::label(
                &format!("Also removed, because they need these apps ({})", extra.len()),
                &["heading"],
            ));
            content.append(&listing(extra));
        }
    }
    let orphans = &plan.orphans;
    let clean = gtk::CheckButton::builder()
        .label(format!("Also remove {} that nothing else needs any more", plural(orphans.len(), "package", "packages")))
        .active(true)
        .tooltip_text("These were installed automatically for other software that is no longer here.")
        .build();
    if !orphans.is_empty() {
        content.append(&clean);
        content.append(&listing(orphans));
    }

    // one "also delete settings" box per kind that can do it; dnf has no way to delete settings files
    let kinds: Vec<Kind> = {
        // AppImages and manual installs share one "settings and data" box
        let mut k: Vec<Kind> = pkgs.iter().map(|q| if q.kind.trashes() { Kind::Manual } else { q.kind }).collect();
        k.sort_by_key(|k| k.css());
        k.dedup();
        k
    };
    let mut purges: Vec<(Kind, gtk::CheckButton)> = Vec::new();
    for &k in &kinds {
        let label = match k {
            Kind::Snap => "Don't keep a backup snapshot of its data".to_string(),
            // apt's purge removes the package's files in /etc, not anything in your home folder
            Kind::Deb => "Also delete its system-wide settings files".to_string(),
            Kind::Pacman => "Don't keep backups of system-wide settings files you changed".to_string(),
            Kind::Flatpak => "Also delete its data and settings".to_string(),
            Kind::Manual => {
                let data: Vec<String> = pkgs
                    .iter()
                    .filter(|q| q.kind.trashes())
                    .flat_map(|q| q.manual.iter().flat_map(|m| m.data.iter().map(|d| short(d))))
                    .collect();
                if data.is_empty() {
                    continue;
                }
                format!("Also move its settings and data to the Trash ({})", data.join(", "))
            }
            Kind::Rpm | Kind::AppImage => continue,
        };
        let label = if single { label } else { label.replace("its ", "their ").replace(" of its", " of their") };
        let c = gtk::CheckButton::with_label(&label);
        if !single {
            c.set_tooltip_text(Some(&format!("For the {} in this list", group_name(k).to_lowercase())));
        }
        content.append(&c);
        purges.push((k, c));
    }
    let unused = gtk::CheckButton::builder()
        .label("Also remove shared Flatpak runtimes that no app uses any more")
        .tooltip_text("Frees space. Apps that need a runtime again download it when they're installed.")
        .build();
    if kinds.contains(&Kind::Flatpak) {
        content.append(&unused);
    }
    let flag = notice_flag();
    if !flag.exists() {
        content.append(&widgets::wrapped(
            "Depot uses your system's own package manager. It always shows what will change before anything \
             happens, and files you installed by hand go to the Trash so you can restore them.",
            &["caption", "dim-label"],
        ));
    }
    d.set_extra_child(Some(&content));
    d.add_response("cancel", "Cancel");
    let only_manual = kinds == [Kind::Manual];
    let (deps_n, orphans_n, apps_n) = (extra.len(), orphans.len(), pkgs.len());
    let native_single = single && p.kind.is_native();
    let action = move |with_orphans: bool| {
        let n = 1 + deps_n + if with_orphans { orphans_n } else { 0 };
        if only_manual {
            "Move to Trash".to_string()
        } else if apps_n > 1 {
            format!("Uninstall {apps_n} apps")
        } else if native_single && n > 1 {
            format!("Uninstall {n} packages")
        } else {
            "Uninstall".to_string()
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
    let st = st.clone();
    d.connect_response(None, move |_, r| {
        if r != "remove" {
            return;
        }
        if let Some(dir) = flag.parent() {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(&flag, "");
        }
        let choices = Choices {
            // only the leftovers that were listed, and only if the box was shown and ticked
            orphans: !plan.orphans.is_empty() && clean.is_active(),
            purge: purges.iter().filter(|(_, c)| c.is_active()).map(|(k, _)| *k).collect(),
            unused_runtimes: unused.is_active(),
        };
        run(&st, pkgs.clone(), plan.clone(), choices);
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
    if errors.is_empty() {
        Outcome::Done
    } else {
        Outcome::Failed { summary: "Some files couldn't be moved to the Trash.".into(), details: errors.join("\n") }
    }
}

/// One step of an uninstall: the packages it covers, as indices into the list being removed.
struct Step {
    items: Vec<usize>,
    how: How,
}

enum How {
    PackageKit(Vec<String>),
    Command { cmd: Vec<String>, root: bool },
    Trash { with_data: bool },
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Removes the Flatpak runtimes no app needs from one installation. Flatpak itself decides
/// which those are, and asks for the password for the system-wide installation.
fn unused_runtimes_cmd(system: bool) -> Vec<String> {
    args(&["flatpak", "uninstall", "--unused", "-y", "--noninteractive", if system { "--system" } else { "--user" }])
}

/// The steps that remove `pkgs` as planned and chosen.
fn steps(pkgs: &[Package], plan: &RemovePlan, c: &Choices) -> Vec<Step> {
    let mut out = Vec::new();
    let pick = |f: &dyn Fn(&Package) -> bool| -> Vec<usize> { (0..pkgs.len()).filter(|&i| f(&pkgs[i])).collect() };
    let purge = |k: Kind| c.purge.contains(&k);
    let (orphans, orphan_pk_ids) =
        if c.orphans { (plan.orphans.clone(), plan.orphan_pk_ids.clone()) } else { (Vec::new(), Vec::new()) };

    let natives = pick(&|p| p.kind.is_native());
    if !natives.is_empty() {
        let kind = pkgs[natives[0]].kind;
        let ids: Vec<String> = natives.iter().map(|&i| pkgs[i].id.clone()).collect();
        // PackageKit can't purge settings files, so purging goes through apt or pacman
        let how = if !plan.pk_ids.is_empty() && !purge(kind) {
            How::PackageKit(plan.pk_ids.iter().chain(&orphan_pk_ids).cloned().collect())
        } else {
            match kind {
                Kind::Deb => How::Command { cmd: removal::apt_command(&ids, purge(kind), &orphans), root: true },
                Kind::Pacman => {
                    How::Command { cmd: removal::pacman_command(&ids, purge(kind), &plan.deps, &orphans), root: true }
                }
                // RPM is only ever removed through PackageKit (see `simulate`)
                _ => How::PackageKit(plan.pk_ids.iter().chain(&orphan_pk_ids).cloned().collect()),
            }
        };
        out.push(Step { items: natives, how });
    }
    let snaps = pick(&|p| p.kind == Kind::Snap);
    if !snaps.is_empty() {
        let mut cmd = args(&["snap", "remove"]);
        if purge(Kind::Snap) {
            cmd.push("--purge".into());
        }
        cmd.push("--".into());
        cmd.extend(snaps.iter().map(|&i| pkgs[i].id.clone()));
        out.push(Step { items: snaps, how: How::Command { cmd, root: true } });
    }
    for system in [true, false] {
        let flats = pick(&|p| p.kind == Kind::Flatpak && p.system == system);
        if flats.is_empty() {
            continue;
        }
        let scope = if system { "--system" } else { "--user" };
        let mut cmd = args(&["flatpak", "uninstall", "-y", "--noninteractive", scope]);
        if purge(Kind::Flatpak) {
            cmd.push("--delete-data".into());
        }
        cmd.push("--".into());
        cmd.extend(flats.iter().map(|&i| pkgs[i].id.clone()));
        // Flatpak asks for the password itself for apps installed for everyone
        out.push(Step { items: flats.clone(), how: How::Command { cmd, root: false } });
        if c.unused_runtimes {
            out.push(Step { items: Vec::new(), how: How::Command { cmd: unused_runtimes_cmd(system), root: false } });
        }
    }
    for i in pick(&|p| p.kind.trashes()) {
        out.push(Step { items: vec![i], how: How::Trash { with_data: purge(Kind::Manual) } });
    }
    out
}

/// Names for the sheet's checklist; copies of one app say how each was installed.
fn item_names(pkgs: &[Package]) -> Vec<String> {
    pkgs.iter()
        .map(|p| {
            if pkgs.iter().filter(|q| q.title == p.title).count() > 1 {
                format!("{} ({})", p.title, p.kind.badge())
            } else {
                p.title.clone()
            }
        })
        .collect()
}

fn run(st: &Rc<State>, pkgs: Rc<Vec<Package>>, plan: RemovePlan, choices: Choices) {
    let only_manual = pkgs.iter().all(|p| p.kind.trashes());
    let title = match (pkgs.len(), only_manual) {
        (1, true) => format!("Moving {} to the Trash", pkgs[0].title),
        (1, false) => format!("Uninstalling {}", pkgs[0].title),
        (n, _) => format!("Uninstalling {n} apps"),
    };
    let op = Operation::show(&st.win, &title, &item_names(&pkgs));
    let st = st.clone();
    glib::spawn_future_local(async move {
        let mut removed = vec![false; pkgs.len()];
        let mut failures: Vec<(Vec<usize>, Outcome)> = Vec::new();
        let mut stopped = false;
        for step in steps(&pkgs, &plan, &choices) {
            if stopped {
                for &i in &step.items {
                    op.item(i, ItemState::Skipped, "Not started");
                }
                continue;
            }
            for &i in &step.items {
                op.item(i, ItemState::Working, "");
            }
            let first = step.items.first().map(|&i| pkgs[i].title.clone()).unwrap_or_default();
            let working = match step.items.len() {
                0 => "Removing unused runtimes…".to_string(),
                1 => format!("Removing {first}…"),
                n => format!("Removing {n} apps…"),
            };
            let o = match &step.how {
                How::PackageKit(ids) => {
                    op.progress("Preparing…", None);
                    let cancel = gio::Cancellable::new();
                    let report = operation::pk_reporter(&op, &cancel, |n| format!("Removing {n}…"));
                    match packagekit::remove(ids, false, false, &cancel, report).await {
                        Ok(_) => Outcome::Done,
                        Err(e) => e.outcome(),
                    }
                }
                How::Command { cmd, root } => {
                    let may_ask = cmd.first().is_some_and(|c| c == "flatpak") && cmd.iter().any(|a| a == "--system");
                    op.set_hint(if may_ask { "You may be asked for your password." } else { "" });
                    operation::command(&op, cmd, *root, &working).await
                }
                How::Trash { with_data } => {
                    op.progress(&format!("Moving {first} to the Trash…"), None);
                    trash_manual(&pkgs[step.items[0]], *with_data).await
                }
            };
            let note_done = if matches!(step.how, How::Trash { .. }) { "Moved to Trash" } else { "Removed" };
            match &o {
                Outcome::Done => {
                    for &i in &step.items {
                        removed[i] = true;
                        op.item(i, ItemState::Done, note_done);
                    }
                }
                Outcome::Cancelled => {
                    stopped = true;
                    for &i in &step.items {
                        op.item(i, ItemState::Skipped, "Cancelled");
                    }
                    failures.push((step.items.clone(), o));
                }
                Outcome::Failed { .. } => {
                    for &i in &step.items {
                        op.item(i, ItemState::Failed, "Failed");
                    }
                    failures.push((step.items.clone(), o));
                }
            }
        }
        let changed = removed.iter().any(|&r| r);
        let left: Vec<Package> = pkgs.iter().zip(&removed).filter(|(_, r)| !**r).map(|(p, _)| p.clone()).collect();
        let r = result(&pkgs, &removed, &failures);
        let retry = op.finish(r).await == Choice::Retry;
        if changed {
            st.nav.pop_to_page(&st.main_page);
            select::finish(&st);
            reload(&st);
        }
        if retry {
            op.close();
            start(&st, left, None);
        }
    });
}

/// Ask before removing the Flatpak runtimes no app needs any more, then remove them.
pub fn clean_up_runtimes(st: &Rc<State>, runtimes: Vec<Package>) {
    if runtimes.is_empty() {
        return;
    }
    let size: u64 = runtimes.iter().map(|p| p.size).sum();
    let heading = if runtimes.len() == 1 {
        "Remove 1 unused runtime?".to_string()
    } else {
        format!("Remove {} unused runtimes?", runtimes.len())
    };
    let d = adw::AlertDialog::builder()
        .heading(heading)
        .body(format!(
            "No installed app needs these any more. This frees about {}. An app that needs one again \
             downloads it when it's installed.",
            glib::format_size(size)
        ))
        .build();
    let lines: Vec<String> = runtimes.iter().map(runtime_line).collect();
    d.set_extra_child(Some(&listing(&lines)));
    d.add_response("cancel", "Cancel");
    d.add_response("remove", "Remove");
    d.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.present(Some(&st.win));
    let st = st.clone();
    d.connect_response(Some("remove"), move |_, _| run_clean_up(&st, Rc::new(runtimes.clone())));
}

/// "GNOME Platform 48, for everyone (1.1 GB)": which copy, for whom, and how big.
fn runtime_line(p: &Package) -> String {
    let who = if p.system { "for everyone" } else { "only for you" };
    let size = if p.size > 0 { format!(" ({})", glib::format_size(p.size)) } else { String::new() };
    format!("{} {}, {who}{size}", p.name, p.revision)
}

fn run_clean_up(st: &Rc<State>, runtimes: Rc<Vec<Package>>) {
    if runtimes.is_empty() {
        return;
    }
    let names: Vec<String> = runtimes.iter().map(runtime_line).collect();
    let op = Operation::show(&st.win, "Removing unused runtimes", &names);
    let st = st.clone();
    glib::spawn_future_local(async move {
        let mut removed = vec![false; runtimes.len()];
        let mut failures: Vec<(Vec<usize>, Outcome)> = Vec::new();
        for system in [true, false] {
            let items: Vec<usize> = (0..runtimes.len()).filter(|&i| runtimes[i].system == system).collect();
            if items.is_empty() {
                continue;
            }
            if failures.iter().any(|(_, o)| matches!(o, Outcome::Cancelled)) {
                items.iter().for_each(|&i| op.item(i, ItemState::Skipped, "Not started"));
                continue;
            }
            items.iter().for_each(|&i| op.item(i, ItemState::Working, ""));
            op.set_hint(if system { "You may be asked for your password." } else { "" });
            let o = operation::command(&op, &unused_runtimes_cmd(system), false, "Removing unused runtimes…").await;
            let (state, note) = match o {
                Outcome::Done => (ItemState::Done, "Removed"),
                Outcome::Cancelled => (ItemState::Skipped, "Cancelled"),
                Outcome::Failed { .. } => (ItemState::Failed, "Failed"),
            };
            for &i in &items {
                removed[i] = state == ItemState::Done;
                op.item(i, state, note);
            }
            if !matches!(o, Outcome::Done) {
                failures.push((items, o));
            }
        }
        let mut r = result(&runtimes, &removed, &failures);
        let done = removed.iter().filter(|&&d| d).count();
        // `result` speaks of apps; say runtimes instead
        r.heading = match (r.ending, runtimes.len()) {
            (Ending::Success, 1) => "The unused runtime was removed".into(),
            (Ending::Success, _) => "Unused runtimes were removed".into(),
            (Ending::Partial, n) => format!("{done} of {n} runtimes were removed"),
            (Ending::Failed, _) => "Couldn't remove the unused runtimes".into(),
            (Ending::Cancelled, _) => r.heading,
        };
        let changed = done > 0;
        let retry = op.finish(r).await == Choice::Retry;
        if changed {
            reload(&st);
        }
        if retry {
            op.close();
            let left: Vec<Package> =
                runtimes.iter().zip(&removed).filter(|(_, r)| !**r).map(|(p, _)| p.clone()).collect();
            run_clean_up(&st, Rc::new(left));
        }
    });
}

/// The result page: what was removed and what wasn't, in plain words.
fn result(pkgs: &[Package], removed: &[bool], failures: &[(Vec<usize>, Outcome)]) -> operation::Result {
    let n = pkgs.len();
    let done = removed.iter().filter(|&&r| r).count();
    let freed: u64 = pkgs.iter().zip(removed).filter(|(_, r)| **r).map(|(p, _)| p.size).sum();
    let freed = if freed > 0 { format!("About {} freed.", glib::format_size(freed)) } else { String::new() };
    let to_trash = pkgs.iter().zip(removed).any(|(p, r)| *r && p.kind.trashes());
    let restore = if to_trash { " You can restore what went to the Trash from the Trash." } else { "" };
    let cancelled = failures.iter().any(|(_, o)| matches!(o, Outcome::Cancelled));
    let summary = failures
        .iter()
        .find_map(|(_, o)| if let Outcome::Failed { summary, .. } = o { Some(summary.clone()) } else { None })
        .unwrap_or_default();
    let details: Vec<String> = failures
        .iter()
        .filter_map(|(items, o)| {
            let Outcome::Failed { details, .. } = o else { return None };
            let names: Vec<&str> = items.iter().map(|&i| pkgs[i].title.as_str()).collect();
            Some(if names.is_empty() { details.clone() } else { format!("{}:\n{details}", names.join(", ")) })
        })
        .collect();
    let not_removed: Vec<&str> =
        pkgs.iter().zip(removed).filter(|(_, r)| !**r).map(|(p, _)| p.title.as_str()).collect();
    let (ending, heading, body) = if failures.is_empty() || done == n && !cancelled {
        let heading = match (n, pkgs[0].kind) {
            (1, k) if k.trashes() => format!("{} was moved to the Trash", pkgs[0].title),
            (1, _) => format!("{} was uninstalled", pkgs[0].title),
            _ => format!("{n} apps were uninstalled"),
        };
        (Ending::Success, heading, format!("{freed}{restore}").trim().to_string())
    } else if done == 0 && cancelled {
        (
            Ending::Cancelled,
            "Nothing was changed".to_string(),
            "The password wasn't entered, so nothing was removed.".to_string(),
        )
    } else if done == 0 {
        let heading = if n == 1 { format!("Couldn't uninstall {}", pkgs[0].title) } else { "Uninstall failed".into() };
        (Ending::Failed, heading, summary)
    } else {
        let what = if cancelled { "You cancelled before the rest were removed." } else { summary.as_str() };
        (
            Ending::Partial,
            format!("{done} of {n} apps were removed"),
            format!(
                "{} {} still installed. {what}",
                not_removed.join(", "),
                if not_removed.len() == 1 { "is" } else { "are" }
            ),
        )
    };
    operation::Result { ending, heading, body, details: details.join("\n\n"), retry: ending != Ending::Success }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(kind: Kind, id: &str) -> Package {
        let mut p = Package::new(kind, id);
        p.title = id.into();
        p
    }

    #[test]
    fn one_step_per_tool_and_one_per_trashed_app() {
        let mut fs = pkg(Kind::Flatpak, "org.x.A");
        fs.system = true;
        let pkgs = vec![
            pkg(Kind::Deb, "vlc"),
            pkg(Kind::Snap, "spotify"),
            pkg(Kind::Snap, "code"),
            fs,
            pkg(Kind::Manual, "t1"),
            pkg(Kind::Manual, "t2"),
            pkg(Kind::AppImage, "t3"),
        ];
        let plan = RemovePlan { pk_ids: vec!["vlc;1;amd64;installed".into()], ..Default::default() };
        let s = steps(&pkgs, &plan, &Choices::default());
        let items: Vec<Vec<usize>> = s.iter().map(|s| s.items.clone()).collect();
        assert_eq!(items, vec![vec![0], vec![1, 2], vec![3], vec![4], vec![5], vec![6]]);
        assert!(matches!(s[5].how, How::Trash { .. }), "AppImages go to the Trash");
        assert!(matches!(&s[0].how, How::PackageKit(ids) if ids.len() == 1));
        match &s[1].how {
            How::Command { cmd, root } => {
                assert!(*root);
                assert_eq!(cmd, &args(&["snap", "remove", "--", "spotify", "code"]));
            }
            _ => panic!("snaps go through one command"),
        }
    }

    #[test]
    fn purging_a_deb_goes_through_apt() {
        let pkgs = vec![pkg(Kind::Deb, "vlc")];
        let plan = RemovePlan { pk_ids: vec!["vlc;1;amd64;installed".into()], ..Default::default() };
        let c = Choices { purge: vec![Kind::Deb], ..Default::default() };
        assert!(matches!(&steps(&pkgs, &plan, &c)[0].how, How::Command { cmd, .. } if cmd[1] == "purge"));
    }

    #[test]
    fn partial_results_name_what_is_left() {
        let pkgs = vec![pkg(Kind::Snap, "a"), pkg(Kind::Manual, "b")];
        let r = result(&pkgs, &[false, true], &[(vec![0], Outcome::failed("error: snap has running apps"))]);
        assert!(r.ending == Ending::Partial && r.retry);
        assert!(r.body.starts_with("a is still installed."), "{}", r.body);
        let all = result(&pkgs, &[true, true], &[]);
        assert!(all.ending == Ending::Success && all.heading == "2 apps were uninstalled");
        let none = result(&pkgs[..1], &[false], &[(vec![0], Outcome::Cancelled)]);
        assert!(none.ending == Ending::Cancelled);
    }
}
