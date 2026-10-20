//! The page shown when a card is clicked.

use super::widgets::{self, label, wrapped};
use super::{State, remove};
use crate::system;
use adw::prelude::*;
use depot_core::backend::manual;
use depot_core::{Kind, Package, Role};
use gtk::{gio, glib};
use std::rc::Rc;

pub fn remove_label(p: &Package) -> &'static str {
    if p.broken {
        "Remove leftover launcher"
    } else if p.kind == Kind::Manual && p.manual.as_ref().is_none_or(|m| m.appdir.is_none()) {
        "Remove launcher"
    } else {
        "Uninstall"
    }
}

/// Open the page for `p`. `others` are other copies of the same app installed different ways.
pub fn show(st: &Rc<State>, p: Package, others: Vec<Package>) {
    let p = Rc::new(p);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    let bx = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .margin_top(24)
        .margin_bottom(32)
        .margin_start(16)
        .margin_end(16)
        .build();

    // header: icon, title, summary, badges, buttons
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 20);
    let img = gtk::Image::builder().pixel_size(112).valign(gtk::Align::Start).build();
    img.set_from_gicon(&widgets::gicon(&p));
    top.append(&img);
    let info = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    info.append(&wrapped(&p.title, &["title-1"]));
    info.append(&wrapped(&p.summary, &["dim-label"]));
    let bb = gtk::Box::builder().spacing(8).margin_top(2).build();
    bb.append(&widgets::kind_badge(p.kind));
    for b in widgets::extra_badges(&p) {
        bb.append(&b);
    }
    info.append(&bb);
    let btns = gtk::Box::builder().spacing(10).margin_top(10).build();
    if let (Some(desktop), false) = (&p.desktop, p.broken) {
        let op = gtk::Button::with_label("Open");
        op.add_css_class("suggested-action");
        op.add_css_class("pill");
        let (st, desktop) = (st.clone(), desktop.clone());
        op.connect_clicked(move |_| launch(&st, &desktop));
        btns.append(&op);
    }
    let rm = gtk::Button::with_label(remove_label(&p));
    rm.add_css_class("destructive-action");
    rm.add_css_class("pill");
    match &p.blocked {
        None => {
            let (st, p) = (st.clone(), p.clone());
            rm.connect_clicked(move |b| remove::start(&st, &p, b));
        }
        Some(reason) => {
            rm.set_sensitive(false);
            rm.set_tooltip_text(Some(reason));
        }
    }
    btns.append(&rm);
    info.append(&btns);
    top.append(&info);
    bx.append(&top);
    if let Some(reason) = &p.blocked {
        bx.append(&wrapped(reason, &["dim-label"]));
    }
    if !others.is_empty() {
        bx.append(&copies(st, &p, &others));
    }

    let desc = if p.description.is_empty() { p.summary.as_str() } else { p.description.as_str() };
    bx.append(&label("About", &["title-4"]));
    let d = wrapped(if desc.is_empty() { "No description available." } else { desc }, &[]);
    d.set_selectable(true);
    bx.append(&d);

    // details list
    let grp = adw::PreferencesGroup::builder().title("Details").build();
    let row = |title: &str, value: &str| {
        let r = adw::ActionRow::builder()
            .title(title)
            .subtitle(glib::markup_escape_text(value))
            .subtitle_selectable(true)
            .build();
        r.add_css_class("property");
        r
    };
    let size_row = row("Installed size", &widgets::size_text(p.size));
    let lazy_size = p.size == 0 && p.manual.as_ref().is_some_and(|m| m.appdir.is_some());
    for (t, v) in [("Source", p.kind.source()), ("Version", p.version.as_str())] {
        if !v.is_empty() {
            grp.add(&row(t, v));
        }
    }
    if p.size > 0 || lazy_size {
        grp.add(&size_row);
    }
    let mut rows: Vec<(&str, String)> =
        vec![(if p.kind == Kind::Manual { "Launcher" } else { "Package name" }, p.id.clone())];
    match p.kind {
        Kind::Snap => rows.extend([
            ("Publisher", p.publisher.clone()),
            ("Channel", p.channel.clone()),
            ("Revision", p.revision.clone()),
        ]),
        Kind::Flatpak => rows.extend([
            ("Origin", p.channel.clone()),
            ("Installed for", if p.system { "All users".into() } else { "Only you".into() }),
            ("Branch", p.revision.clone()),
        ]),
        Kind::Manual => rows.push(("Location", p.location.clone())),
        Kind::Rpm => rows.push(("Group", p.section.clone())),
        Kind::Deb | Kind::Pacman => {
            rows.push((if p.kind == Kind::Deb { "Section" } else { "Groups" }, p.section.clone()));
            if p.foreign {
                rows.push(("Comes from", p.channel.clone()));
            }
            if p.role != Role::App {
                let how = if p.user_installed { "By you" } else { "Automatically, because something else needs it" };
                rows.push(("Installed", how.into()));
            }
        }
    }
    for (t, v) in &rows {
        if !v.is_empty() {
            grp.add(&row(t, v));
        }
    }
    for l in &p.extra_launchers {
        grp.add(&row("Also has launcher", &system::short(l)));
    }
    if !p.homepage.is_empty() {
        let r = adw::ActionRow::builder()
            .title("Website")
            .subtitle(glib::markup_escape_text(&p.homepage))
            .activatable(true)
            .build();
        r.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
        let (win, url) = (st.win.clone(), p.homepage.clone());
        r.connect_activated(move |_| gtk::UriLauncher::new(&url).launch(Some(&win), None::<&gio::Cancellable>, |_| {}));
        grp.add(&r);
    }
    bx.append(&grp);

    if lazy_size {
        size_row.set_subtitle("Calculating…");
        let dir = p.manual.as_ref().and_then(|m| m.appdir.clone()).unwrap_or_default();
        glib::spawn_future_local(async move {
            let n = gio::spawn_blocking(move || manual::dir_size(&dir)).await.unwrap_or(0);
            size_row.set_subtitle(&widgets::size_text(n));
        });
    }

    tv.set_content(Some(
        &gtk::ScrolledWindow::builder().child(&adw::Clamp::builder().maximum_size(760).child(&bx).build()).build(),
    ));
    st.nav.push(&adw::NavigationPage::new(&tv, &p.title));
}

