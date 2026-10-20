//! Small building blocks shared by the grid and the detail page.

use adw::prelude::*;
use depot_core::{Kind, Package, Role};
use gtk::{gio, glib, pango};
use std::cell::Cell;
use std::rc::Rc;

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).build();
    for c in classes {
        l.add_css_class(c);
    }
    l
}

pub fn wrapped(text: &str, classes: &[&str]) -> gtk::Label {
    let l = label(text, classes);
    l.set_wrap(true);
    l.set_wrap_mode(pango::WrapMode::WordChar);
    l
}

fn pill(text: &str, class: &str, tip: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("badge");
    l.add_css_class(class);
    if !tip.is_empty() {
        l.set_tooltip_text(Some(tip));
    }
    l
}

pub fn kind_badge(k: Kind) -> gtk::Label {
    pill(k.badge(), &format!("badge-{}", k.css()), k.explain())
}

fn aur_badge() -> gtk::Label {
    pill(
        "AUR",
        "badge-aur",
        "Built from the AUR or another source outside your repositories. Pacman doesn't update it.",
    )
}

fn broken_badge() -> gtk::Label {
    pill("Broken", "badge-broken", "The program this launcher starts is no longer installed.")
}

/// Badges shown next to the source badge.
pub fn extra_badges(p: &Package) -> Vec<gtk::Label> {
    let mut v = Vec::new();
    if p.broken {
        v.push(broken_badge());
    }
    if p.foreign {
        v.push(aur_badge());
    }
    match p.role {
        Role::Tool if p.kind.is_native() => v.push(pill("Library / tool", "badge-sys", "")),
        Role::Tool => v.push(pill("No launcher", "badge-sys", "Installed, but it has no entry in the app menu.")),
        Role::Runtime => v.push(pill("Runtime", "badge-sys", "Shared parts that other apps need to run.")),
        Role::App => {}
    }
    v
}

pub fn gicon(p: &Package) -> gio::Icon {
    gicon_for(p.icon.as_deref(), p.role == Role::App)
}

/// Icon from a launcher's `Icon=` value (theme name or file path), with a generic fallback.
pub fn gicon_for(icon: Option<&str>, app: bool) -> gio::Icon {
    let fallback = if app { "application-x-executable" } else { "package-x-generic" };
    match icon {
        Some(i) if i.starts_with('/') && std::path::Path::new(i).exists() => {
            gio::FileIcon::new(&gio::File::for_path(i)).upcast()
        }
        Some(i) if !i.starts_with('/') => {
            // some launchers name the icon with its file extension
            let name = i
                .strip_suffix(".png")
                .or_else(|| i.strip_suffix(".svg"))
                .or_else(|| i.strip_suffix(".xpm"))
                .unwrap_or(i);
            // a multi-name ThemedIcon would let the current theme's fallback beat an app icon that
            // only exists in hicolor, so ask the theme directly
            let found = gtk::gdk::Display::default().is_some_and(|d| gtk::IconTheme::for_display(&d).has_icon(name));
            gio::ThemedIcon::new(if found { name } else { fallback }).upcast()
        }
        _ => gio::ThemedIcon::new(fallback).upcast(),
    }
}

/// A modal dialog with a status line and a progress bar, for work the user has to wait for.
pub struct Progress {
    dialog: adw::Dialog,
    status: gtk::Label,
    bar: gtk::ProgressBar,
    pulsing: Rc<Cell<bool>>,
    /// Set once the dialog closes, which stops the pulse timer.
    closed: Rc<Cell<bool>>,
}

