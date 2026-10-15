//! The Updates page: apps listed one by one, everything else grouped as "System updates".

use super::widgets::{self, Progress, label};
use super::{State, reload};
use crate::updates::{self, Done};
use adw::prelude::*;
use depot_core::{Kind, Role, Update};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Re-check automatically when the page is opened and the last check is older than this.
const STALE: Duration = Duration::from_secs(10 * 60);

pub struct View {
    pub root: gtk::Stack,
    page: adw::PreferencesPage,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    pub update_all: gtk::Button,
    pending: RefCell<Vec<Update>>,
    checked_at: Cell<Option<Instant>>,
    busy: Cell<bool>,
    /// A check asked for while another was running or before the installed list was read
    /// (`true` = with fresh package lists). It runs as soon as possible instead of being dropped.
    queued: Cell<Option<bool>>,
}

impl View {
    pub fn new() -> Self {
        let root = gtk::Stack::new();
        let spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
        root.add_named(
            &adw::StatusPage::builder()
                .title("Checking for updates…")
                .description("Asking your software sources for newer versions.")
                .child(&spinner)
                .build(),
            Some("checking"),
        );
        root.add_named(
            &adw::StatusPage::builder()
                .icon_name("emblem-ok-symbolic")
                .title("Everything is up to date")
                .description("Depot checks again when you open this page or press Refresh.")
                .build(),
            Some("none"),
        );
        let page = adw::PreferencesPage::new();
        root.add_named(&page, Some("list"));
        let update_all = gtk::Button::builder().label("Update all").visible(false).build();
        update_all.add_css_class("suggested-action");
        View {
            root,
            page,
            groups: RefCell::new(Vec::new()),
            update_all,
            pending: RefCell::new(Vec::new()),
            checked_at: Cell::new(None),
            busy: Cell::new(false),
            queued: Cell::new(None),
        }
    }

    /// Number shown next to "Updates" in the sidebar.
    pub fn count(&self) -> usize {
        self.pending.borrow().len()
    }

    pub fn pending(&self) -> Vec<Update> {
        self.pending.borrow().clone()
    }

    pub fn take_queued(&self) -> Option<bool> {
        self.queued.take()
    }

    pub fn is_stale(&self) -> bool {
        self.checked_at.get().is_none_or(|t| t.elapsed() > STALE)
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
        Kind::Deb => "System package",
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

fn render(st: &Rc<State>, warnings: &[String]) {
    let v = &st.updates;
    for g in v.groups.borrow_mut().drain(..) {
        v.page.remove(&g);
    }
    let add = |g: adw::PreferencesGroup| {
        v.page.add(&g);
        v.groups.borrow_mut().push(g);
    };
    let pending = v.pending.borrow();
    let on_updates = st.view.visible_child_name().as_deref() == Some("updates");
    v.update_all.set_visible(on_updates && !pending.is_empty());
    v.update_all.set_sensitive(!v.busy.get());

    let apps: Vec<&Update> = pending.iter().filter(|u| u.role == Role::App).collect();
    if !apps.is_empty() {
        let grp = adw::PreferencesGroup::builder().title("Apps").build();
        for u in apps {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&u.title))
                .subtitle(glib::markup_escape_text(&format!("{} · {}", versions(u), source(u))))
                .build();
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
            if u.security {
                row.add_suffix(&security_badge());
            }
            exp.add_row(&row);
        }
        grp.add(&exp);
        add(grp);
    }

    if !warnings.is_empty() {
        let grp = adw::PreferencesGroup::builder()
            .title("Couldn't check everything")
            .description("Check your internet connection, then press Refresh.")
            .build();
        for w in warnings {
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(w)).build();
            row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
            grp.add(&row);
        }
        add(grp);
    }
    v.root.set_visible_child_name(if pending.is_empty() && warnings.is_empty() { "none" } else { "list" });
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
    if v.busy.get() || st.loading.get() {
        v.queued.set(Some(refresh || v.queued.get().unwrap_or(false)));
        if refresh {
            v.root.set_visible_child_name("checking");
        }
        return;
    }
    v.busy.set(true);
    if refresh || v.checked_at.get().is_none() {
        v.root.set_visible_child_name("checking");
    }
    let st = st.clone();
    glib::spawn_future_local(async move {
        let inv = st.packages.borrow().clone();
        let checked = updates::check(refresh, &inv).await;
        let v = &st.updates;
        *v.pending.borrow_mut() = checked.updates;
        v.checked_at.set(Some(Instant::now()));
        v.busy.set(false);
        render(&st, &checked.warnings);
        super::update_counts(&st);
        if let Some(refresh) = v.queued.take() {
            check(&st, refresh);
        }
    });
}

/// Install updates, after a dry run shows whether anything would be removed along the way.
pub fn run(st: &Rc<State>, list: Vec<Update>) {
    if st.updates.busy.replace(true) {
        return;
    }
    st.updates.update_all.set_sensitive(false);
    let st = st.clone();
    glib::spawn_future_local(async move {
        let deb_ids: Vec<String> = list.iter().filter(|u| u.kind == Kind::Deb).map(|u| u.id.clone()).collect();
        let removed = if deb_ids.is_empty() { Vec::new() } else { updates::removals(&deb_ids).await };
        if !removed.is_empty() && !confirm_removals(&st, &removed).await {
            st.updates.busy.set(false);
            st.updates.update_all.set_sensitive(true);
            return;
        }
        let n = list.len();
        let title = if n == 1 { format!("Updating {}", list[0].title) } else { format!("Installing {n} updates") };
        let hint = if list.iter().any(|u| u.kind != Kind::Flatpak || u.system) {
            "Approve the password prompt if it appears."
        } else {
            ""
        };
        let opened = Instant::now();
        let progress = Progress::show(&st.win, &title, hint);
        let p = progress.clone();
        let done = updates::apply(&list, move |text, frac| p.set(text, frac)).await;
        progress.close(opened).await;
        st.updates.busy.set(false);
        match done {
            Done::Ok => {
                st.toast(if n == 1 { "Update installed" } else { "Updates installed" });
                reload(&st);
            }
            Done::Cancelled => {
                st.toast("Cancelled: authentication was not granted");
                st.updates.update_all.set_sensitive(true);
            }
            Done::Failed(msg) => {
                let d = adw::AlertDialog::builder().heading("Update failed").body(msg).build();
                d.add_response("close", "Close");
                d.present(Some(&st.win));
                check(&st, false);
            }
        }
    });
}

async fn confirm_removals(st: &State, removed: &[String]) -> bool {
    let d = adw::AlertDialog::builder()
        .heading("This update removes software")
        .body(format!(
            "To install these updates, {} package(s) will be removed:\n\n{}",
            removed.len(),
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