/// The "Installed N ways" list: every copy of the app, each with a button to its own page.
fn copies(st: &Rc<State>, p: &Rc<Package>, others: &[Package]) -> adw::PreferencesGroup {
    let n = others.len() + 1;
    let grp = adw::PreferencesGroup::builder()
        .title(format!("Installed {n} ways"))
        .description("This app is on your computer more than once. You usually only need one, so you can remove the copies you don't use.")
        .build();
    let all: Vec<Package> = std::iter::once((**p).clone()).chain(others.iter().cloned()).collect();
    for (i, c) in all.iter().enumerate() {
        let mut sub = vec![c.version.clone(), widgets::size_text(c.size)];
        if c.kind == Kind::Flatpak {
            sub.push(if c.system { "All users".into() } else { "Only you".into() });
        }
        let sub: Vec<String> = sub.into_iter().filter(|s| !s.is_empty()).collect();
        let row = adw::ActionRow::builder()
            .title(c.kind.source())
            .subtitle(glib::markup_escape_text(&sub.join(" · ")))
            .build();
        row.add_prefix(&widgets::kind_badge(c.kind));
        if i == 0 {
            row.add_suffix(&label("This page", &["dim-label", "caption"]));
        } else {
            let b = gtk::Button::builder().label("Show").valign(gtk::Align::Center).build();
            let (st, c) = (st.clone(), c.clone());
            let rest: Vec<Package> = all.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| o.clone()).collect();
            b.connect_clicked(move |_| show(&st, c.clone(), rest.clone()));
            row.add_suffix(&b);
            row.set_activatable_widget(Some(&b));
        }
        grp.add(&row);
    }
    grp
}

fn launch(st: &State, desktop: &std::path::Path) {
    let args = [std::ffi::OsStr::new("gio"), "launch".as_ref(), desktop.as_os_str()];
    if let Err(e) = gio::Subprocess::newv(&args, gio::SubprocessFlags::NONE) {
        st.toast(&format!("Couldn't open the app: {e}"));
    }
}
