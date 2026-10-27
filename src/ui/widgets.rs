//! Small building blocks shared by the grid and the detail page.

use adw::prelude::*;
use depot_core::{Kind, Package, Role};
use gtk::{gio, glib, pango};

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

/// Tell screen readers about a change they wouldn't otherwise hear about. GTK 4.14 crashes when
/// asked to announce without a screen-reader connection (`GTK_A11Y=none`, or no accessibility
/// bus), so nothing is said then: there's nobody to hear it.
pub fn announce(w: &impl IsA<gtk::Widget>, text: &str) {
    let w = w.upcast_ref::<gtk::Widget>();
    if w.at_context().type_().name() == "GtkAtSpiContext" {
        w.announce(text.trim_end_matches(['.', ' ']), gtk::AccessibleAnnouncementPriority::Medium);
    }
}

/// Whether the desktop's screen reader is on, from GNOME's setting (other desktops: assumed off).
pub fn screen_reader_on() -> bool {
    const SCHEMA: &str = "org.gnome.desktop.a11y.applications";
    gio::SettingsSchemaSource::default().and_then(|src| src.lookup(SCHEMA, true)).is_some_and(|s| {
        s.has_key("screen-reader-enabled") && gio::Settings::new(SCHEMA).boolean("screen-reader-enabled")
    })
}

/// A tick box whose label wraps instead of widening its dialog.
pub fn check(text: &str) -> gtk::CheckButton {
    let c = gtk::CheckButton::new();
    let l = wrapped(text, &[]);
    c.set_child(Some(&l));
    c
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

pub fn size_text(n: u64) -> String {
    if n == 0 { String::new() } else { glib::format_size(n).to_string() }
}

/// A grid card: a box (icon, title, summary, badge row) with a tick box over its top-right
/// corner for selection mode.
pub fn card() -> gtk::Overlay {
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
    let o = gtk::Overlay::builder().child(&b).build();
    let tick = gtk::CheckButton::builder()
        .halign(gtk::Align::End)
        .valign(gtk::Align::Start)
        .margin_top(10)
        .margin_end(10)
        .visible(false)
        // clicks go to the card, which toggles the selection
        .can_target(false)
        .can_focus(false)
        .build();
    tick.add_css_class("card-tick");
    // the card's grid cell is what's announced, marked selected by `select::Selection`
    tick.set_accessible_role(gtk::AccessibleRole::Presentation);
    o.add_overlay(&tick);
    o
}

/// The parts of a card made by [`card`].
pub fn card_parts(card: &gtk::Overlay) -> (gtk::Box, gtk::CheckButton) {
    let b = card.child().and_downcast::<gtk::Box>().expect("card box");
    let tick = b.next_sibling().and_downcast::<gtk::CheckButton>().expect("tick");
    (b, tick)
}

/// What a screen reader says for a card: "GIMP, Flatpak and Deb, 1.2 GB".
pub fn card_label(p: &Package, others: &[&Package]) -> String {
    let mut kinds = vec![p.kind.badge()];
    for o in others {
        if !kinds.contains(&o.kind.badge()) {
            kinds.push(o.kind.badge());
        }
    }
    let mut parts = vec![p.title.clone(), kinds.join(" and ")];
    let size = size_text(p.size);
    if !size.is_empty() && others.is_empty() {
        parts.push(size);
    }
    if p.broken {
        parts.push("broken launcher".into());
    }
    if p.foreign {
        parts.push("from the AUR".into());
    }
    parts.join(", ")
}

/// Fill a card for `p`; `others` are other copies of the same app, shown as extra source badges.
pub fn bind_card(card: &gtk::Overlay, p: &Package, others: &[&Package]) {
    let (card, _) = card_parts(card);
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
