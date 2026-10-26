//! Packages in the user's Nix profile (`nix profile install`).

use crate::command;
use crate::model::{Kind, Package, Role};
use serde_json::Value;
use std::path::PathBuf;

/// `nix profile` is still marked experimental, so it has to be switched on for each call.
pub const FEATURES: [&str; 2] = ["--extra-experimental-features", "nix-command flakes"];

/// Installed packages. Nothing is read if Nix isn't installed.
pub fn load() -> Result<Vec<Package>, String> {
    let args = [FEATURES[0], FEATURES[1], "profile", "list", "--json"];
    let Some(text) = command::output("nix", &args)? else { return Ok(Vec::new()) };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("Nix sent invalid data: {e}"))?;
    Ok(parse(&v))
}

/// `/nix/store/<hash>-ripgrep-14.1.0` → `("ripgrep", "14.1.0")`: the version starts at the first
/// `-` followed by a digit.
fn name_version(store_path: &str) -> (String, String) {
    let base = store_path.rsplit('/').next().unwrap_or(store_path);
    let base = base.split_once('-').map_or(base, |(_, rest)| rest);
    let cut = base.char_indices().find(|&(i, c)| c == '-' && base[i + 1..].starts_with(|d: char| d.is_ascii_digit()));
    match cut {
        Some((i, _)) => (base[..i].to_string(), base[i + 1..].to_string()),
        None => (base.to_string(), String::new()),
    }
}

/// Newer Nix lists elements by name; older versions (profile format 2) as an array, removed by
/// their position.
pub fn parse(v: &Value) -> Vec<Package> {
    let entries: Vec<(String, &Value)> = match v.get("elements") {
        Some(Value::Object(m)) => m.iter().map(|(k, o)| (k.clone(), o)).collect(),
        Some(Value::Array(a)) => a.iter().enumerate().map(|(i, o)| (i.to_string(), o)).collect(),
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .filter(|(_, o)| o.get("active").and_then(Value::as_bool).unwrap_or(true))
        .map(|(id, o)| {
            let stores: Vec<PathBuf> = o
                .get("storePaths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
            let (store_name, version) = stores.first().map(|s| name_version(&s.to_string_lossy())).unwrap_or_default();
            let attr = o.get("attrPath").and_then(Value::as_str).unwrap_or("");
            let name = if id.parse::<usize>().is_ok() {
                attr.rsplit('.').next().filter(|s| !s.is_empty()).map_or(store_name, str::to_string)
            } else {
                id.clone()
            };
            let mut p = Package::new(Kind::Nix, id);
            p.name = name.clone();
            p.title = name;
            p.role = Role::Tool;
            p.user_installed = true;
            p.version = version;
            p.channel = o.get("originalUrl").and_then(Value::as_str).unwrap_or("").to_string();
            p.location = stores.first().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            p.paths = stores;
            p
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_paths_give_name_and_version() {
        assert_eq!(name_version("/nix/store/abc123-ripgrep-14.1.0"), ("ripgrep".into(), "14.1.0".into()));
        assert_eq!(
            name_version("/nix/store/abc123-python3.12-black-24.1.0"),
            ("python3.12-black".into(), "24.1.0".into())
        );
        assert_eq!(name_version("/nix/store/abc123-hello"), ("hello".into(), String::new()));
    }

    #[test]
    fn named_and_numbered_profiles() {
        let named: Value = serde_json::from_str(
            r#"{"elements":{"ripgrep":{"active":true,"attrPath":"legacyPackages.x86_64-linux.ripgrep","originalUrl":"flake:nixpkgs",
            "storePaths":["/nix/store/abc-ripgrep-14.1.0"]},"old":{"active":false,"storePaths":[]}},"version":3}"#,
        )
        .unwrap();
        let p = parse(&named);
        assert_eq!(p.len(), 1, "inactive elements aren't installed");
        assert_eq!((p[0].id.as_str(), p[0].version.as_str()), ("ripgrep", "14.1.0"));
        let numbered: Value = serde_json::from_str(
            r#"{"elements":[{"active":true,"attrPath":"legacyPackages.x86_64-linux.hello","storePaths":["/nix/store/abc-hello-2.12.1"]}],"version":2}"#,
        )
        .unwrap();
        let p = parse(&numbered);
        assert_eq!((p[0].id.as_str(), p[0].name.as_str()), ("0", "hello"));
    }
}
