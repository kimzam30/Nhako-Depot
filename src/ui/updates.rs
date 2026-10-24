//! The Updates page: apps listed one by one, everything else grouped as "System updates".
//! Every update has a tick box, all ticked to start with, so several can be installed at once.

use super::operation::{self, Choice, Ending, ItemState, Operation};
use super::widgets::{self, label};
use super::{State, reload};
use crate::system::Outcome;
use crate::updates;
use adw::prelude::*;
use depot_core::{Kind, Role, Update};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Re-check automatically when the page is opened and the last check is older than this.
const STALE: Duration = Duration::from_secs(10 * 60);

/// Identifies one update across checks, so a refresh keeps what the user unticked.
fn key(u: &Update) -> String {
    format!("{}/{}/{}", u.kind.css(), u.id, u.system)
}

pub struct View {
    pub root: gtk::Stack,
    page: adw::PreferencesPage,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    pub update_all: gtk::Button,
    pending: RefCell<Vec<Update>>,
    /// Updates the user unticked. Everything else is ticked, including updates found later.
    skipped: RefCell<HashSet<String>>,
    /// Tick boxes on the page, by update key.
    checks: RefCell<Vec<(String, gtk::CheckButton)>>,
    /// The tick box on "System updates" that ticks or unticks all of them.
    system_check: RefCell<Option<gtk::CheckButton>>,
    select_all: RefCell<Option<gtk::Button>>,
    /// "You've skipped a security fix" note.
    security_note: gtk::Revealer,
    /// Set while ticks are changed in code, so their handlers don't run again.
    syncing: Cell<bool>,
    /// Sources the last check couldn't reach.
    warnings: RefCell<Vec<String>>,
    offline: adw::StatusPage,
    checked_at: Cell<Option<Instant>>,
    /// A check is running.
    checking: Cell<bool>,
    /// Updates are being installed; the Update buttons are disabled meanwhile.
    installing: Cell<bool>,
    /// A check asked for while another was running, during an install, or before the installed
    /// list was read (`true` = with fresh package lists). It runs as soon as possible instead of
    /// being dropped.
    queued: Cell<Option<bool>>,
}

