//! Builds the full list of installed software and decides which launcher belongs to which package.

use crate::backend::{appimage, dpkg, flatpak, manual, pacman, rpm, snap};
use crate::exec::{self, Target};
use crate::model::{Kind, Package, Role};
use crate::xdg::{self, Launcher};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const SELF_ID: &str = "dev.kimzam.Depot.desktop";

/// Where to look for everything. [`Env::system`] for the real machine; tests point it at fixtures.
#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub app_dirs: Vec<PathBuf>,
    pub path: Vec<PathBuf>,
    pub desktops: Vec<String>,
    pub dpkg_dir: PathBuf,
    pub apt_states: PathBuf,
    pub pacman_dir: PathBuf,
    pub pacman_conf: PathBuf,
    /// Ask the `pacman` command which packages are from the AUR (off in tests).
    pub pacman_foreign: bool,
    /// The rpm database folder, read through the `rpm` command.
    pub rpm_db: Option<PathBuf>,
    pub snap_socket: Option<PathBuf>,
    pub snap_desktop_dir: PathBuf,
    pub flatpak: bool,
    pub flatpak_share: Vec<PathBuf>,
    pub cache_dir: Option<PathBuf>,
    /// The distro family from `/etc/os-release` (`ID` and `ID_LIKE`), which decides between
    /// package databases when more than one is present.
    pub os_like: Vec<String>,
}

impl Env {
    pub fn system() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cache"));
        Env {
            app_dirs: xdg::app_dirs(&home),
            path: exec::search_path(&home),
            desktops: std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            dpkg_dir: "/var/lib/dpkg".into(),
            apt_states: "/var/lib/apt/extended_states".into(),
            pacman_dir: "/var/lib/pacman".into(),
            pacman_conf: "/etc/pacman.conf".into(),
            pacman_foreign: true,
            rpm_db: ["/var/lib/rpm", "/usr/lib/sysimage/rpm"].into_iter().map(PathBuf::from).find(|d| d.is_dir()),
            snap_socket: Some(snap::SOCKET.into()),
            snap_desktop_dir: snap::DESKTOP_DIR.into(),
            flatpak: true,
            flatpak_share: vec![
                home.join(".local/share/flatpak/exports/share"),
                "/var/lib/flatpak/exports/share".into(),
            ],
            cache_dir: Some(cache.join("depot")),
            os_like: std::fs::read_to_string("/etc/os-release").map(|t| parse_os_like(&t)).unwrap_or_default(),
            home,
        }
    }
}

/// `ID` and `ID_LIKE` from os-release, as a list of lowercase words.
pub fn parse_os_like(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("ID=").or_else(|| l.strip_prefix("ID_LIKE=")))
        .flat_map(|v| v.trim_matches(['"', '\'']).split_whitespace().map(str::to_lowercase).collect::<Vec<_>>())
        .collect()
}

/// The distro's own package manager, if Depot can read it. Only one is used, so a stray
/// `rpm` on Ubuntu or `dpkg` on Arch doesn't count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    Dpkg,
    Pacman,
    Rpm,
}

impl Native {
    fn detect(env: &Env) -> Option<Native> {
        let present: Vec<Native> = [
            (Native::Dpkg, env.dpkg_dir.join("status").exists()),
            (Native::Pacman, env.pacman_dir.join("local").is_dir()),
            (Native::Rpm, env.rpm_db.as_deref().is_some_and(Path::is_dir)),
        ]
        .into_iter()
        .filter_map(|(n, here)| here.then_some(n))
        .collect();
        // when several are installed, the one the distro family uses wins
        present
            .iter()
            .copied()
            .find(|n| n.families().iter().any(|f| env.os_like.iter().any(|o| o == f)))
            .or_else(|| present.first().copied())
    }

