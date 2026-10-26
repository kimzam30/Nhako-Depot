//! Working out exactly what an uninstall of a native package (deb, rpm, pacman) will remove, so the user sees it before anything changes.
//! Everything removed is named explicitly; nothing relies on "autoremove", whose set can be larger
//! than what was shown.

use crate::backend::{self, dpkg};
use crate::model::Kind;
use crate::updates::{info, pk_split};
use std::path::{Component, Path, PathBuf};

/// What removing one or more native packages involves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovePlan {
    /// Packages that need the targets and must go with them, as package-manager ids
    /// (`name`, or `name:arch` / `name.arch` for other architectures).
    pub deps: Vec<String>,
    /// Automatically installed packages nothing needs any more once the targets are gone.
    pub orphans: Vec<String>,
    /// PackageKit ids of `orphans`, in the same order, when PackageKit made the plan.
    pub orphan_pk_ids: Vec<String>,
    /// Why it can't be removed, if it can't.
    pub blocked: Option<String>,
    /// PackageKit's ids for the targets, when PackageKit will do the removal; empty otherwise.
    pub pk_ids: Vec<String>,
}

pub const SYSTEM_NEEDS_IT: &str = "Your system needs this package to work, so it can't be removed.";

/// The apt name for a `name:arch` that may carry the native architecture.
fn apt_name(s: &str) -> String {
    match s.split_once(':') {
        Some((n, a)) => dpkg::pkg_id(n, a),
        None => s.to_string(),
    }
}

/// Name and architecture of a PackageKit id. Its last field says where the package came from
/// and differs between calls (`installed`, `installed:fedora`), so it never identifies one.
fn name_arch(pk_id: &str) -> Option<(&str, &str)> {
    pk_split(pk_id).map(|(n, _, a, _)| (n, a))
}

/// Packages a PackageKit dry run says it would remove, other than the `targets`, as `(native id, pk id)`.
fn pk_removed(kind: Kind, list: &[(u32, String)], targets: &[String]) -> Vec<(String, String)> {
    let targets: Vec<(&str, &str)> = targets.iter().filter_map(|t| name_arch(t)).collect();
    let mut v: Vec<(String, String)> = list
        .iter()
        .filter(|(i, _)| *i == info::REMOVING)
        .filter_map(|(_, id)| {
            let (name, arch) = name_arch(id)?;
            (!targets.contains(&(name, arch))).then(|| (backend::native_id(kind, name, arch), id.clone()))
        })
        .collect();
    v.sort();
    v.dedup_by(|a, b| a.0 == b.0);
    v
}

/// Build a plan from two PackageKit dry runs of removing `targets`: one with autoremove (`with`)
/// and one without (`without`). The difference is what's merely left unused.
pub fn plan_from_packagekit(
    kind: Kind,
    targets: &[String],
    with: &[(u32, String)],
    without: &[(u32, String)],
) -> RemovePlan {
    let deps: Vec<String> = pk_removed(kind, without, targets).into_iter().map(|(n, _)| n).collect();
    let (orphans, orphan_pk_ids) =
        pk_removed(kind, with, targets).into_iter().filter(|(n, _)| !deps.contains(n)).unzip();
    RemovePlan { deps, orphans, orphan_pk_ids, blocked: None, pk_ids: targets.to_vec() }
}

/// Packages an `apt-get -s remove` run would remove, other than the `ids` themselves.
fn parse_apt_simulation(stdout: &str, ids: &[String]) -> Vec<String> {
    let ids: Vec<String> = ids.iter().map(|i| apt_name(i)).collect();
    let mut v: Vec<String> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("Remv ")?.split_whitespace().next())
        .map(apt_name)
        .filter(|m| !ids.contains(m))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Packages a `pacman -R…p --print-format %n` dry run lists, other than the `ids` themselves.
fn parse_pacman_print(stdout: &str, ids: &[String]) -> Vec<String> {
    let mut v: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !ids.iter().any(|i| i == l))
        .map(str::to_string)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Why apt or pacman refused the dry run, if it did.
pub fn refusal(stdout: &str, stderr: &str, success: bool) -> Option<String> {
    if stdout.contains("essential packages will be removed") || stderr.contains("essential packages will be removed") {
        return Some("Removing this would also remove essential system packages.".into());
    }
    if success {
        return None;
    }
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    Some(text.trim().lines().last().unwrap_or("The package manager refused.").to_string())
}

