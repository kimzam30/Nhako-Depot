//! Programs installed with `cargo install`, read from cargo's own record of them
//! (`~/.cargo/.crates2.json`).

use crate::command;
use crate::model::{Kind, Package, Role};
use serde_json::Value;
use std::path::{Path, PathBuf};

const RECORD: &str = ".crates2.json";
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// The cargo folder: `CARGO_HOME`, or `~/.cargo`.
pub fn home_dir(home: &Path) -> PathBuf {
    std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cargo"))
}

/// Installed programs. Nothing is read if cargo has never installed one.
pub fn load(cargo_home: &Path) -> Result<Vec<Package>, String> {
    let text = match std::fs::read_to_string(cargo_home.join(RECORD)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Couldn't read {RECORD}: {e}")),
    };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{RECORD} isn't valid: {e}"))?;
    Ok(parse(&v, cargo_home))
}

/// `name version (source)`, the key of each install.
fn split_key(key: &str) -> Option<(&str, &str, &str)> {
    let (name, rest) = key.split_once(' ')?;
    let (version, source) = rest.split_once(' ')?;
    Some((name, version, source.trim_start_matches('(').trim_end_matches(')')))
}

pub fn parse(v: &Value, cargo_home: &Path) -> Vec<Package> {
    let Some(installs) = v.get("installs").and_then(Value::as_object) else { return Vec::new() };
    let bin = cargo_home.join("bin");
    installs
        .iter()
        .filter_map(|(key, o)| {
            let (name, version, source) = split_key(key)?;
            let mut p = Package::new(Kind::Cargo, name);
            p.role = Role::Tool;
            p.user_installed = true;
            p.version = version.to_string();
            p.channel = if source == CRATES_IO {
                p.homepage = format!("https://crates.io/crates/{name}");
                "crates.io".into()
            } else if let Some(git) = source.strip_prefix("git+") {
                git.split(['?', '#']).next().unwrap_or(git).to_string()
            } else if source.starts_with("path+") {
                "A folder on this computer".into()
            } else {
                source.to_string()
            };
            let bins: Vec<&str> =
                o.get("bins").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
            p.summary = match bins.as_slice() {
                [] => String::new(),
                [one] => format!("Command: {one}"),
                many => format!("Commands: {}", many.join(", ")),
            };
            p.paths = bins.iter().map(|b| bin.join(b)).collect();
            p.size = p.paths.iter().filter_map(|b| std::fs::metadata(b).ok()).map(|m| m.len()).sum();
            p.location = bin.to_string_lossy().into_owned();
            Some(p)
        })
        .collect()
}

/// Whether `name` came from crates.io, so a newer version can be looked up there.
pub fn from_crates_io(cargo_home: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(cargo_home.join(RECORD)) else { return false };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return false };
    v.get("installs")
        .and_then(Value::as_object)
        .is_some_and(|m| m.keys().filter_map(|k| split_key(k)).any(|(n, _, s)| n == name && s == CRATES_IO))
}

/// The arguments that install the newest version of `name` the way it was installed before,
/// with the same features.
pub fn reinstall_args(cargo_home: &Path, name: &str) -> Vec<String> {
    let mut args = vec!["cargo".to_string(), "install".into()];
    let text = std::fs::read_to_string(cargo_home.join(RECORD)).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    let entry = v
        .get("installs")
        .and_then(Value::as_object)
        .and_then(|m| m.iter().find(|(k, _)| split_key(k).is_some_and(|(n, _, _)| n == name)).map(|(_, o)| o));
    if let Some(o) = entry {
        let flag = |k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
        if flag("all_features") {
            args.push("--all-features".into());
        }
        if flag("no_default_features") {
            args.push("--no-default-features".into());
        }
        let features: Vec<&str> =
            o.get("features").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        if !features.is_empty() {
            args.push("--features".into());
            args.push(features.join(","));
        }
    }
    args.push("--".into());
    args.push(name.to_string());
    args
}

/// Programs from crates.io with a newer version there, as `(name, newest version)`, from the
/// installed `(name, version)` list. Asks crates.io once per program.
pub fn outdated(cargo_home: &Path, installed: &[(String, String)]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (name, version) in installed.iter().filter(|(n, _)| from_crates_io(cargo_home, n)) {
        // the exact crate isn't always the first result
        let Some(text) = command::output("cargo", &["search", "--limit", "5", name.as_str()])? else { break };
        if let Some(latest) = parse_search(&text, name).filter(|l| crate::updates::newer(l, version)) {
            out.push((name.clone(), latest));
        }
    }
    Ok(out)
}

/// The newest version on crates.io, from `cargo search` output (`name = "1.2.3"    # ...`).
pub fn parse_search(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (n, rest) = l.split_once(" = \"")?;
        (n.trim() == name).then(|| rest.split('"').next().unwrap_or("").to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"installs":{
        "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["rg"],"features":[],"all_features":false,"no_default_features":false},
        "cargo-ndk 4.1.2 (registry+https://github.com/rust-lang/crates.io-index)":{"bins":["cargo-ndk","cargo-ndk-env"],"features":["x","y"],"all_features":false,"no_default_features":true},
        "mytool 0.1.0 (git+https://github.com/me/mytool?branch=main#abc123)":{"bins":["mytool"]}
    }}"#;

    #[test]
    fn installs_are_listed_with_their_commands() {
        let v: Value = serde_json::from_str(SAMPLE).unwrap();
        let home = Path::new("/home/u/.cargo");
        let mut p = parse(&v, home);
        p.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["cargo-ndk", "mytool", "ripgrep"]);
        let rg = &p[2];
        assert_eq!((rg.version.as_str(), rg.channel.as_str()), ("14.1.0", "crates.io"));
        assert_eq!(rg.paths, [home.join("bin/rg")]);
        assert_eq!(rg.summary, "Command: rg");
        assert_eq!(p[1].channel, "https://github.com/me/mytool");
        assert_eq!(p[0].summary, "Commands: cargo-ndk, cargo-ndk-env");
    }

    #[test]
    fn reinstalling_keeps_the_features() {
        let dir = std::env::temp_dir().join(format!("depot-cargo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(RECORD), SAMPLE).unwrap();
        assert_eq!(
            reinstall_args(&dir, "cargo-ndk"),
            ["cargo", "install", "--no-default-features", "--features", "x,y", "--", "cargo-ndk"]
        );
        assert!(from_crates_io(&dir, "ripgrep") && !from_crates_io(&dir, "mytool"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_finds_the_exact_crate() {
        let out = "ripgrep_all = \"0.10.6\"    # rga\nripgrep = \"14.1.1\"    # fast grep\n";
        assert_eq!(parse_search(out, "ripgrep").as_deref(), Some("14.1.1"));
        assert_eq!(parse_search(out, "nope"), None);
    }
}
