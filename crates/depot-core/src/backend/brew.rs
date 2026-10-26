//! Homebrew on Linux ("Linuxbrew"). Homebrew refuses to run as root, so nothing here ever asks
//! for a password.

use crate::command;
use crate::model::{Kind, Package, Role};
use serde_json::Value;
use std::path::Path;

/// Installed formulae. Nothing is read if Homebrew isn't installed.
pub fn load() -> Result<Vec<Package>, String> {
    let Some(prefix) = command::output("brew", &["--prefix"])? else { return Ok(Vec::new()) };
    let Some(text) = command::output("brew", &["info", "--json=v2", "--installed"])? else { return Ok(Vec::new()) };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("Homebrew sent invalid data: {e}"))?;
    Ok(parse(&v, Path::new(prefix.trim())))
}

pub fn parse(v: &Value, prefix: &Path) -> Vec<Package> {
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let Some(list) = v.get("formulae").and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .filter_map(|o| {
            let name = o.get("name")?.as_str()?;
            let installed = o.get("installed").and_then(Value::as_array).and_then(|a| a.last())?;
            let mut p = Package::new(Kind::Brew, name);
            p.role = Role::Tool;
            p.version = s(installed, "version");
            // pulled in for another formula: listed, but not as something the user chose
            p.user_installed = installed.get("installed_on_request").and_then(Value::as_bool).unwrap_or(true);
            p.summary = s(o, "desc");
            p.homepage = s(o, "homepage");
            p.channel = s(o, "tap");
            let dir = prefix.join("Cellar").join(name);
            p.location = dir.to_string_lossy().into_owned();
            p.paths = vec![dir];
            Some(p)
        })
        .collect()
}

/// `brew outdated --json=v2`: `(name, newest version)`.
pub fn parse_outdated(v: &Value) -> Vec<(String, String)> {
    v.get("formulae")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| Some((o.get("name")?.as_str()?.to_string(), o.get("current_version")?.as_str()?.to_string())))
        .collect()
}

pub fn outdated() -> Result<Vec<(String, String)>, String> {
    let Some(text) = command::output("brew", &["outdated", "--json=v2"])? else { return Ok(Vec::new()) };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("Homebrew sent invalid data: {e}"))?;
    Ok(parse_outdated(&v))
}

/// Installed formulae that need `name`, which Homebrew would refuse to remove it from under.
pub fn needed_by(name: &str) -> Result<Vec<String>, String> {
    let text = command::output("brew", &["uses", "--installed", "--", name])?.unwrap_or_default();
    Ok(text.split_whitespace().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulae_and_whether_they_were_asked_for() {
        let v: Value = serde_json::from_str(
            r#"{"formulae":[
            {"name":"gh","full_name":"gh","tap":"homebrew/core","desc":"GitHub command-line tool","homepage":"https://cli.github.com/",
             "installed":[{"version":"2.60.1","installed_as_dependency":false,"installed_on_request":true}]},
            {"name":"pcre2","tap":"homebrew/core","desc":"Perl compatible regular expressions","homepage":"https://www.pcre.org/",
             "installed":[{"version":"10.44","installed_as_dependency":true,"installed_on_request":false}]},
            {"name":"ghost","installed":[]}],"casks":[]}"#,
        )
        .unwrap();
        let p = parse(&v, Path::new("/home/linuxbrew/.linuxbrew"));
        assert_eq!(p.len(), 2, "a formula with nothing installed is skipped");
        assert!(p[0].user_installed && !p[1].user_installed);
        assert_eq!(p[0].paths, [Path::new("/home/linuxbrew/.linuxbrew/Cellar/gh").to_path_buf()]);
        assert_eq!(p[0].summary, "GitHub command-line tool");
    }

    #[test]
    fn outdated_formulae() {
        let v: Value = serde_json::from_str(
            r#"{"formulae":[{"name":"gh","installed_versions":["2.60.1"],"current_version":"2.61.0","pinned":false}],"casks":[]}"#,
        )
        .unwrap();
        assert_eq!(parse_outdated(&v), [("gh".to_string(), "2.61.0".to_string())]);
    }
}
