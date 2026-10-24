//! The main menu: keyboard shortcuts, privacy, and the About window with its legal notices.
//!
//! GPLv3 asks interactive programs to show "Appropriate Legal Notices" (copyright, that there is
//! no warranty, and where the licence is); the About window's Legal page does that.

use super::State;
use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;

const REPO: &str = "https://github.com/kimzam30/Nhako-Depot";

const PRIVACY: &str = "Depot has no accounts, no ads and no tracking, and it never sends anything about you \
or your computer anywhere.\n\nTo check for updates, it asks the software sources your computer already uses \
(through PackageKit, Snap and Flatpak), the same way the Software app does. Everything else Depot shows is \
read from your own computer and stays there.\n\nThe report file made by \"depot --list-only\" stays on your \
computer unless you choose to share it. It lists app names and the folders they're installed in, which can \
include your username.";

const TRADEMARKS: &str = "Debian, Ubuntu, Fedora, Arch Linux, openSUSE, Snap, Snapcraft, Flatpak, Flathub, \
GNOME and Linux are trademarks of their respective owners. Depot isn't affiliated with or endorsed by any of \
them; their names are used only to say which kinds of software Depot works with.";

const WARRANTY: &str = "Depot is free software: you can share and change it under the terms of the GNU General \
Public License, version 3 or (at your option) any later version.\n\nIt comes with absolutely no warranty. \
Depot always shows what an uninstall or update will change before it happens and uses your system's own \
package manager to do it, but you decide what to remove.";

const SHORTCUTS: &str = r#"<interface>
  <object class="GtkShortcutsWindow" id="shortcuts">
    <property name="modal">True</property>
    <child>
      <object class="GtkShortcutsSection">
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">General</property>
            <child><object class="GtkShortcutsShortcut"><property name="title">Search</property><property name="accelerator">&lt;Control&gt;f</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Refresh</property><property name="accelerator">F5</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Go back</property><property name="accelerator">Escape</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Keyboard shortcuts</property><property name="accelerator">&lt;Control&gt;question</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Quit</property><property name="accelerator">&lt;Control&gt;q</property></object></child>
          </object>
        </child>
        <child>
          <object class="GtkShortcutsGroup">
            <property name="title">Selecting apps</property>
            <child><object class="GtkShortcutsShortcut"><property name="title">Start selecting with this app</property><property name="accelerator">&lt;Control&gt;Pointer_Button1</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Select every app shown</property><property name="accelerator">&lt;Control&gt;a</property></object></child>
            <child><object class="GtkShortcutsShortcut"><property name="title">Stop selecting</property><property name="accelerator">Escape</property></object></child>
          </object>
        </child>
      </object>
    </child>
  </object>
</interface>"#;

/// The main menu's items.
pub fn menu() -> gio::Menu {
    let m = gio::Menu::new();
    let top = gio::Menu::new();
    top.append(Some("Select Apps"), Some("win.select"));
    m.append_section(None, &top);
    let help = gio::Menu::new();
    help.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    help.append(Some("Privacy"), Some("win.privacy"));
    help.append(Some("Report a Problem"), Some("win.report"));
    help.append(Some("About Depot"), Some("win.about"));
    m.append_section(None, &help);
    m
}

fn action(st: &Rc<State>, name: &str, f: impl Fn(&Rc<State>) + 'static) {
    let a = gio::SimpleAction::new(name, None);
    let st2 = Rc::downgrade(st);
    a.connect_activate(move |_, _| {
        if let Some(st) = st2.upgrade() {
            f(&st);
        }
    });
    st.win.add_action(&a);
}

/// Add the menu's actions to the window.
pub fn install(st: &Rc<State>) {
    action(st, "about", about);
    action(st, "privacy", privacy);
    action(st, "shortcuts", |st| {
        let b = gtk::Builder::from_string(SHORTCUTS);
        if let Some(w) = b.object::<gtk::ShortcutsWindow>("shortcuts") {
            w.set_transient_for(Some(&st.win));
            w.present();
        }
    });
    action(st, "report", |st| open(st, &format!("{REPO}/issues")));
    action(st, "select", |st| super::select::set_mode(st, true));
    action(st, "quit", |st| st.win.close());
    if let Some(app) = st.win.application() {
        app.set_accels_for_action("win.shortcuts", &["<Control>question"]);
        app.set_accels_for_action("win.quit", &["<Control>q"]);
    }
}