/// Build a plan from two dry runs of the package manager itself: one that removes the `ids` and
/// what needs them (`without`), and one that also removes what's left unused (`with`). For apt
/// these are `-s remove` and `-s remove --autoremove`; for pacman `-Rcp` and `-Rcsp` with
/// `--print-format %n`.
pub fn plan_from_dry_runs(kind: Kind, ids: &[String], with: &str, without: &str) -> RemovePlan {
    let parse = if kind == Kind::Pacman { parse_pacman_print } else { parse_apt_simulation };
    let deps = parse(without, ids);
    let orphans = parse(with, ids).into_iter().filter(|n| !deps.contains(n)).collect();
    RemovePlan { deps, orphans, ..Default::default() }
}

/// The pacman command that removes exactly the `ids`, the packages that need them and the chosen
/// orphans. `purge` also skips keeping `.pacsave` backups of changed settings files.
pub fn pacman_command(ids: &[String], purge: bool, deps: &[String], orphans: &[String]) -> Vec<String> {
    let mut c = vec!["pacman".to_string(), "-R".into(), "--noconfirm".into()];
    if purge {
        c.push("--nosave".into());
    }
    c.push("--".into());
    c.extend(ids.iter().chain(deps).chain(orphans).cloned());
    c
}

/// How long apt waits for another installer (Software, automatic updates) to finish, in seconds,
/// instead of failing straight away.
const APT_LOCK_WAIT: &str = "DPkg::Lock::Timeout=120";

/// The apt command that removes the `ids` and exactly the listed orphans (dependents go with them anyway).
pub fn apt_command(ids: &[String], purge: bool, orphans: &[String]) -> Vec<String> {
    let mut c = vec![
        "apt-get".to_string(),
        "-o".into(),
        APT_LOCK_WAIT.into(),
        if purge { "purge" } else { "remove" }.into(),
        "-y".into(),
        "--".into(),
    ];
    c.extend(ids.iter().chain(orphans).cloned());
    c
}

/// Folders in a home folder that hold the user's own things, every app's settings, or a whole
/// tool's installs. Depot never trashes one of these as a whole, whatever a plan says, and never
/// anything inside the key stores. The user's own folders under their translated names come from
/// `~/.config/user-dirs.dirs` (see [`user_dirs`]).
const PERSONAL: &[&str] = &[
    ".config",
    ".local",
    ".local/share",
    ".local/bin",
    ".local/state",
    ".cache",
    ".var",
    ".var/app",
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "Public",
    "Templates",
    "Videos",
    "Applications",
    "AppImages",
    "bin",
    "snap",
    ".mozilla",
    ".thunderbird",
    ".steam",
    ".local/share/Steam",
    ".local/share/flatpak",
    ".cargo",
    ".cargo/bin",
    ".rustup",
    ".local/pipx",
    ".local/share/pipx",
    ".npm",
    ".npm-global",
    ".nix-profile",
    ".linuxbrew",
    ".nix-defexpr",
    "go",
];
const KEY_STORES: &[&str] = &[".ssh", ".gnupg", ".pki", ".password-store", ".local/share/keyrings"];

/// The user's own folders (Documents, Music…) as `xdg-user-dirs` names them in their language,
/// when they're inside `home`.
fn user_dirs(home: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(home.join(".config/user-dirs.dirs")).unwrap_or_default();
    text.lines()
        .filter(|l| l.starts_with("XDG_"))
        .filter_map(|l| l.split_once('=').map(|(_, v)| v.trim().trim_matches('"')))
        .filter_map(|v| v.strip_prefix("$HOME/"))
        .map(|rel| home.join(rel.trim_end_matches('/')))
        .collect()
}

fn lexically_safe(t: &Path, home: &Path, protected: &[PathBuf]) -> bool {
    t.is_absolute()
        && t.starts_with(home)
        && t != home
        && !protected.iter().any(|r| r == t)
        && !t.components().any(|c| matches!(c, Component::ParentDir))
        && !PERSONAL.iter().any(|f| t == home.join(f))
        && !KEY_STORES.iter().any(|f| t.starts_with(home.join(f)))
}