impl View {
    pub fn new() -> Self {
        let root =
            gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(200).build();
        let spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
        root.add_named(
            &adw::StatusPage::builder()
                .title("Checking for updates…")
                .description("Asking your software sources for newer versions. This takes a few seconds.")
                .child(&spinner)
                .build(),
            Some("checking"),
        );
        let none = adw::StatusPage::builder()
            .icon_name("emblem-ok-symbolic")
            .title("Everything is up to date")
            .description("Depot checks again when you open this page or press Refresh.")
            .build();
        none.add_css_class("op-pop");
        root.add_named(&none, Some("none"));
        let offline = adw::StatusPage::builder()
            .icon_name("network-offline-symbolic")
            .title("Couldn't check for updates")
            .description("Check your internet connection, then try again.")
            .build();
        root.add_named(&offline, Some("offline"));
        let page = adw::PreferencesPage::new();
        let note = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).margin_top(12).build();
        note.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
        note.append(&label("You've unticked a security fix. It's best to install it soon.", &["warning"]));
        let security_note = gtk::Revealer::builder()
            .child(&note)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(200)
            .build();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.append(&security_note);
        list.append(&page);
        page.set_vexpand(true);
        root.add_named(&list, Some("list"));
        let update_all = gtk::Button::builder().label("Update all").visible(false).build();
        update_all.add_css_class("suggested-action");
        View {
            root,
            page,
            groups: RefCell::new(Vec::new()),
            update_all,
            pending: RefCell::new(Vec::new()),
            skipped: RefCell::new(HashSet::new()),
            checks: RefCell::new(Vec::new()),
            system_check: RefCell::new(None),
            select_all: RefCell::new(None),
            security_note,
            syncing: Cell::new(false),
            warnings: RefCell::new(Vec::new()),
            offline,
            checked_at: Cell::new(None),
            checking: Cell::new(false),
            installing: Cell::new(false),
            queued: Cell::new(None),
        }
    }

    /// Number shown next to "Updates" in the sidebar.
    pub fn count(&self) -> usize {
        self.pending.borrow().len()
    }

    /// The ticked updates.
    pub fn selected(&self) -> Vec<Update> {
        let skipped = self.skipped.borrow();
        self.pending.borrow().iter().filter(|u| !skipped.contains(&key(u))).cloned().collect()
    }

    pub fn take_queued(&self) -> Option<bool> {
        self.queued.take()
    }

    /// Ask for a check to run as soon as possible (`true` = with fresh package lists).
    fn queue(&self, refresh: bool) {
        self.queued.set(Some(refresh || self.queued.get().unwrap_or(false)));
    }

    pub fn is_stale(&self) -> bool {
        self.checked_at.get().is_none_or(|t| t.elapsed() > STALE)
    }

    /// Follow the ticks: the header button, the "System updates" tick, "Select all" and the
    /// security note.
    pub fn sync_ticks(&self, on_updates: bool) {
        let pending = self.pending.borrow();
        let skipped = self.skipped.borrow();
        let picked = pending.iter().filter(|u| !skipped.contains(&key(u))).count();
        let b = &self.update_all;
        b.set_visible(on_updates && !pending.is_empty());
        b.set_sensitive(picked > 0 && !self.installing.get());
        b.set_label(&match picked {
            0 => "Nothing selected".to_string(),
            n if n == pending.len() => "Update all".to_string(),
            n => format!("Update {n} selected"),
        });
        b.set_tooltip_text((picked == 0).then_some("Tick the updates you want to install."));
        self.syncing.set(true);
        for (k, c) in self.checks.borrow().iter() {
            c.set_active(!skipped.contains(k));
        }
        if let Some(sc) = self.system_check.borrow().as_ref() {
            let system: Vec<&Update> = pending.iter().filter(|u| u.role != Role::App).collect();
            let on = system.iter().filter(|u| !skipped.contains(&key(u))).count();
            sc.set_inconsistent(on > 0 && on < system.len());
            sc.set_active(on == system.len());
        }
        self.syncing.set(false);
        if let Some(sa) = self.select_all.borrow().as_ref() {
            sa.set_label(if picked == pending.len() { "Select none" } else { "Select all" });
        }
        self.security_note.set_reveal_child(pending.iter().any(|u| u.security && skipped.contains(&key(u))));
    }

    /// Tick or untick these updates.
    fn set_ticked<'a>(&self, keys: impl IntoIterator<Item = &'a str>, on: bool) {
        let mut s = self.skipped.borrow_mut();
        for k in keys {
            if on {
                s.remove(k);
            } else {
                s.insert(k.to_string());
            }
        }
    }
}

fn versions(u: &Update) -> String {
    match (u.from.as_str(), u.to.as_str()) {
        ("", "") => "New version available".into(),
        (f, t) if f == t || t.is_empty() => format!("New build of {f}"),
        ("", t) => format!("Version {t}"),
        (f, t) => format!("{f} → {t}"),
    }
}

fn source(u: &Update) -> &'static str {
    match u.kind {
        Kind::Deb | Kind::Rpm | Kind::Pacman => "System package",
        Kind::Snap => "Snap",
        Kind::Flatpak => "Flatpak",
        Kind::Manual => "",
    }
}

fn update_button(st: &Rc<State>, list: Vec<Update>) -> gtk::Button {
    let b = gtk::Button::builder().label("Update").valign(gtk::Align::Center).build();
    let st = st.clone();
    b.connect_clicked(move |_| run(&st, list.clone()));
    b
}

fn on_updates(st: &State) -> bool {
    st.view.visible_child_name().as_deref() == Some("updates")
}

/// A tick box for `keys`; toggling it by hand ticks or unticks all of them.
fn tick(st: &Rc<State>, keys: Vec<String>) -> gtk::CheckButton {
    let c = gtk::CheckButton::builder().active(true).valign(gtk::Align::Center).build();
    c.set_tooltip_text(Some("Include in the next update"));
    let st = st.clone();
    c.connect_toggled(move |c| {
        let v = &st.updates;
        if v.syncing.get() {
            return;
        }
        v.set_ticked(keys.iter().map(String::as_str), c.is_active());
        v.sync_ticks(on_updates(&st));
    });
    c
}

