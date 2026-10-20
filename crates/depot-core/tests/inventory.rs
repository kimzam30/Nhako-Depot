//! End-to-end inventory tests against a fake filesystem.

use depot_core::{Env, Kind, Package, Role, scan};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct Fake {
    root: PathBuf,
}

impl Fake {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("depot-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Fake { root }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.p(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }

    fn exe(&self, rel: &str) -> PathBuf {
        let p = self.write(rel, "#!/bin/sh\n");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn deb(&self, name: &str, files: &[&Path]) {
        let list: String = files.iter().map(|f| format!("{}\n", f.display())).collect();
        self.write(&format!("var/lib/dpkg/info/{name}.list"), &list);
        let status = self.p("var/lib/dpkg/status");
        let mut s = fs::read_to_string(&status).unwrap_or_default();
        s.push_str(&format!(
            "Package: {name}\nStatus: install ok installed\nArchitecture: all\nVersion: 1.0\nInstalled-Size: 10\nDescription: {name} package\n\n"
        ));
        self.write("var/lib/dpkg/status", &s);
    }

    fn launcher(&self, rel: &str, body: &str) -> PathBuf {
        self.write(rel, &format!("[Desktop Entry]\nType=Application\n{body}\n"))
    }

    fn env(&self) -> Env {
        Env {
            home: self.p("home"),
            app_dirs: vec![
                self.p("home/.local/share/applications"),
                self.p("usr/share/applications"),
                self.p("var/lib/snapd/desktop/applications"),
            ],
            path: vec![self.p("usr/bin"), self.p("home/.local/bin")],
            desktops: vec!["GNOME".into()],
            dpkg_dir: self.p("var/lib/dpkg"),
            apt_states: self.p("var/lib/apt/extended_states"),
            snap_socket: None,
            snap_desktop_dir: self.p("var/lib/snapd/desktop/applications"),
            flatpak: false,
            flatpak_share: vec![],
            cache_dir: Some(self.p("home/.cache/depot")),
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn find<'a>(pkgs: &'a [Package], title: &str) -> Vec<&'a Package> {
    pkgs.iter().filter(|p| p.title == title).collect()
}

/// The Nuvio .deb ships nuvio.desktop and its postinst copies a second launcher in with
/// xdg-desktop-menu. Both must count as the one deb, not a deb plus a manual install.
#[test]
fn launcher_copied_by_install_script_belongs_to_the_deb() {
    let f = Fake::new("nuvio");
    let bin = f.exe("opt/nuvio/bin/Nuvio");
    let own = f.launcher("usr/share/applications/nuvio.desktop", &format!("Name=Nuvio\nExec={} %u", bin.display()));
    f.launcher(
        "usr/share/applications/nuvio-Nuvio.desktop",
        &format!("Name=Nuvio\nComment=Nuvio\nExec={}", bin.display()),
    );
    f.deb("nuvio", &[&bin, &own]);

    let inv = scan(&f.env());
    let hits = find(&inv.packages, "Nuvio");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].kind, Kind::Deb);
    assert_eq!(hits[0].role, Role::App);
    assert_eq!(hits[0].desktop.as_deref(), Some(own.as_path()), "the package's own launcher is the main one");
    assert_eq!(hits[0].extra_launchers.len(), 1);

    // second run is served from the owner cache and must agree
    let again = scan(&f.env());
    assert_eq!(find(&again.packages, "Nuvio").len(), 1);
}

/// Spotify's deb keeps its launcher outside the applications folder and copies it in.
#[test]
fn launcher_owned_through_its_program() {
    let f = Fake::new("spotify");
    let real = f.exe("usr/share/spotify/spotify");
    let link = f.p("usr/bin/spotify");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    f.launcher("usr/share/applications/spotify.desktop", "Name=Spotify\nExec=spotify %U");
    f.deb("spotify-client", &[&link, &real]);

    let inv = scan(&f.env());
    let hits = find(&inv.packages, "Spotify");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].kind, Kind::Deb);
    assert_eq!(hits[0].id, "spotify-client");
    assert_eq!(hits[0].role, Role::App);
}

#[test]
fn hidden_user_override_hides_system_launcher() {
    let f = Fake::new("hidden");
    let bin = f.exe("opt/thing/thing");
    f.launcher("usr/share/applications/thing.desktop", &format!("Name=Thing\nExec={}", bin.display()));
    f.launcher("home/.local/share/applications/thing.desktop", "Name=Thing\nHidden=true");
    let inv = scan(&f.env());
    assert!(find(&inv.packages, "Thing").is_empty());
}