/// A path Depot is willing to move to the Trash: inside a real home folder, not the home folder
/// itself, not one of the shared app folders or the user's personal folders, and without `..`.
/// Checked both as written and with symlinks resolved, so a link can't point it somewhere else.
pub fn safe_target(t: &Path, home: &Path, shared: &[impl AsRef<Path>]) -> bool {
    let home_ok = home.is_absolute() && home.components().count() > 1;
    let mut protected: Vec<PathBuf> = shared.iter().map(|r| r.as_ref().to_path_buf()).collect();
    protected.extend(user_dirs(home));
    if !home_ok || !lexically_safe(t, home, &protected) {
        return false;
    }
    // the item itself may be a symlink (it's the link that gets trashed), but the folders it sits
    // in must really be inside the home folder
    let (Some(parent), Some(name)) = (t.parent(), t.file_name()) else { return false };
    match (std::fs::canonicalize(parent), std::fs::canonicalize(home)) {
        (Ok(p), Ok(h)) => {
            let protected: Vec<PathBuf> =
                protected.iter().map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.clone())).collect();
            lexically_safe(&p.join(name), &h, &protected)
        }
        // nothing there to resolve: the lexical check is all there is, and trashing it fails anyway
        _ if std::fs::symlink_metadata(t).is_err() => true,
        // it's there but its folders can't be resolved (no permission): too unsure to touch
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packagekit_plan_lists_exactly_what_goes() {
        let arch = dpkg::native_arch();
        let target = format!("app;1;{arch};installed");
        let dep = (info::REMOVING, format!("app-plugin;1;{arch};installed"));
        let orphan = (info::REMOVING, format!("libapp;1;{arch};installed"));
        let foreign = (info::REMOVING, "libapp32;1;i386;installed".to_string());
        let itself = (info::REMOVING, target.clone());
        let without = vec![itself.clone(), dep.clone()];
        let with = vec![itself, dep, orphan.clone(), foreign.clone()];
        let p = plan_from_packagekit(Kind::Deb, std::slice::from_ref(&target), &with, &without);
        assert_eq!(p.deps, ["app-plugin"]);
        let mut want =
            vec!["libapp".to_string(), if arch == "i386" { "libapp32".into() } else { "libapp32:i386".into() }];
        want.sort();
        assert_eq!(p.orphans, want);
        assert_eq!(p.orphan_pk_ids.len(), 2);
        assert!(p.orphan_pk_ids.contains(&orphan.1) && p.orphan_pk_ids.contains(&foreign.1));
        assert_eq!(p.pk_ids, std::slice::from_ref(&target));
        // the same package reported with a different origin field is still the target itself
        let moved = vec![(info::REMOVING, format!("app;1;{arch};installed:ubuntu"))];
        assert!(plan_from_packagekit(Kind::Deb, &[target], &moved, &moved).deps.is_empty());
    }

    #[test]
    fn apt_plan_and_command_name_every_package() {
        let arch = dpkg::native_arch();
        let without = format!("Remv app [1.0]\nRemv app-plugin:{arch} [1.0]\n");
        let with = format!("{without}Remv libapp [2]\nRemv libapp32:i386 [2]\n");
        let app = ["app".to_string()];
        let p = plan_from_dry_runs(Kind::Deb, &app, &with, &without);
        assert_eq!(p.deps, ["app-plugin"]);
        assert!(p.orphans.contains(&"libapp".to_string()));
        assert!(p.pk_ids.is_empty());
        // removing with no orphans ticked never asks apt to autoremove anything
        let lock = ["apt-get", "-o", APT_LOCK_WAIT];
        assert_eq!(apt_command(&app, false, &[]), [&lock[..], &["remove", "-y", "--", "app"]].concat());
        assert_eq!(
            apt_command(&app, true, &p.orphans[..1]),
            [&lock[..], &["purge", "-y", "--", "app", &p.orphans[0]]].concat()
        );
        assert!(!apt_command(&app, true, &p.orphans).iter().any(|a| a.contains("autoremove")));
        // several apps at once: none of them counts as a dependency of the others
        let both = ["app".to_string(), "app-plugin".to_string()];
        assert!(plan_from_dry_runs(Kind::Deb, &both, &with, &without).deps.is_empty());
    }

    #[test]
    fn pacman_plan_and_command() {
        let app = ["app".to_string()];
        let p = plan_from_dry_runs(Kind::Pacman, &app, "app\napp-plugin\nlibapp\n", "app\napp-plugin\n");
        assert_eq!(
            (p.deps.as_slice(), p.orphans.as_slice()),
            (&["app-plugin".to_string()][..], &["libapp".to_string()][..])
        );
        assert_eq!(
            pacman_command(&app, true, &p.deps, &[]),
            ["pacman", "-R", "--noconfirm", "--nosave", "--", "app", "app-plugin"]
        );
    }

    #[test]
    fn rpm_ids_from_packagekit() {
        let arch = crate::backend::rpm::native_arch();
        let target = format!("app;1;{arch};fedora");
        let with =
            vec![(info::REMOVING, format!("libapp;1;{arch};fedora")), (info::REMOVING, "lib32;1;i686;fedora".into())];
        let p = plan_from_packagekit(Kind::Rpm, &[target], &with, &[]);
        assert!(p.orphans.contains(&"libapp".to_string()));
        if arch != "i686" {
            assert!(p.orphans.contains(&"lib32.i686".to_string()));
        }
    }

    #[test]
    fn apt_refusals() {
        assert!(refusal("WARNING: The following essential packages will be removed.", "", false).is_some());
        assert_eq!(
            refusal("", "E: Unable to locate package x\n", false).as_deref(),
            Some("E: Unable to locate package x")
        );
        assert_eq!(refusal("Remv x [1]", "", true), None);
    }

    #[test]
    fn trash_only_inside_a_real_home() {
        let home = PathBuf::from("/home/u");
        let shared = [home.join(".local/opt")];
        assert!(safe_target(&home.join(".local/opt/foo"), &home, &shared));
        assert!(!safe_target(&home, &home, &shared));
        assert!(!safe_target(&home.join(".local/opt"), &home, &shared));
        assert!(!safe_target(Path::new("/usr/bin/x"), &home, &shared));
        assert!(!safe_target(&home.join("a/../../x"), &home, &shared));
        // an unset or root home must not make everything fair game
        assert!(!safe_target(Path::new("/usr/bin/x"), Path::new(""), &shared));
        assert!(!safe_target(Path::new("/usr/bin/x"), Path::new("/"), &shared));
        // the user's own folders, and anything in the key stores, are never fair game
        assert!(!safe_target(&home.join(".config"), &home, &shared));
        assert!(!safe_target(&home.join("Documents"), &home, &shared));
        assert!(!safe_target(&home.join(".ssh/id_ed25519"), &home, &shared));
        assert!(safe_target(&home.join(".config/someapp"), &home, &shared));
    }

    #[test]
    fn trash_spares_tool_homes_and_translated_folders() {
        let root = std::env::temp_dir().join(format!("depot-safe-dirs-{}", std::process::id()));
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(
            home.join(".config/user-dirs.dirs"),
            "# written by xdg-user-dirs-update\nXDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\nXDG_MUSIC_DIR=\"/elsewhere/Musik\"\n",
        )
        .unwrap();
        let shared: [PathBuf; 0] = [];
        for d in ["Dokumente", "AppImages", "snap", ".cargo", ".cargo/bin", ".nix-profile", ".steam", ".thunderbird"] {
            assert!(!safe_target(&home.join(d), &home, &shared), "{d} must never be trashed");
        }
        assert!(!safe_target(&home.join(".password-store/bank.gpg"), &home, &shared));
        assert!(safe_target(&home.join("Dokumente/old-app"), &home, &shared), "only the folder itself is kept");
        assert!(safe_target(&home.join("Musik"), &home, &shared), "a user folder outside home protects nothing here");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn trash_refuses_a_folder_linked_out_of_home() {
        let root = std::env::temp_dir().join(format!("depot-safe-{}", std::process::id()));
        let (home, outside) = (root.join("home"), root.join("outside"));
        std::fs::create_dir_all(home.join(".local/opt")).unwrap();
        std::fs::create_dir_all(outside.join("app")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("Apps")).unwrap();
        let shared = [home.join(".local/opt")];
        assert!(!safe_target(&home.join("Apps/app"), &home, &shared), "resolves outside the home folder");
        // a symlink itself may go: it's the link that moves to the Trash, not what it points at
        assert!(safe_target(&home.join("Apps"), &home, &shared));
        let _ = std::fs::remove_dir_all(root);
    }
}