fn render(st: &Rc<State>) {
    let v = &st.updates;
    let warnings = v.warnings.borrow();
    for g in v.groups.borrow_mut().drain(..) {
        v.page.remove(&g);
    }
    v.checks.borrow_mut().clear();
    *v.system_check.borrow_mut() = None;
    *v.select_all.borrow_mut() = None;
    let add = |g: adw::PreferencesGroup| {
        v.page.add(&g);
        v.groups.borrow_mut().push(g);
    };
    let pending = v.pending.borrow();
    // forget unticks of updates that are gone, so they start ticked if they come back
    let keys: HashSet<String> = pending.iter().map(key).collect();
    v.skipped.borrow_mut().retain(|k| keys.contains(k));
    v.page.set_sensitive(!v.installing.get());

    let select_all = gtk::Button::builder().label("Select none").valign(gtk::Align::Center).build();
    select_all.add_css_class("flat");
    {
        let st = st.clone();
        select_all.connect_clicked(move |_| {
            let v = &st.updates;
            let all: Vec<String> = v.pending.borrow().iter().map(key).collect();
            let everything = v.skipped.borrow().is_empty();
            v.set_ticked(all.iter().map(String::as_str), !everything);
            v.sync_ticks(on_updates(&st));
        });
    }

    let apps: Vec<&Update> = pending.iter().filter(|u| u.role == Role::App).collect();
    if !apps.is_empty() {
        let grp = adw::PreferencesGroup::builder().title("Apps").build();
        for u in apps {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&u.title))
                .subtitle(glib::markup_escape_text(&format!("{} · {}", versions(u), source(u))))
                .build();
            let c = tick(st, vec![key(u)]);
            row.add_prefix(&c);
            row.set_activatable_widget(Some(&c));
            v.checks.borrow_mut().push((key(u), c));
            let img = gtk::Image::builder().pixel_size(40).build();
            img.set_from_gicon(&widgets::gicon_for(u.icon.as_deref(), true));
            row.add_prefix(&img);
            if u.security {
                row.add_suffix(&security_badge());
            }
            row.add_suffix(&update_button(st, vec![u.clone()]));
            grp.add(&row);
        }
        add(grp);
    }

    let system: Vec<Update> = pending.iter().filter(|u| u.role != Role::App).cloned().collect();
    if !system.is_empty() {
        let grp = adw::PreferencesGroup::builder()
            .title("System")
            .description("Libraries, drivers and shared runtimes that your apps rely on.")
            .build();
        let security = system.iter().any(|u| u.security);
        let n = system.len();
        let subtitle = format!(
            "{n} package{}{}",
            if n == 1 { "" } else { "s" },
            if security { ", including security fixes" } else { "" }
        );
        let exp = adw::ExpanderRow::builder().title("System updates").subtitle(subtitle).build();
        let all = tick(st, system.iter().map(key).collect());
        all.set_tooltip_text(Some("Include all system updates"));
        exp.add_prefix(&all);
        *v.system_check.borrow_mut() = Some(all);
        exp.add_prefix(&gtk::Image::builder().icon_name("system-software-update-symbolic").pixel_size(32).build());
        exp.add_suffix(&update_button(st, system.clone()));
        for u in &system {
            let mut sub = format!("{} · {}", versions(u), source(u));
            if u.kind == Kind::Flatpak {
                sub.push_str(&format!(" · {}", u.id));
            }
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&u.title))
                .subtitle(glib::markup_escape_text(&sub))
                .build();
            let c = tick(st, vec![key(u)]);
            row.add_prefix(&c);
            row.set_activatable_widget(Some(&c));
            v.checks.borrow_mut().push((key(u), c));
            if u.security {
                row.add_suffix(&security_badge());
            }
            exp.add_row(&row);
        }
        grp.add(&exp);
        add(grp);
    }
    if let Some(first) = v.groups.borrow().first() {
        first.set_header_suffix(Some(&select_all));
        *v.select_all.borrow_mut() = Some(select_all);
    }

    if !warnings.is_empty() {
        let grp = adw::PreferencesGroup::builder()
            .title("Couldn't check everything")
            .description("Check your internet connection, then press Refresh.")
            .build();
        for w in warnings.iter() {
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(w)).build();
            row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
            grp.add(&row);
        }
        add(grp);
    }
    v.page.set_description(&checked_text(v.checked_at.get()));
    let page = match (pending.is_empty(), warnings.is_empty()) {
        (true, true) => "none",
        (true, false) => {
            v.offline.set_description(Some(&format!(
                "Check your internet connection, then press Refresh.\n\n{}",
                warnings.join("\n")
            )));
            "offline"
        }
        _ => "list",
    };
    v.root.set_visible_child_name(page);
    drop(pending);
    v.sync_ticks(on_updates(st));
}

