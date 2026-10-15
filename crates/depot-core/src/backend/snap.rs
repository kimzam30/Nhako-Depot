//! Snaps, read from the snapd REST API over its Unix socket.

use crate::model::{Kind, Package, Role};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

pub const SOCKET: &str = "/run/snapd.socket";
pub const DESKTOP_DIR: &str = "/var/lib/snapd/desktop/applications";
const PROTECTED_TYPES: &[&str] = &["base", "os", "snapd", "core", "kernel", "gadget"];

/// Minimal HTTP/1.0 GET. snapd answers 1.0 requests without chunking and closes the connection.
fn get(socket: &Path, path: &str) -> Result<Vec<u8>, String> {
    let mut s = UnixStream::connect(socket).map_err(|e| format!("snapd isn't reachable: {e}"))?;
    let t = Some(Duration::from_secs(3));
    let _ = s.set_read_timeout(t);
    let _ = s.set_write_timeout(t);
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| format!("snapd didn't answer: {e}"))?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("snapd sent a malformed reply")?;
    let head = String::from_utf8_lossy(&buf[..split]);
    if !head.lines().next().is_some_and(|l| l.contains(" 200 ")) {
        return Err(format!("snapd replied: {}", head.lines().next().unwrap_or("")));
    }
    Ok(buf[split + 4..].to_vec())
}

pub fn load(socket: &Path) -> Result<Vec<Package>, String> {
    if !socket.exists() {
        return Ok(Vec::new());
    }
    let body = get(socket, "/v2/snaps")?;
    let v: Value = serde_json::from_slice(&body).map_err(|e| format!("snapd sent invalid data: {e}"))?;
    Ok(parse(&v))
}

pub fn parse(v: &Value) -> Vec<Package> {
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let Some(list) = v.get("result").and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .filter_map(|o| {
            let name = o.get("name")?.as_str()?;
            let typ = o.get("type").and_then(Value::as_str).unwrap_or("app");
            let mut p = Package::new(Kind::Snap, name);
            let title = s(o, "title");
            if !title.is_empty() {
                p.title = title;
            }
            p.version = s(o, "version");
            p.size = o.get("installed-size").and_then(Value::as_u64).unwrap_or(0);
            p.summary = s(o, "summary");
            p.description = s(o, "description");
            p.homepage = s(o, "website");
            p.channel = s(o, "tracking-channel");
            p.revision = match o.get("revision") {
                Some(Value::String(r)) => r.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            };
            if let Some(pubr) = o.get("publisher") {
                p.publisher = s(pubr, "display-name");
                if matches!(pubr.get("validation").and_then(Value::as_str), Some("verified" | "starred")) {
                    p.publisher.push_str(" ✔");
                }
            }
            p.section = typ.to_string();
            if PROTECTED_TYPES.contains(&typ) {
                p.role = Role::Runtime;
                p.blocked = Some("This is a core snap that other snaps and snapd depend on.".into());
            } else if o.get("apps").and_then(Value::as_array).is_none_or(|a| a.is_empty()) {
                // content snaps (themes, GPU drivers, GNOME platform) have no programs of their own
                p.role = Role::Runtime;
            } else {
                p.user_installed = true;
            }
            Some(p)
        })
        .collect()
}

/// Icon shipped inside the snap, for snaps whose launcher has none.
pub fn bundled_icon(name: &str) -> Option<String> {
    ["png", "svg"].iter().map(|ext| format!("/snap/{name}/current/meta/gui/icon.{ext}")).find(|p| Path::new(p).exists())
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_snaps() {
        let v = serde_json::json!({"result": [
            {"name": "core22", "type": "base", "version": "1", "revision": "100"},
            {"name": "mesa-2404", "type": "app"},
            {"name": "spotify", "type": "app", "title": "Spotify", "installed-size": 10, "apps": [{"name": "spotify"}],
             "publisher": {"display-name": "Spotify", "validation": "verified"}, "revision": 80}
        ]});
        let p = super::parse(&v);
        assert!(p[0].blocked.is_some());
        assert_eq!(p[1].role, super::Role::Runtime);
        assert!(p[1].blocked.is_none());
        assert_eq!(p[2].title, "Spotify");
        assert_eq!(p[2].publisher, "Spotify ✔");
        assert_eq!(p[2].revision, "80");
    }
}