    fn families(self) -> &'static [&'static str] {
        match self {
            Native::Dpkg => &["debian", "ubuntu"],
            Native::Pacman => &["arch", "archlinux"],
            Native::Rpm => &["fedora", "rhel", "centos", "suse", "opensuse", "mageia"],
        }
    }

    fn load(self, env: &Env) -> Result<Vec<Package>, String> {
        match self {
            Native::Dpkg => dpkg::load(&env.dpkg_dir, &env.apt_states),
            Native::Pacman => pacman::load(&env.pacman_dir, &env.pacman_conf, env.pacman_foreign),
            Native::Rpm => env.rpm_db.as_deref().map_or(Ok(Vec::new()), rpm::load),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Native::Dpkg => "Debian packages",
            Native::Pacman => "Pacman packages",
            Native::Rpm => "RPM packages",
        }
    }

    fn owners(self, env: &Env, wanted: &HashSet<PathBuf>) -> (HashMap<PathBuf, String>, Option<String>) {
        let cache = |name: &str| env.cache_dir.as_ref().map(|d| d.join(name));
        match self {
            Native::Dpkg => dpkg::owners(&env.dpkg_dir, cache("dpkg-owners").as_deref(), wanted),
            Native::Pacman => pacman::owners(&env.pacman_dir, cache("pacman-owners").as_deref(), wanted),
            Native::Rpm => env
                .rpm_db
                .as_deref()
                .map(|db| rpm::owners(db, cache("rpm-owners").as_deref(), wanted))
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Inventory {
    pub packages: Vec<Package>,
    /// Sources that couldn't be read, in plain language.
    pub warnings: Vec<String>,
    /// The system's own package database was found (dpkg, pacman or rpm). When it wasn't,
    /// system apps can't be told apart from manual installs.
    pub system_packages: bool,
    /// Apps installed more than one way, as indices into `packages`, best copy first.
    pub duplicates: Vec<Vec<usize>>,
    /// Flatpak runtimes no app needs any more, as indices into `packages` (see [`flatpak::unused`]).
    pub unused_runtimes: Vec<usize>,
}

/// How a launcher was matched to its package.
enum Claim {
    Snap(String),
    Flatpak(String),
    /// Paths to look up in the package database, strongest evidence first.
    /// The flag marks paths that come from the launcher's command rather than the launcher file itself.
    /// The last flag is set when the launcher's program doesn't exist.
    Files(Vec<(PathBuf, bool)>, Option<PathBuf>, bool),
    WebApp(bool),
}

fn web_app(argv: &[String]) -> bool {
    argv.iter().any(|a| a.starts_with("--app-id=") || a.starts_with("--app="))
}

fn claim(l: &Launcher, env: &Env) -> Claim {
    let path = &l.entry.path;
    if path.starts_with(&env.snap_desktop_dir) {
        let id = l.entry.id.strip_suffix(".desktop").unwrap_or(&l.entry.id);
        return Claim::Snap(id.split('_').next().unwrap_or(id).to_string());
    }
    if flatpak::is_export(path) {
        return Claim::Flatpak(l.entry.id.strip_suffix(".desktop").unwrap_or(&l.entry.id).to_string());
    }
    let argv = l.entry.argv();
    let target = exec::target(&argv, &env.path);
    let missing = !matches!(
        target,
        Some(Target::Program { path: Some(_), .. }) | Some(Target::Snap(_)) | Some(Target::Flatpak(_))
    );
    if web_app(&argv) {
        return Claim::WebApp(missing);
    }
    let mut files: Vec<(PathBuf, bool)> =
        std::iter::once(path).chain(&l.shadowed).map(|p| (p.clone(), false)).collect();
    let mut program = None;
    match target {
        Some(Target::Snap(s)) => return Claim::Snap(s),
        Some(Target::Flatpak(f)) => return Claim::Flatpak(f),
        Some(Target::Program { path, generic, script }) => {
            if let (Some(p), false) = (&path, generic) {
                files.extend(exec::path_variants(p).into_iter().map(|v| (v, true)));
            }
            // `appimage-run Foo.AppImage`: the AppImage is the app
            let run_appimage = generic && script.as_deref().is_some_and(appimage::is_appimage);
            if let Some(s) = &script {
                files.extend(exec::path_variants(s).into_iter().map(|v| (v, true)));
            }
            program = if run_appimage {
                script
            } else if generic {
                None
            } else {
                path
            };
        }
        None => {}
    }
    Claim::Files(files, program, missing)
}

/// Whether launcher id `id` is an extra launcher of the Flatpak app `app` (`org.x.App.Tool` is,
/// `org.x.AppTwo` isn't).
fn flatpak_child(id: &str, app: &str) -> bool {
    id.strip_prefix(app).is_some_and(|r| r.starts_with('.'))
}

fn attach(p: &mut Package, l: &Launcher) {
    let e = &l.entry;
    if p.desktop.is_some() {
        if p.desktop.as_ref() != Some(&e.path) && !p.extra_launchers.contains(&e.path) {
            p.extra_launchers.push(e.path.clone());
        }
        return;
    }
    if p.role != Role::Runtime {
        p.role = Role::App;
    }
    if !e.name.is_empty() {
        p.title = e.name.clone();
    }
    if !e.icon.is_empty() {
        p.icon = Some(e.icon.clone());
    }
    if p.summary.is_empty() {
        p.summary = if e.comment.is_empty() { e.generic_name.clone() } else { e.comment.clone() };
    }
    p.desktop = Some(e.path.clone());
}

fn manual_package(
    l: &Launcher,
    program: Option<&Path>,
    env: &Env,
    icon_users: &HashMap<String, usize>,
    programs: &[PathBuf],
    web: bool,
    broken: bool,
) -> Package {
    let e = &l.entry;
    let base = e.id.strip_suffix(".desktop").unwrap_or(&e.id);
    let mut p = Package::new(Kind::Manual, base);
    p.role = Role::App;
    p.user_installed = true;
    p.title = if e.name.is_empty() { base.to_string() } else { e.name.clone() };
    p.summary = if e.comment.is_empty() { e.generic_name.clone() } else { e.comment.clone() };
    p.icon = (!e.icon.is_empty()).then(|| e.icon.clone());
    p.desktop = Some(e.path.clone());
    p.broken = broken;
    if broken {
        p.summary = "The program this launcher starts is no longer installed.".into();
        p.location = if e.exec.is_empty() { "(no command)".into() } else { e.exec.clone() };
        p.manual = Some(crate::model::ManualPlan { files: vec![e.path.clone()], ..Default::default() });
        if !e.path.starts_with(&env.home) {
            p.blocked =
                Some("This leftover launcher is in a system folder, so removing it needs an administrator.".into());
        }
        return p;
    }
    if web {
        p.location = "Web app".into();
        p.blocked = Some("This is a web app made by your browser. Remove it from the browser's own apps page.".into());
        return p;
    }
    let plan = manual::plan(e, program, &env.home, icon_users, programs);
    p.location = match (&plan.appdir, program) {
        (Some(d), _) => d.to_string_lossy().into_owned(),
        (None, Some(prog)) => prog.to_string_lossy().into_owned(),
        (None, None) => e.exec.clone(),
    };
    if !e.path.starts_with(&env.home) {
        p.blocked = Some(
            "This launcher is in a system folder and wasn't installed by a package manager, so Depot can't remove it safely."
                .into(),
        );
    }
    p.manual = Some(plan);
    p
}

/// Read everything installed. Sources are read in parallel; one failing source only adds a warning.
pub fn scan(env: &Env) -> Inventory {
    let native = Native::detect(env);
    let (natives, snaps, flats, flat_usage, launchers) = std::thread::scope(|s| {
        let d = s.spawn(|| native.map_or(Ok(Vec::new()), |n| n.load(env)));
        let sn = s.spawn(|| env.snap_socket.as_deref().map_or(Ok(Vec::new()), snap::load));
        let f = s.spawn(|| if env.flatpak { flatpak::load() } else { Ok(Vec::new()) });
        let fu = s.spawn(|| if env.flatpak { flatpak::usage() } else { None });
        let l = xdg::collect(&env.app_dirs);
        let join = |h: std::thread::ScopedJoinHandle<'_, Result<Vec<Package>, String>>| {
            h.join().unwrap_or_else(|_| Err("A loader crashed.".into()))
        };
        (join(d), join(sn), join(f), fu.join().ok().flatten(), l)
    });

    let mut inv = Inventory { system_packages: native.is_some(), ..Default::default() };
    let mut take = |r: Result<Vec<Package>, String>, what: &str| match r {
        Ok(v) => v,
        Err(e) => {
            inv.warnings.push(format!("{what}: {e}"));
            Vec::new()
        }
    };
    let natives = take(natives, native.map_or("System packages", Native::label));
    let snaps = take(snaps, "Snaps");
    let flats = take(flats, "Flatpaks");

    let mut pkgs: Vec<Package> = Vec::with_capacity(natives.len() + snaps.len() + flats.len() + 16);
    // native packages by the name their database's file owners use: the id for dpkg, the
    // package name for rpm and pacman (preferring the native-architecture copy)
    let mut native_ix: HashMap<String, usize> = HashMap::new();
    let mut snap_ix: HashMap<String, usize> = HashMap::new();
    let mut flat_ix: HashMap<String, usize> = HashMap::new();
    for p in natives.into_iter().chain(snaps).chain(flats) {
        let i = pkgs.len();
        match p.kind {
            Kind::Deb => {
                native_ix.insert(p.id.clone(), i);
            }
            Kind::Rpm | Kind::Pacman => {
                if p.id == p.name || !native_ix.contains_key(&p.name) {
                    native_ix.insert(p.name.clone(), i);
                }
            }
            Kind::Snap => {
                snap_ix.insert(p.name.clone(), i);
            }
            Kind::Flatpak => {
                // prefer an app over a same-named runtime, and the default branch over others
                let rank = |q: &Package| (q.role != Role::Runtime, q.current);
                if flat_ix.get(&p.name).is_none_or(|&j| rank(&p) > rank(&pkgs[j])) {
                    flat_ix.insert(p.name.clone(), i);
                }
            }
            Kind::AppImage | Kind::Manual => {}
        }
        pkgs.push(p);
    }

    // like the app menu, skip launchers whose TryExec program isn't installed
    let shown: Vec<&Launcher> = launchers
        .iter()
        .filter(|l| l.entry.id != SELF_ID && l.entry.shown(&env.desktops))
        .filter(|l| l.entry.try_exec.is_empty() || exec::resolve(&l.entry.try_exec, &env.path).is_some())
        .collect();
    let mut icon_users: HashMap<String, usize> = HashMap::new();
    for l in &launchers {
        if !l.entry.icon.is_empty() {
            *icon_users.entry(l.entry.icon.clone()).or_default() += 1;
        }
    }
    let claims: Vec<Claim> = shown.iter().map(|l| claim(l, env)).collect();

    let owners = if let Some(native) = native {
        let wanted: HashSet<PathBuf> = claims
            .iter()
            .filter_map(|c| if let Claim::Files(f, _, _) = c { Some(f.iter().map(|(p, _)| p.clone())) } else { None })
            .flatten()
            .collect();
        let (owners, error) = native.owners(env, &wanted);
        if let Some(e) = error {
            inv.warnings.push(format!(
                "{}: couldn't tell which apps came from them ({e}). They may show up as manual installs; press Refresh to try again.",
                native.label()
            ));
        }
        owners
    } else {
        HashMap::new()
    };

    // Launchers owned outright come first, so a package's own launcher is its main one and
    // copies made by install scripts become extra launchers rather than the main one.
    let mut deferred = Vec::new();
    for (l, c) in shown.iter().zip(&claims) {
        match c {
            Claim::Snap(name) => match snap_ix.get(name) {
                Some(&i) => {
                    if pkgs[i].role != Role::Runtime {
                        attach(&mut pkgs[i], l)
                    }
                }
                None => deferred.push((*l, None, false, true)),
            },
            Claim::Flatpak(id) => {
                let hit = flat_ix.get(id).or_else(|| {
                    flat_ix.iter().filter(|(k, _)| flatpak_child(id, k)).max_by_key(|(k, _)| k.len()).map(|(_, i)| i)
                });
                match hit {
                    Some(&i) => attach(&mut pkgs[i], l),
                    None => deferred.push((*l, None, false, true)),
                }
            }
            Claim::WebApp(missing) => deferred.push((*l, None, true, *missing)),
            Claim::Files(files, program, missing) => {
                let direct = files.iter().find(|(p, via_exec)| !via_exec && owners.contains_key(p));
                match direct.and_then(|(p, _)| native_ix.get(&owners[p])) {
                    Some(&i) => attach(&mut pkgs[i], l),
                    None => deferred.push((*l, Some((files, program.as_deref())), false, *missing)),
                }
            }
        }
    }
    // programs of hand-installed launchers, so one app's folder is never mistaken for a shared one
    let programs: Vec<PathBuf> = claims
        .iter()
        .filter_map(|c| if let Claim::Files(_, Some(p), _) = c { Some(p) } else { None })
        .filter(|p| p.starts_with(&env.home))
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let mut appimages = HashSet::new();
    for (l, files, web, broken) in deferred {
        if let Some((files, program)) = files {
            let by_exec = files.iter().find_map(|(p, _)| owners.get(p)).and_then(|o| native_ix.get(o));
            if let Some(&i) = by_exec {
                attach(&mut pkgs[i], l);
                continue;
            }
            match program.filter(|p| appimage::is_appimage(p)) {
                Some(file) => {
                    appimages.insert(std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf()));
                    pkgs.push(appimage::from_launcher(&l.entry, file, &env.home, &icon_users));
                }
                None => pkgs.push(manual_package(l, program, env, &icon_users, &programs, false, broken)),
            }
        } else {
            pkgs.push(manual_package(l, None, env, &icon_users, &programs, web, broken));
        }
    }
    pkgs.extend(appimage::loose(&env.home, &appimages));

    for p in &mut pkgs {
        if p.icon.is_none() {
            p.icon = match p.kind {
                Kind::Snap => snap::bundled_icon(&p.name),
                Kind::Flatpak => flatpak::icon(&env.flatpak_share, &p.name),
                _ => None,
            };
        } else if p.kind == Kind::Flatpak && p.icon.as_deref() == Some(p.name.as_str()) {
            // exported icons may not be on the icon theme path; prefer the file
            if let Some(f) = flatpak::icon(&env.flatpak_share, &p.name) {
                p.icon = Some(f);
            }
        }
    }
    // the same ref installed both for everyone and for this user would otherwise be two identical cards
    let mut flat_refs: HashMap<&str, usize> = HashMap::new();
    for p in pkgs.iter().filter(|p| p.kind == Kind::Flatpak) {
        *flat_refs.entry(p.id.as_str()).or_default() += 1;
    }
    let twice: HashSet<String> = flat_refs.into_iter().filter(|(_, n)| *n > 1).map(|(id, _)| id.to_string()).collect();
    for p in pkgs.iter_mut().filter(|p| p.kind == Kind::Flatpak && twice.contains(&p.id)) {
        p.title = format!("{} ({})", p.title, if p.system { "all users" } else { "only you" });
    }
    pkgs.sort_by_cached_key(|p| p.title.to_lowercase());
    inv.duplicates = crate::group::duplicates(&pkgs);
    inv.unused_runtimes = flat_usage.map(|u| flatpak::unused(&pkgs, &u)).unwrap_or_default();
    inv.packages = pkgs;
    inv
}

#[cfg(test)]
mod tests {
    #[test]
    fn os_release_family() {
        let t = "NAME=\"Zorin OS\"\nID=zorin\nID_LIKE=\"ubuntu debian\"\nVERSION_ID=18\n";
        assert_eq!(super::parse_os_like(t), ["zorin", "ubuntu", "debian"]);
    }

    #[test]
    fn flatpak_child_needs_a_name_boundary() {
        assert!(super::flatpak_child("org.x.App.Tool", "org.x.App"));
        assert!(!super::flatpak_child("org.x.AppTwo", "org.x.App"));
        assert!(!super::flatpak_child("org.x.App", "org.x.App"));
    }
}