fn checked_text(at: Option<Instant>) -> String {
    let Some(t) = at else { return String::new() };
    match t.elapsed().as_secs() / 60 {
        0 => "Checked just now".into(),
        1 => "Checked a minute ago".into(),
        m if m < 60 => format!("Checked {m} minutes ago"),
        _ => "Checked over an hour ago".into(),
    }
}

/// Keep "Checked N minutes ago" current.
pub fn tick_clock(st: &Rc<State>) {
    let st = Rc::downgrade(st);
    glib::timeout_add_seconds_local(30, move || {
        let Some(st) = st.upgrade() else { return glib::ControlFlow::Break };
        st.updates.page.set_description(&checked_text(st.updates.checked_at.get()));
        glib::ControlFlow::Continue
    });
}

fn security_badge() -> gtk::Label {
    let l = label("Security", &["badge", "badge-broken"]);
    l.set_valign(gtk::Align::Center);
    l.set_tooltip_text(Some("Fixes a security problem. Install this soon."));
    l
}

/// Check every source. `refresh` also downloads fresh package lists, which is slower.
pub fn check(st: &Rc<State>, refresh: bool) {
    let v = &st.updates;
    // updates are matched against the installed list, so wait for it; never drop a request
    if v.checking.get() || v.installing.get() || st.loading.get() {
        v.queue(refresh);
        // keep showing the list while updates install; the queued check replaces it afterwards
        if refresh && !v.installing.get() {
            v.root.set_visible_child_name("checking");
        }
        return;
    }
    v.checking.set(true);
    if refresh || v.checked_at.get().is_none() {
        v.root.set_visible_child_name("checking");
    }
    let st = st.clone();
    glib::spawn_future_local(async move {
        let inv = st.packages.borrow().clone();
        let checked = updates::check(refresh, &inv).await;
        let v = &st.updates;
        *v.pending.borrow_mut() = checked.updates;
        *v.warnings.borrow_mut() = checked.warnings;
        v.checked_at.set(Some(Instant::now()));
        v.checking.set(false);
        render(&st);
        super::update_counts(&st);
        if let Some(refresh) = v.queued.take() {
            check(&st, refresh);
        }
    });
}

/// After the installed list was re-read without a new check: drop updates of removed software.
pub fn prune(st: &Rc<State>) {
    // a running check renders its own fresh answer
    if st.updates.checking.get() {
        return;
    }
    let inv = st.packages.borrow().clone();
    depot_core::updates::retain_installed(&mut st.updates.pending.borrow_mut(), &inv);
    render(st);
    super::update_counts(st);
}

/// Install updates in the progress sheet, after a dry run shows whether anything would be
/// removed along the way.
pub fn run(st: &Rc<State>, list: Vec<Update>) {
    // the buttons are disabled while installing, so this only guards against a double click
    if list.is_empty() || st.updates.installing.replace(true) {
        return;
    }
    set_installing(st, true);
    let st = st.clone();
    glib::spawn_future_local(async move {
        let changed = install(&st, list).await;
        set_installing(&st, false);
        if changed {
            // reloading checks again afterwards, merged with any check queued meanwhile
            st.updates.queue(false);
            reload(&st);
        } else if let Some(refresh) = st.updates.take_queued() {
            check(&st, refresh);
        }
    });
}

