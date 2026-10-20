//! Working out exactly what an uninstall will remove, so the user sees it before anything changes.
//! Everything removed is named explicitly; nothing relies on "autoremove", whose set can be larger
//! than what was shown.

use crate::backend::dpkg;
use crate::updates::{info, pk_split};
use std::path::{Component, Path};

/// What removing a Debian package involves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DebPlan {
    /// Packages that need this one and must go with it, as apt names (`name` or `name:arch`).
    pub deps: Vec<String>,
    /// Automatically installed packages nothing needs any more once this one is gone, as apt names.
    pub orphans: Vec<String>,
    /// PackageKit ids of `orphans`, in the same order, when PackageKit made the plan.
    pub orphan_pk_ids: Vec<String>,
    /// Why it can't be removed, if it can't.
    pub blocked: Option<String>,
    /// PackageKit's id for the package, when PackageKit will do the removal.
    pub pk_id: Option<String>,
}

pub const SYSTEM_NEEDS_IT: &str = "Your system needs this package to work, so it can't be removed.";

/// The apt name for a `name:arch` that may carry the native architecture.
fn apt_name(s: &str) -> String {
    match s.split_once(':') {
        Some((n, a)) => dpkg::pkg_id(n, a),
        None => s.to_string(),
    }
}

/// Packages a PackageKit dry run says it would remove, other than `target`, as `(apt name, pk id)`.
fn pk_removed(list: &[(u32, String)], target: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = list
        .iter()
        .filter(|(i, id)| *i == info::REMOVING && id != target)
        .filter_map(|(_, id)| {
            let (name, _, arch, _) = pk_split(id)?;
            Some((dpkg::pkg_id(name, arch), id.clone()))
        })
        .collect();
    v.sort();
    v.dedup_by(|a, b| a.0 == b.0);
    v
}

/// Build a plan from two PackageKit dry runs of removing `target`: one with autoremove (`with`)
/// and one without (`without`). The difference is what's merely left unused.
pub fn plan_from_packagekit(target: &str, with: &[(u32, String)], without: &[(u32, String)]) -> DebPlan {
    let deps: Vec<String> = pk_removed(without, target).into_iter().map(|(n, _)| n).collect();
    let (orphans, orphan_pk_ids) = pk_removed(with, target).into_iter().filter(|(n, _)| !deps.contains(n)).unzip();
    DebPlan { deps, orphans, orphan_pk_ids, blocked: None, pk_id: Some(target.to_string()) }
}

/// Packages an `apt-get -s remove` run would remove, other than `id` itself.
pub fn parse_apt_simulation(stdout: &str, id: &str) -> Vec<String> {
    let id = apt_name(id);
    let mut v: Vec<String> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("Remv ")?.split_whitespace().next())
        .map(apt_name)
        .filter(|m| *m != id)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Why apt refused the dry run, if it did.
pub fn apt_blocked(stdout: &str, stderr: &str, success: bool) -> Option<String> {
    if stdout.contains("essential packages will be removed") || stderr.contains("essential packages will be removed") {
        return Some("Removing this would also remove essential system packages.".into());
    }
    if success {
        return None;
    }
    let text = if stderr.trim().is_empty() { stdout } else { stderr };
    Some(text.trim().lines().last().unwrap_or("apt refused.").to_string())
}

/// Build a plan from two apt dry runs: plain `remove` (`without`) and `remove --autoremove` (`with`).
pub fn plan_from_apt(id: &str, with: &str, without: &str) -> DebPlan {
    let deps = parse_apt_simulation(without, id);
    let orphans = parse_apt_simulation(with, id).into_iter().filter(|n| !deps.contains(n)).collect();
    DebPlan { deps, orphans, ..Default::default() }
}

/// The apt command that removes `id` and exactly the listed orphans (dependents go with it anyway).
pub fn apt_command(id: &str, purge: bool, orphans: &[String]) -> Vec<String> {
    let mut c = vec!["apt-get".to_string(), if purge { "purge" } else { "remove" }.into(), "-y".into(), id.into()];
    c.extend(orphans.iter().cloned());
    c
}

/// A path Depot is willing to move to the Trash: inside a real home folder, not the home folder
/// itself, not one of the shared app folders, and without `..`.
pub fn safe_target(t: &Path, home: &Path, shared: &[impl AsRef<Path>]) -> bool {
    let home_ok = home.is_absolute() && home.components().count() > 1;
    home_ok
        && t.is_absolute()
        && t.starts_with(home)
        && t != home
        && !shared.iter().any(|r| r.as_ref() == t)
        && !t.components().any(|c| matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        let p = plan_from_packagekit(&target, &with, &without);
        assert_eq!(p.deps, ["app-plugin"]);
        let mut want =
            vec!["libapp".to_string(), if arch == "i386" { "libapp32".into() } else { "libapp32:i386".into() }];
        want.sort();
        assert_eq!(p.orphans, want);
        assert_eq!(p.orphan_pk_ids.len(), 2);
        assert!(p.orphan_pk_ids.contains(&orphan.1) && p.orphan_pk_ids.contains(&foreign.1));
        assert_eq!(p.pk_id.as_deref(), Some(target.as_str()));
    }

    #[test]
    fn apt_plan_and_command_name_every_package() {
        let arch = dpkg::native_arch();
        let without = format!("Remv app [1.0]\nRemv app-plugin:{arch} [1.0]\n");
        let with = format!("{without}Remv libapp [2]\nRemv libapp32:i386 [2]\n");
        let p = plan_from_apt("app", &with, &without);
        assert_eq!(p.deps, ["app-plugin"]);
        assert!(p.orphans.contains(&"libapp".to_string()));
        assert!(p.pk_id.is_none());
        // removing with no orphans ticked never asks apt to autoremove anything
        assert_eq!(apt_command("app", false, &[]), ["apt-get", "remove", "-y", "app"]);
        assert_eq!(apt_command("app", true, &p.orphans[..1]), ["apt-get", "purge", "-y", "app", &p.orphans[0]]);
        assert!(!apt_command("app", true, &p.orphans).iter().any(|a| a.contains("autoremove")));
    }

    #[test]
    fn apt_refusals() {
        assert!(apt_blocked("WARNING: The following essential packages will be removed.", "", false).is_some());
        assert_eq!(
            apt_blocked("", "E: Unable to locate package x\n", false).as_deref(),
            Some("E: Unable to locate package x")
        );
        assert_eq!(apt_blocked("Remv x [1]", "", true), None);
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
    }
}
