//! Node.js packages installed with `npm install -g`.

use crate::model::{Kind, Package, Role};
use crate::{command, exec};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Shipped with Node.js itself, so they belong to whatever installed Node.
const BUNDLED: &[&str] = &["npm", "corepack"];

/// npm's global prefix: `NPM_CONFIG_PREFIX`, `prefix=` in `~/.npmrc`, or else the folder Node
/// itself is installed in (`/usr`, `~/.nvm/versions/node/v22`).
pub fn global_prefix(home: &Path, search: &[PathBuf]) -> Option<PathBuf> {
    let expand = |v: &str| {
        let v = v.trim().trim_matches('"');
        v.strip_prefix("~/").map_or_else(|| PathBuf::from(v), |r| home.join(r))
    };
    if let Some(v) = std::env::var("NPM_CONFIG_PREFIX").ok().or_else(|| std::env::var("npm_config_prefix").ok()) {
        return Some(expand(&v));
    }
    let rc = std::fs::read_to_string(home.join(".npmrc")).unwrap_or_default();
    if let Some(v) =
        rc.lines().find_map(|l| l.trim().strip_prefix("prefix").map(str::trim).and_then(|r| r.strip_prefix('=')))
    {
        return Some(expand(v));
    }
    let node = exec::resolve("node", search)?;
    node.parent()?.parent().map(Path::to_path_buf)
}

/// Global packages, read from npm's global `node_modules` folder. Nothing is read if Node.js
/// isn't installed.
pub fn load(home: &Path, search: &[PathBuf]) -> Result<Vec<Package>, String> {
    let Some(prefix) = global_prefix(home, search) else { return Ok(Vec::new()) };
    Ok(read_root(&prefix.join("lib/node_modules"), home))
}

/// The packages in a global `node_modules` folder, `@scope/name` ones included.
pub fn read_root(root: &Path, home: &Path) -> Vec<Package> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('@') {
            let scoped = std::fs::read_dir(e.path()).into_iter().flatten().flatten();
            names.extend(scoped.map(|s| format!("{name}/{}", s.file_name().to_string_lossy())));
        } else if !name.starts_with('.') {
            names.push(name);
        }
    }
    names.sort();
    names
        .into_iter()
        .filter(|n| !BUNDLED.contains(&n.as_str()))
        .filter_map(|n| {
            let text = std::fs::read_to_string(root.join(&n).join("package.json")).ok()?;
            let v: Value = serde_json::from_str(&text).ok()?;
            Some(package(&n, &v, root, home))
        })
        .collect()
}

/// One package from its `package.json`. Packages are only removable from Depot when `root` is in
/// the home folder: elsewhere npm needs an administrator.
fn package(name: &str, v: &Value, root: &Path, home: &Path) -> Package {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut p = Package::new(Kind::Npm, name);
    p.role = Role::Tool;
    p.user_installed = true;
    p.version = s("version");
    p.summary = s("description");
    p.homepage =
        Some(s("homepage")).filter(|h| !h.is_empty()).unwrap_or(format!("https://www.npmjs.com/package/{name}"));
    let dir = root.join(name);
    p.location = dir.to_string_lossy().into_owned();
    p.paths = vec![dir];
    if !root.starts_with(home) {
        p.blocked = Some(format!(
            "It's installed for everyone, which needs an administrator to change. In a terminal: sudo npm uninstall -g {name}"
        ));
    }
    p
}

/// `npm outdated -g --json`: `(name, newest version)` for each package with a newer version.
pub fn parse_outdated(v: &Value) -> Vec<(String, String)> {
    let Some(m) = v.as_object() else { return Vec::new() };
    m.iter()
        .filter(|(name, _)| !BUNDLED.contains(&name.as_str()))
        .filter_map(|(name, o)| {
            let latest = o.get("latest").and_then(Value::as_str)?;
            let current = o.get("current").and_then(Value::as_str).unwrap_or("");
            (latest != current).then(|| (name.clone(), latest.to_string()))
        })
        .collect()
}

/// Packages with a newer version. npm reports them with exit code 1.
pub fn outdated() -> Result<Vec<(String, String)>, String> {
    let text = match command::output("npm", &["outdated", "-g", "--json"]) {
        Ok(t) => t.unwrap_or_default(),
        Err(f) if !f.stdout.trim().is_empty() => f.stdout,
        Err(f) => return Err(f.message),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("npm sent invalid data: {e}"))?;
    Ok(parse_outdated(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_with(dir: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("depot-npm-{dir}-{}", std::process::id()));
        for (name, json) in [
            ("npm", r#"{"name":"npm","version":"10.9.9"}"#),
            ("typescript", r#"{"name":"typescript","version":"5.6.2","description":"TypeScript is a language"}"#),
            ("@angular/cli", r#"{"name":"@angular/cli","version":"18.2.0"}"#),
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("package.json"), json).unwrap();
        }
        root
    }

    #[test]
    fn global_packages_without_the_ones_node_ships() {
        let root = root_with("home");
        let home = root.parent().unwrap();
        let p = read_root(&root, home);
        let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["@angular/cli", "typescript"]);
        assert_eq!(p[0].paths, [root.join("@angular/cli")]);
        assert_eq!((p[1].version.as_str(), p[1].summary.as_str()), ("5.6.2", "TypeScript is a language"));
        assert!(p.iter().all(|p| p.blocked.is_none()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn packages_installed_for_everyone_need_an_administrator() {
        let root = root_with("everyone");
        let p = read_root(&root, Path::new("/home/nobody-here"));
        assert!(p.iter().all(|p| p.blocked.as_deref().is_some_and(|b| b.contains("sudo npm uninstall -g"))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn outdated_lists_newer_versions() {
        let v: Value = serde_json::from_str(
            r#"{"npm":{"current":"10.9.9","latest":"12.2.1"},"typescript":{"current":"5.6.2","wanted":"5.6.2","latest":"5.7.1"}}"#,
        )
        .unwrap();
        assert_eq!(parse_outdated(&v), [("typescript".to_string(), "5.7.1".to_string())]);
    }
}