/// The whole install, including "Try again". Returns whether anything was installed.
async fn install(st: &Rc<State>, list: Vec<Update>) -> bool {
    let n = list.len();
    let title = if n == 1 { format!("Updating {}", list[0].title) } else { format!("Installing {n} updates") };
    let batches = updates::batches(&list);
    let titles: Vec<String> = batches.iter().map(|b| b.title.clone()).collect();
    let op = Operation::show(&st.win, &title, &titles);
    // batches still to run, with their place in the sheet's checklist
    let mut todo: Vec<(usize, updates::Batch)> = batches.into_iter().enumerate().collect();
    let mut changed = false;
    let mut done_count = 0;
    loop {
        let native: Vec<String> = todo.iter().filter_map(|(_, b)| b.native_ids()).flatten().cloned().collect();
        if !native.is_empty() {
            op.checking("Checking what else would change…");
            match updates::removals(&native).await {
                Ok(removed) if !removed.is_empty() && !confirm_removals(st, &removed).await => {
                    op.close();
                    return changed;
                }
                Ok(_) => {}
                Err(o) => {
                    let r = result(n, &list, done_count, &[o]);
                    if op.finish(r).await == Choice::Retry {
                        continue;
                    }
                    return changed;
                }
            }
        }
        let mut failures = Vec::new();
        let mut left = Vec::new();
        let mut stopped = false;
        for (i, b) in todo {
            if stopped {
                op.item(i, ItemState::Skipped, "Not started");
                left.push((i, b));
                continue;
            }
            op.item(i, ItemState::Working, "");
            op.set_hint(if updates::may_ask(&b) { "You may be asked for your password." } else { "" });
            let o = updates::apply(&b, &op).await;
            match &o {
                Outcome::Done => {
                    op.item(i, ItemState::Done, "Updated");
                    changed = true;
                    done_count += 1;
                }
                Outcome::Cancelled => {
                    op.item(i, ItemState::Skipped, "Cancelled");
                    stopped = true;
                    failures.push(o);
                    left.push((i, b));
                }
                Outcome::Failed { .. } => {
                    op.item(i, ItemState::Failed, "Failed");
                    failures.push(o);
                    left.push((i, b));
                }
            }
        }
        let r = result(n, &list, done_count, &failures);
        if op.finish(r).await == Choice::Retry && !left.is_empty() {
            todo = left;
            continue;
        }
        return changed;
    }
}

/// The result page for an install of `n` updates in which `done` batches worked.
fn result(n: usize, list: &[Update], done: usize, failures: &[Outcome]) -> operation::Result {
    let details: Vec<String> = failures
        .iter()
        .filter_map(|o| if let Outcome::Failed { details, .. } = o { Some(details.clone()) } else { None })
        .collect();
    let summary = failures
        .iter()
        .find_map(|o| if let Outcome::Failed { summary, .. } = o { Some(summary.clone()) } else { None });
    let cancelled = failures.iter().any(|o| matches!(o, Outcome::Cancelled));
    let (ending, heading, body) = match (failures.is_empty(), done > 0, cancelled) {
        (true, _, _) => (
            Ending::Success,
            if n == 1 { format!("{} is up to date", list[0].title) } else { "Updates installed".into() },
            if n == 1 { String::new() } else { format!("All {n} updates were installed.") },
        ),
        (false, false, true) => (
            Ending::Cancelled,
            "Nothing was changed".into(),
            "The password wasn't entered, so no updates were installed.".into(),
        ),
        (false, true, true) => (
            Ending::Partial,
            "Stopped before the end".into(),
            "Some updates were installed before you cancelled. The rest weren't started.".into(),
        ),
        (false, true, false) => {
            (Ending::Partial, "Some updates couldn't be installed".into(), summary.unwrap_or_default())
        }
        (false, false, false) => (Ending::Failed, "Updates couldn't be installed".into(), summary.unwrap_or_default()),
    };
    operation::Result { ending, heading, body, details: details.join("\n\n"), retry: ending != Ending::Success }
}

fn set_installing(st: &State, on: bool) {
    let v = &st.updates;
    v.installing.set(on);
    v.page.set_sensitive(!on);
    v.sync_ticks(on_updates(st));
}

async fn confirm_removals(st: &State, removed: &[String]) -> bool {
    let n = removed.len();
    let d = adw::AlertDialog::builder()
        .heading("This update removes software")
        .body(format!(
            "To install these updates, {} will be removed:\n\n{}",
            if n == 1 { "1 package".to_string() } else { format!("{n} packages") },
            removed.join(", ")
        ))
        .build();
    d.add_response("cancel", "Cancel");
    d.add_response("go", "Update anyway");
    d.set_response_appearance("go", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.choose_future(Some(st.win.upcast_ref::<gtk::Widget>())).await == "go"
}