fn open(st: &State, url: &str) {
    let st_win = st.win.clone();
    gtk::UriLauncher::new(url).launch(Some(&st_win), None::<&gio::Cancellable>, |_| {});
}

fn privacy(st: &Rc<State>) {
    let d = adw::AlertDialog::builder().heading("Your privacy").body(PRIVACY).build();
    d.add_response("policy", "Read the full policy");
    d.add_response("close", "Close");
    d.set_default_response(Some("close"));
    d.set_close_response("close");
    let st2 = st.clone();
    d.connect_response(Some("policy"), move |_, _| open(&st2, &format!("{REPO}/blob/main/docs/PRIVACY.md")));
    d.present(Some(&st.win));
}

/// What the "Troubleshooting" page shows, for bug reports.
fn debug_info(st: &State) -> String {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string())))
        .unwrap_or_else(|| "unknown".into());
    let pkgs = st.packages.borrow();
    let mut kinds: Vec<String> = Vec::new();
    use depot_core::Kind;
    for k in [Kind::Deb, Kind::Rpm, Kind::Pacman, Kind::Snap, Kind::Flatpak, Kind::Manual] {
        let n = pkgs.iter().filter(|p| p.kind == k).count();
        if n > 0 {
            kinds.push(format!("{}: {n}", k.badge()));
        }
    }
    format!(
        "Depot {}\nSystem: {os}\nGTK {}.{}.{}, libadwaita {}.{}.{}\nInstalled: {}\n",
        env!("CARGO_PKG_VERSION"),
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
        kinds.join(", ")
    )
}

fn about(st: &Rc<State>) {
    let d = adw::AboutDialog::builder()
        .application_icon(crate::APP_ID)
        .application_name("Depot")
        .developer_name("Kim")
        .version(env!("CARGO_PKG_VERSION"))
        .comments("See every app on your Linux computer in one place. Keep them up to date, and remove the ones you don't need.")
        .website(REPO)
        .issue_url(format!("{REPO}/issues"))
        .support_url(format!("{REPO}/blob/main/docs/GUIDE.md"))
        .copyright("© 2026 Kim")
        .license_type(gtk::License::Gpl30)
        .developers(["Kim https://github.com/kimzam30"])
        .artists(["Kim (the penguin mascot)"])
        .debug_info(debug_info(st))
        .debug_info_filename("depot-debug-info.txt")
        .release_notes_version(env!("CARGO_PKG_VERSION"))
        .release_notes(
            "<p>This version makes every step easy to follow:</p>\
             <ul>\
             <li>Select several apps and uninstall them together</li>\
             <li>Tick exactly which updates to install</li>\
             <li>A progress window that says when your password was accepted and how it all went</li>\
             <li>Safer clean-up of apps installed by hand</li>\
             </ul>",
        )
        .build();
    d.add_legal_section("No warranty", Some("© 2026 Kim"), gtk::License::Gpl30, Some(WARRANTY));
    d.add_legal_section("Privacy", None, gtk::License::Custom, Some(PRIVACY));
    d.add_legal_section("Trademarks", None, gtk::License::Custom, Some(TRADEMARKS));
    d.add_legal_section(
        "Artwork",
        Some("© 2026 Kim"),
        gtk::License::Custom,
        Some("The penguin mascot and the app icon were made for Depot and are shared under the same licence as the program (GPL-3.0-or-later)."),
    );
    d.add_legal_section(
        "Rust libraries",
        None,
        gtk::License::Custom,
        Some(&format!(
            "Depot is built with these open-source libraries, each under the licence shown. MIT and Apache-2.0 licence texts: https://opensource.org/license/mit and https://www.apache.org/licenses/LICENSE-2.0\n\n{}",
            include_str!("../../data/third-party.txt")
        )),
    );
    d.add_legal_section(
        "GTK and libadwaita",
        None,
        gtk::License::Lgpl21,
        Some("Depot's window is drawn with GTK and libadwaita, which are shared under the GNU Lesser General Public License, version 2.1 or later."),
    );
    d.present(Some(&st.win));
}