#[test]
fn user_override_of_package_launcher_stays_with_package() {
    let f = Fake::new("override");
    let sys = f.launcher("usr/share/applications/steam.desktop", "Name=Steam\nExec=steam");
    let bin = f.exe("usr/bin/steam");
    f.deb("steam-installer", &[&sys, &bin]);
    f.launcher("home/.local/share/applications/steam.desktop", "Name=Steam\nExec=steam --silent");
    let inv = scan(&f.env());
    let hits = find(&inv.packages, "Steam");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, Kind::Deb);
}

#[test]
fn interpreter_does_not_make_python_an_app() {
    let f = Fake::new("interp");
    let py = f.exe("usr/bin/python3");
    f.deb("python3", &[&py]);
    let script = f.exe("home/scripts/tool.py");
    f.launcher(
        "home/.local/share/applications/tool.desktop",
        &format!("Name=My Tool\nExec=python3 {}", script.display()),
    );
    let inv = scan(&f.env());
    let py = inv.packages.iter().find(|p| p.id == "python3").unwrap();
    assert_eq!(py.role, Role::Tool);
    assert_eq!(find(&inv.packages, "My Tool")[0].kind, Kind::Manual);
}

#[test]
fn web_apps_and_broken_launchers() {
    let f = Fake::new("web");
    let browser = f.exe("usr/bin/brave");
    f.launcher(
        "home/.local/share/applications/brave-abc.desktop",
        &format!("Name=Teams\nExec={} --app-id=abc", browser.display()),
    );
    f.launcher(
        "home/.local/share/applications/brave-gone.desktop",
        "Name=Old\nExec=/opt/brave.com/brave/brave --app-id=x",
    );
    f.launcher("home/.local/share/applications/steam.desktop", "Name=Steam\nExec=/usr/games/steam %U");
    f.launcher("home/.local/share/applications/dev.kimzam.Depot.desktop", "Name=Depot\nExec=depot");
    let inv = scan(&f.env());
    let teams = find(&inv.packages, "Teams")[0];
    assert!(teams.blocked.is_some() && !teams.broken);
    let old = find(&inv.packages, "Old")[0];
    assert!(old.broken && old.blocked.is_none(), "a broken web app is just a leftover launcher");
    let steam = find(&inv.packages, "Steam")[0];
    assert!(steam.broken);
    assert_eq!(steam.manual.as_ref().unwrap().files.len(), 1);
    assert!(find(&inv.packages, "Depot").is_empty(), "Depot doesn't list itself");
}

#[test]
fn manual_plan_never_takes_shared_or_lookalike_files() {
    let f = Fake::new("manual");
    let prog = f.exe("home/.local/opt/foo/bin/foo");
    f.exe("home/.local/opt/foobar/bin/foobar");
    f.write("home/.local/bin/foo", &format!("#!/bin/sh\nexec {} \"$@\"\n", prog.display()));
    f.write(
        "home/.local/bin/foobar",
        &format!("#!/bin/sh\nexec {}/../foobar/bin/foobar\n", f.p("home/.local/opt").display()),
    );
    f.write("home/.local/bin/other", "#!/bin/sh\nexec /home/x/.local/opt/foobar/bin/foobar\n");
    f.write("home/.local/share/icons/hicolor/48x48/apps/foo.png", "png");
    f.write("home/.local/share/icons/hicolor/48x48/apps/foo-extra.png", "png");
    f.write("home/.local/share/icons/hicolor/48x48/apps/shared.png", "png");
    f.launcher("home/.local/share/applications/foo.desktop", &format!("Name=Foo\nExec={}\nIcon=foo", prog.display()));
    f.launcher(
        "home/.local/share/applications/bar.desktop",
        &format!("Name=Bar\nExec={}\nIcon=shared", prog.display()),
    );
    f.launcher("home/.local/share/applications/baz.desktop", "Name=Baz\nExec=foo\nIcon=shared\nNoDisplay=true");
    f.write("home/.config/foo/settings", "x");

    let inv = scan(&f.env());
    let foo = find(&inv.packages, "Foo")[0];
    let plan = foo.manual.as_ref().unwrap();
    assert_eq!(plan.appdir.as_deref(), Some(f.p("home/.local/opt/foo").as_path()));
    let names: Vec<String> =
        plan.files.iter().map(|p| p.strip_prefix(&f.root).unwrap().display().to_string()).collect();
    assert!(names.contains(&"home/.local/bin/foo".to_string()), "{names:?}");
    assert!(names.contains(&"home/.local/share/icons/hicolor/48x48/apps/foo.png".to_string()), "{names:?}");
    assert!(
        !names.iter().any(|n| n.contains("foobar") || n.contains("foo-extra") || n.ends_with("bin/other")),
        "{names:?}"
    );
    assert_eq!(plan.data, vec![f.p("home/.config/foo")]);

    let bar = find(&inv.packages, "Bar")[0];
    assert!(
        !bar.manual.as_ref().unwrap().files.iter().any(|p| p.ends_with("shared.png")),
        "icon used by two launchers is kept"
    );
}

