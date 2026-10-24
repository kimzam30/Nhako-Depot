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

    /// A file carrying the AppImage mark, ready to run.
    fn appimage(&self, rel: &str) -> PathBuf {
        let p = self.p(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"\x7fELF\x02\x01\x01\x00AI\x02 squashfs follows").unwrap();
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

    /// A pacman package in the local database. Paths are absolute fixture paths, stored the way
    /// pacman does: without the leading slash.
    fn pacman(&self, name: &str, files: &[&Path], explicit: bool) {
        let dir = format!("var/lib/pacman/local/{name}-1.0-1");
        let reason = if explicit { "" } else { "%REASON%\n1\n" };
        self.write(
            &format!("{dir}/desc"),
            &format!("%NAME%\n{name}\n\n%VERSION%\n1.0-1\n\n%DESC%\n{name} package\n\n%ARCH%\nx86_64\n\n{reason}"),
        );
        let list: String =
            files.iter().map(|f| format!("{}\n", f.display().to_string().trim_start_matches('/'))).collect();
        self.write(&format!("{dir}/files"), &format!("%FILES%\n{list}"));
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
            pacman_dir: self.p("var/lib/pacman"),
            pacman_conf: self.p("etc/pacman.conf"),
            pacman_foreign: false,
            rpm_db: None,
            snap_socket: None,
            snap_desktop_dir: self.p("var/lib/snapd/desktop/applications"),
            flatpak: false,
            flatpak_share: vec![],
            cache_dir: Some(self.p("home/.cache/depot")),
            os_like: vec![],
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

/// On Arch, launchers belong to the pacman package that owns them or their program.
#[test]
fn pacman_packages_own_their_launchers() {
    let f = Fake::new("pacman");
    let bin = f.exe("usr/lib/firefox/firefox");
    let own = f.launcher("usr/share/applications/firefox.desktop", &format!("Name=Firefox\nExec={} %u", bin.display()));
    f.pacman("firefox", &[&bin, &own], true);
    let tool = f.exe("usr/bin/htop");
    f.launcher("usr/share/applications/htop.desktop", &format!("Name=Htop\nExec={}", tool.display()));
    f.pacman("htop", &[&tool], false);
    f.pacman("glibc", &[], false);
    f.write("etc/pacman.conf", "[options]\nHoldPkg = pacman glibc\n");

    let inv = scan(&f.env());
    assert!(inv.system_packages && inv.warnings.is_empty(), "{:?}", inv.warnings);
    let ff = find(&inv.packages, "Firefox");
    assert_eq!(ff.len(), 1, "{ff:#?}");
    assert_eq!((ff[0].kind, ff[0].role), (Kind::Pacman, Role::App));
    assert!(ff[0].user_installed);
    let htop = find(&inv.packages, "Htop")[0];
    assert_eq!(htop.kind, Kind::Pacman, "owned through its program, not a manual install");
    assert!(!htop.user_installed);
    let glibc = inv.packages.iter().find(|p| p.id == "glibc").unwrap();
    assert!(glibc.blocked.is_some(), "HoldPkg packages can't be removed");
}

/// A stray dpkg database on Arch must not hide every pacman package.
#[test]
fn distro_family_picks_the_package_database() {
    let f = Fake::new("family");
    let bin = f.exe("usr/lib/firefox/firefox");
    let own = f.launcher("usr/share/applications/firefox.desktop", &format!("Name=Firefox\nExec={}", bin.display()));
    f.pacman("firefox", &[&bin, &own], true);
    f.deb("dpkg", &[]);
    let mut env = f.env();
    env.os_like = vec!["arch".into()];
    let inv = scan(&env);
    assert_eq!(find(&inv.packages, "Firefox")[0].kind, Kind::Pacman);
    assert!(inv.packages.iter().all(|p| p.kind != Kind::Deb));
}

/// A program in ~/.cargo/bin is installed, even when Depot was started with a short PATH.
#[test]
fn home_program_folders_count_as_installed() {
    let f = Fake::new("homebin");
    f.exe("home/.cargo/bin/mytool");
    f.launcher("home/.local/share/applications/mytool.desktop", "Name=My Tool\nExec=mytool");
    let mut env = f.env();
    env.path = depot_core::exec::search_path(&f.p("home"));
    let inv = scan(&env);
    let t = find(&inv.packages, "My Tool");
    assert_eq!(t.len(), 1);
    assert!(!t[0].broken, "its program is in ~/.cargo/bin");
}

/// An AppImage in ~/Applications with a launcher: one AppImage card whose removal takes the file
/// too, not just the launcher, and no second card for the file itself.
#[test]
fn appimage_with_a_launcher() {
    let f = Fake::new("appimage-launcher");
    let file = f.appimage("home/Applications/Foo-1.2-x86_64.AppImage");
    let link = f.p("home/.local/bin/foo");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let own = f.launcher(
        "home/.local/share/applications/foo.desktop",
        &format!("Name=Foo Editor\nExec={} %F", file.display()),
    );
    let settings = f.p("home/.config/foo");
    fs::create_dir_all(&settings).unwrap();

    let inv = scan(&f.env());
    let all: Vec<&Package> = inv.packages.iter().filter(|p| p.kind == Kind::AppImage).collect();
    assert_eq!(all.len(), 1, "{all:#?}");
    let p = all[0];
    assert_eq!(p.title, "Foo Editor");
    assert_eq!(p.version, "1.2");
    assert!(p.blocked.is_none() && p.removes_program());
    let plan = p.manual.as_ref().unwrap();
    for want in [&own, &file, &link] {
        assert!(plan.files.contains(want), "{want:?} missing from {:#?}", plan.files);
    }
    assert_eq!(plan.data, vec![settings]);
}

/// AppImages nobody made a launcher for still show up, once each.
#[test]
fn loose_appimages() {
    let f = Fake::new("appimage-loose");
    f.appimage("home/Downloads/Bar.AppImage");
    f.appimage("home/AppImages/tools/Qux-3.0.AppImage");
    f.write("home/Downloads/notes.txt", "hello");
    // a plain program named like an AppImage isn't one
    f.write("home/Downloads/Fake.AppImage", "\x7fELF no mark here");

    let inv = scan(&f.env());
    let names: Vec<(&str, &str)> = inv
        .packages
        .iter()
        .filter(|p| p.kind == Kind::AppImage)
        .map(|p| (p.title.as_str(), p.version.as_str()))
        .collect();
    assert_eq!(names, vec![("Bar", ""), ("Qux", "3.0")]);
    let bar = &find(&inv.packages, "Bar")[0];
    assert_eq!(bar.role, Role::App);
    assert!(bar.desktop.is_none());
    assert_eq!(bar.manual.as_ref().unwrap().files, vec![f.p("home/Downloads/Bar.AppImage")]);
}

/// `appimage-run Foo.AppImage` (NixOS and some app stores) starts the AppImage.
#[test]
fn appimage_started_through_appimage_run() {
    let f = Fake::new("appimage-run");
    f.exe("usr/bin/appimage-run");
    let file = f.appimage("home/Applications/Baz.AppImage");
    f.launcher(
        "home/.local/share/applications/baz.desktop",
        &format!("Name=Baz\nExec=appimage-run {}", file.display()),
    );

    let inv = scan(&f.env());
    let hits = find(&inv.packages, "Baz");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].kind, Kind::AppImage);
    assert!(hits[0].manual.as_ref().unwrap().files.contains(&file));
}

/// A hand-installed program that isn't an AppImage stays a manual install.
#[test]
fn plain_program_is_still_manual() {
    let f = Fake::new("appimage-not");
    let bin = f.exe("home/.local/opt/tool/tool");
    f.launcher("home/.local/share/applications/tool.desktop", &format!("Name=Tool\nExec={}", bin.display()));

    let inv = scan(&f.env());
    assert_eq!(find(&inv.packages, "Tool")[0].kind, Kind::Manual);
    assert!(inv.packages.iter().all(|p| p.kind != Kind::AppImage));
}
