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

/// Badges shown next to the source badge.
pub fn extra_badges(p: &Package) -> Vec<gtk::Label> {
    let mut v = Vec::new();
    if p.broken {
        v.push(pill("Broken", "badge-broken", "The program this launcher starts is no longer installed."));
    }
    match p.role {
        Role::Tool if p.kind == Kind::Deb => v.push(pill("Library / tool", "badge-sys", "")),
        Role::Tool => v.push(pill("No launcher", "badge-sys", "Installed, but it has no entry in the app menu.")),
        Role::Runtime => v.push(pill("Runtime", "badge-sys", "Shared parts that other apps need to run.")),
        Role::App => {}
    }
    v
}

pub fn gicon(p: &Package) -> gio::Icon {
    let fallback = if p.role == Role::App { "application-x-executable" } else { "package-x-generic" };
    match p.icon.as_deref() {
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

pub fn bind_card(card: &gtk::Widget, p: &Package) {
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
    if p.broken {
        badges.append(&extra_badges(p)[0]);
    }
    let size = size_text(p.size);
    if !size.is_empty() {
        badges.append(&label(&size, &["dim-label", "caption"]));
    }
}