#[test]
fn missing_package_managers_are_not_errors() {
    let f = Fake::new("empty");
    let inv = scan(&f.env());
    assert!(inv.packages.is_empty());
    assert!(inv.warnings.is_empty());
    assert!(!inv.system_packages, "no dpkg database means the distro isn't supported yet");
    f.deb("x", &[]);
    assert!(scan(&f.env()).system_packages);
}

/// A folder that only groups several apps is never planned for removal as one app's folder.
#[test]
fn shared_folder_is_not_one_apps_folder() {
    let f = Fake::new("shared");
    let a = f.exe("home/Applications/tools/a/a");
    let b = f.exe("home/Applications/tools/b/b");
    f.launcher("home/.local/share/applications/a.desktop", &format!("Name=A\nExec={}", a.display()));
    f.launcher("home/.local/share/applications/b.desktop", &format!("Name=B\nExec={}", b.display()));
    // one app with a helper in its own lib/ folder is still one app
    let foo = f.exe("home/.local/opt/foo/bin/foo");
    let helper = f.exe("home/.local/opt/foo/lib/helper");
    f.launcher("home/.local/share/applications/foo.desktop", &format!("Name=Foo\nExec={}", foo.display()));
    f.launcher("home/.local/share/applications/foo-helper.desktop", &format!("Name=Helper\nExec={}", helper.display()));

    let inv = scan(&f.env());
    let dir = |title: &str| find(&inv.packages, title)[0].manual.as_ref().unwrap().appdir.clone();
    assert_eq!(dir("A"), Some(f.p("home/Applications/tools/a")));
    assert_eq!(dir("B"), Some(f.p("home/Applications/tools/b")));
    assert_eq!(dir("Foo"), Some(f.p("home/.local/opt/foo")));
    assert_eq!(dir("Helper"), Some(f.p("home/.local/opt/foo")));
}

/// Settings folders are only offered when they carry the app's own name, and never shared ones.
#[test]
fn data_folders_need_the_apps_own_name() {
    let f = Fake::new("data");
    let icons = f.exe("home/.local/opt/icons/bin/icons");
    f.launcher("home/.local/share/applications/icons.desktop", &format!("Name=Icons\nExec={}", icons.display()));
    let tool = f.exe("home/.local/opt/tool-2.0/tool");
    f.launcher("home/.local/share/applications/fancy.desktop", &format!("Name=Fancy\nExec={}", tool.display()));
    f.write("home/.local/share/icons/hicolor/index.theme", "x");
    f.write("home/.config/fancy/settings", "x");

    let inv = scan(&f.env());
    let data = |title: &str| find(&inv.packages, title)[0].manual.as_ref().unwrap().data.clone();
    assert!(data("Icons").is_empty(), "the shared icons folder is never an app's data");
    assert!(data("Fancy").is_empty(), "a launcher name that isn't the app's own name doesn't pick folders");
}

#[test]
fn launcher_with_missing_tryexec_is_skipped() {
    let f = Fake::new("tryexec");
    let bin = f.exe("usr/bin/real");
    f.launcher("usr/share/applications/gone.desktop", "Name=Gone\nTryExec=not-installed\nExec=not-installed");
    f.launcher("usr/share/applications/here.desktop", &format!("Name=Here\nTryExec=real\nExec={}", bin.display()));
    let inv = scan(&f.env());
    assert!(find(&inv.packages, "Gone").is_empty(), "the app menu hides it, so Depot does too");
    assert_eq!(find(&inv.packages, "Here").len(), 1);
}