impl Progress {
    pub fn show(parent: &impl IsA<gtk::Widget>, title: &str, hint: &str) -> Rc<Progress> {
        let dialog = adw::Dialog::builder().content_width(360).can_close(false).build();
        let bx = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(28)
            .margin_bottom(28)
            .margin_start(24)
            .margin_end(24)
            .build();
        let heading = label(title, &["title-4"]);
        heading.set_halign(gtk::Align::Center);
        bx.append(&heading);
        let status = wrapped("", &["dim-label"]);
        status.set_justify(gtk::Justification::Center);
        status.set_xalign(0.5);
        bx.append(&status);
        let bar = gtk::ProgressBar::new();
        bx.append(&bar);
        if !hint.is_empty() {
            let h = wrapped(hint, &["caption", "dim-label"]);
            h.set_justify(gtk::Justification::Center);
            h.set_xalign(0.5);
            bx.append(&h);
        }
        dialog.set_child(Some(&bx));
        dialog.present(Some(parent));
        let pulsing = Rc::new(Cell::new(true));
        let closed = Rc::new(Cell::new(false));
        let (b, p, c) = (bar.clone(), pulsing.clone(), closed.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            if c.get() {
                return glib::ControlFlow::Break;
            }
            if p.get() {
                b.pulse();
            }
            glib::ControlFlow::Continue
        });
        Rc::new(Progress { dialog, status, bar, pulsing, closed })
    }

    /// Update the status line (empty text keeps the old one) and the bar (`None` = unknown, pulse).
    pub fn set(&self, text: &str, fraction: Option<f64>) {
        if !text.is_empty() {
            self.status.set_text(text);
        }
        match fraction {
            Some(f) => {
                self.pulsing.set(false);
                self.bar.set_fraction(f.clamp(0.0, 1.0));
            }
            None => self.pulsing.set(true),
        }
    }

    /// Close once the dialog has finished opening (closing an opening dialog is ignored).
    pub async fn close(&self, opened: std::time::Instant) {
        if let Some(wait) = std::time::Duration::from_millis(400).checked_sub(opened.elapsed()) {
            glib::timeout_future(wait).await;
        }
        self.closed.set(true);
        self.dialog.force_close();
    }
}

pub fn size_text(n: u64) -> String {
    if n == 0 { String::new() } else { glib::format_size(n).to_string() }
}

/// A grid card. Children, in order: icon, title, summary, badge row.
pub fn card() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.add_css_class("card");
    b.add_css_class("pkg-card");
    b.set_size_request(210, 190);
    b.append(&gtk::Image::builder().pixel_size(64).halign(gtk::Align::Start).build());
    let title = label("", &["heading"]);
    title.set_ellipsize(pango::EllipsizeMode::End);
    b.append(&title);
    let summ = wrapped("", &["dim-label", "caption"]);
    summ.set_lines(2);
    summ.set_ellipsize(pango::EllipsizeMode::End);
    summ.set_valign(gtk::Align::Start);
    summ.set_vexpand(true);
    b.append(&summ);
    b.append(&gtk::Box::new(gtk::Orientation::Horizontal, 8));
    b
}

/// Fill a card for `p`; `others` are other copies of the same app, shown as extra source badges.
pub fn bind_card(card: &gtk::Widget, p: &Package, others: &[&Package]) {
    let img = card.first_child().and_downcast::<gtk::Image>().expect("icon");
    let title = img.next_sibling().and_downcast::<gtk::Label>().expect("title");
    let summ = title.next_sibling().and_downcast::<gtk::Label>().expect("summary");
    let badges = summ.next_sibling().and_downcast::<gtk::Box>().expect("badges");
    img.set_from_gicon(&gicon(p));
    title.set_text(&p.title);
    summ.set_text(&p.summary);
    while let Some(c) = badges.first_child() {
        badges.remove(&c);
    }
    badges.append(&kind_badge(p.kind));
    let mut shown = vec![p.kind];
    for o in others {
        if !shown.contains(&o.kind) {
            shown.push(o.kind);
            badges.append(&kind_badge(o.kind));
        }
    }
    if p.broken {
        badges.append(&broken_badge());
    }
    if p.foreign {
        badges.append(&aur_badge());
    }
    let size = size_text(p.size);
    // the size of one copy would be misleading next to several source badges
    if !size.is_empty() && others.is_empty() {
        badges.append(&label(&size, &["dim-label", "caption"]));
    }
}
