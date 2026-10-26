//! Python apps installed with pipx, each in a folder of its own.

use crate::command;
use crate::model::{Kind, Package, Role};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Installed apps. Nothing is read if pipx isn't installed.
pub fn load() -> Result<Vec<Package>, String> {
    let Some(text) = command::output("pipx", &["list", "--json"])? else { return Ok(Vec::new()) };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("pipx sent invalid data: {e}"))?;
    Ok(parse(&v))
}

/// pipx writes paths as `{"__Path__": "/home/u/.local/bin/black", "__type__": "Path"}`.
fn path(v: &Value) -> Option<PathBuf> {
    v.get("__Path__").and_then(Value::as_str).map(PathBuf::from)
}

pub fn parse(v: &Value) -> Vec<Package> {
    let Some(venvs) = v.get("venvs").and_then(Value::as_object) else { return Vec::new() };
    venvs
        .iter()
        .map(|(name, o)| {
            let main = &o["metadata"]["main_package"];
            let mut p = Package::new(Kind::Pipx, name.as_str());
            p.role = Role::Tool;
            p.user_installed = true;
            p.version = main.get("package_version").and_then(Value::as_str).unwrap_or("").to_string();
            p.homepage =
                format!("https://pypi.org/project/{}/", main.get("package").and_then(Value::as_str).unwrap_or(name));
            let apps: Vec<&str> =
                main.get("apps").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
            if !apps.is_empty() {
                p.summary = format!("{}: {}", if apps.len() == 1 { "Command" } else { "Commands" }, apps.join(", "));
            }
            // the app's commands live in its own folder: `<venvs>/<name>/bin/<command>`
            let venv = main
                .get("app_paths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(path)
                .find_map(|a| a.parent()?.parent().map(Path::to_path_buf));
            if let Some(dir) = venv {
                p.location = dir.to_string_lossy().into_owned();
                p.paths = vec![dir];
            }
            if let Some(py) = o["metadata"].get("python_version").and_then(Value::as_str) {
                p.revision = py.to_string();
            }
            p
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_with_their_own_folders() {
        let v: Value = serde_json::from_str(
            r#"{"pipx_spec_version":"0.1","venvs":{"black":{"metadata":{"injected_packages":{},
            "main_package":{"app_paths":[{"__Path__":"/home/u/.local/share/pipx/venvs/black/bin/black","__type__":"Path"},
            {"__Path__":"/home/u/.local/share/pipx/venvs/black/bin/blackd","__type__":"Path"}],
            "apps":["black","blackd"],"package":"black","package_or_url":"black","package_version":"24.1.0","pip_args":[],"suffix":""},
            "python_version":"Python 3.12.3","venv_args":[]}}}}"#,
        )
        .unwrap();
        let p = parse(&v);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].name.as_str(), p[0].version.as_str()), ("black", "24.1.0"));
        assert_eq!(p[0].paths, [PathBuf::from("/home/u/.local/share/pipx/venvs/black")]);
        assert_eq!(p[0].summary, "Commands: black, blackd");
    }
}
