//! Finding one app installed several ways (say Firefox as a Deb and a Snap), so it can be shown once.

use crate::model::{Kind, Package, Role};
use std::collections::HashMap;

/// Words that are too generic to identify an app on their own (`com.spotify.Client`).
const GENERIC: &[&str] = &["client", "app", "desktop", "browser", "editor", "player", "launcher", "gui", "bin"];

fn norm(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()
}

/// The part of a package's id that names the app: `firefox` for the deb, snap and
/// `org.mozilla.firefox`.
fn token(p: &Package) -> String {
    let raw = match p.kind {
        Kind::Flatpak | Kind::Manual => p.name.rsplit('.').next().unwrap_or(&p.name),
        _ => &p.name,
    };
    norm(raw)
}

/// Order of preference for the copy shown on the card: the system's own package first.
fn rank(k: Kind) -> u8 {
    match k {
        Kind::Deb | Kind::Rpm | Kind::Pacman => 0,
        Kind::Flatpak => 1,
        Kind::Snap => 2,
        Kind::Manual => 3,
    }
}

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while parent[r] != r {
        parent[r] = parent[parent[r]];
        r = parent[r];
    }
    r
}

/// Groups of indices into `pkgs` that are the same app installed different ways, best copy
/// first. Apps installed only once aren't listed. Two different packages of the same kind are
/// never grouped, so look-alike names can't merge unrelated apps.
pub fn duplicates(pkgs: &[Package]) -> Vec<Vec<usize>> {
    let apps: Vec<usize> = (0..pkgs.len())
        .filter(|&i| {
            let p = &pkgs[i];
            p.role == Role::App && !p.broken && p.location != "Web app"
        })
        .collect();
    let mut parent: Vec<usize> = (0..pkgs.len()).collect();
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for &i in &apps {
        let t = token(&pkgs[i]);
        if t.len() >= 3 && !GENERIC.contains(&t.as_str()) {
            by_key.entry(format!("id:{t}")).or_default().push(i);
        }
        let title = norm(&pkgs[i].title);
        if title.len() >= 3 {
            by_key.entry(format!("title:{title}")).or_default().push(i);
        }
    }
    // two different packages of one kind sharing a key means the key is ambiguous ("Terminal"),
    // so it can't tell which of them a third copy belongs to
    let same_kind_clash = |m: &[usize]| {
        m.iter().any(|&a| m.iter().any(|&b| pkgs[a].kind == pkgs[b].kind && pkgs[a].name != pkgs[b].name))
    };
    for members in by_key.values().filter(|m| !same_kind_clash(m)) {
        for w in members.windows(2) {
            if pkgs[w[0]].kind != pkgs[w[1]].kind {
                let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &apps {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups
        .into_values()
        .filter(|g| g.len() > 1)
        .filter(|g| {
            // the same app twice in one kind is fine (a Flatpak for you and for everyone),
            // two different packages of one kind means the names only look alike
            g.iter().all(|&a| g.iter().all(|&b| pkgs[a].kind != pkgs[b].kind || pkgs[a].name == pkgs[b].name))
        })
        .map(|mut g| {
            g.sort_by_key(|&i| (rank(pkgs[i].kind), i));
            g
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(kind: Kind, name: &str, title: &str) -> Package {
        let mut p = Package::new(kind, name);
        p.title = title.into();
        p.role = Role::App;
        p
    }

    #[test]
    fn same_app_different_ways() {
        let pkgs = vec![
            app(Kind::Snap, "firefox", "Firefox"),
            app(Kind::Deb, "firefox", "Firefox Web Browser"),
            app(Kind::Flatpak, "org.mozilla.firefox", "Firefox"),
            app(Kind::Deb, "spotify-client", "Spotify"),
            app(Kind::Flatpak, "com.spotify.Client", "Spotify"),
            app(Kind::Deb, "gimp", "GNU Image Manipulation Program"),
        ];
        let g = duplicates(&pkgs);
        assert_eq!(g, vec![vec![1, 2, 0], vec![3, 4]], "the deb comes first, then Flatpak, then Snap");
    }

    #[test]
    fn look_alikes_stay_apart() {
        let pkgs = vec![
            app(Kind::Deb, "gnome-terminal", "Terminal"),
            app(Kind::Deb, "xfce4-terminal", "Terminal"),
            app(Kind::Flatpak, "org.example.Client", "Example"),
            app(Kind::Snap, "other-client", "Other"),
        ];
        assert!(duplicates(&pkgs).is_empty());
        // a third copy can't chain two different debs together
        let mut more = pkgs.clone();
        more.push(app(Kind::Snap, "terminal", "Terminal"));
        assert!(duplicates(&more).is_empty());
    }

    #[test]
    fn web_apps_and_tools_are_left_out() {
        let mut web = app(Kind::Manual, "chrome-abc", "Firefox");
        web.location = "Web app".into();
        let mut tool = app(Kind::Snap, "firefox", "Firefox");
        tool.role = Role::Tool;
        assert!(duplicates(&[app(Kind::Deb, "firefox", "Firefox"), web, tool]).is_empty());
    }
}
