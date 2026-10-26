//! Minimal reader for freedesktop `.desktop` launchers.
//! Only the `[Desktop Entry]` group and the keys Depot needs are read.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct DesktopEntry {
    pub path: PathBuf,
    /// Desktop file ID, e.g. `org.gnome.Nautilus.desktop`.
    pub id: String,
    pub name: String,
    pub generic_name: String,
    pub comment: String,
    /// Raw `Exec` value; use [`DesktopEntry::argv`] to split it.
    pub exec: String,
    /// Program that must exist for the launcher to be shown.
    pub try_exec: String,
    pub icon: String,
    pub is_application: bool,
    pub hidden: bool,
    pub no_display: bool,
    pub only_show_in: Vec<String>,
    pub not_show_in: Vec<String>,
}

/// Locale suffixes to prefer for `Name[xx]`, best first.
fn locale_suffixes() -> &'static [String] {
    static L: OnceLock<Vec<String>> = OnceLock::new();
    L.get_or_init(|| {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let raw = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| var(k)).unwrap_or_default();
        preferred(&raw, var("LANGUAGE").as_deref())
    })
}

/// Locale suffixes to try for locale `raw`, after GNU `LANGUAGE`'s priority list (`de:fr`) if it's
/// set; as with gettext, `LANGUAGE` counts only when the locale isn't plain C.
fn preferred(raw: &str, language: Option<&str>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if !locale_order(raw).is_empty() {
        for l in language.unwrap_or("").split(':') {
            v.extend(locale_order(l));
        }
    }
    for l in locale_order(raw) {
        if !v.contains(&l) {
            v.push(l);
        }
    }
    v.dedup();
    v
}

/// `sr_RS.UTF-8@latin` → `sr_RS@latin`, `sr_RS`, `sr@latin`, `sr`: the order the Desktop Entry
/// spec matches `Name[…]` keys in.
fn locale_order(raw: &str) -> Vec<String> {
    let (main, modifier) = match raw.split_once('@') {
        Some((m, md)) => (m, Some(md)),
        None => (raw, None),
    };
    let base = main.split('.').next().unwrap_or("");
    if base.is_empty() || base == "C" || base == "POSIX" {
        return Vec::new();
    }
    let lang = base.split_once('_').map(|(l, _)| l);
    let mut v = Vec::new();
    if let Some(m) = modifier {
        v.push(format!("{base}@{m}"));
    }
    v.push(base.to_string());
    if let Some(l) = lang {
        if let Some(m) = modifier {
            v.push(format!("{l}@{m}"));
        }
        v.push(l.to_string());
    }
    v
}

/// Launchers bigger than this aren't read.
const MAX_SIZE: u64 = 256 * 1024;

fn list(v: &str) -> Vec<String> {
    v.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

impl DesktopEntry {
    /// The id without `.desktop`: `org.gnome.Nautilus`.
    pub fn base_id(&self) -> &str {
        self.id.strip_suffix(".desktop").unwrap_or(&self.id)
    }

    pub fn load(path: &Path, id: String) -> Option<Self> {
        // a launcher is a small text file; a link to a pipe or device would hang the scan
        let m = std::fs::metadata(path).ok()?;
        if !m.is_file() || m.len() > MAX_SIZE {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        Some(Self::parse(&String::from_utf8_lossy(&bytes), path.to_path_buf(), id))
    }

    pub fn parse(text: &str, path: PathBuf, id: String) -> Self {
        let langs = locale_suffixes();
        let mut e = DesktopEntry { path, id, ..Default::default() };
        // rank of the value currently held for Name / GenericName / Comment; lower is better
        let mut ranks = [usize::MAX; 3];
        let mut in_main = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_main = line == "[Desktop Entry]";
                continue;
            }
            if !in_main {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let (key, loc) = match k.split_once('[') {
                Some((b, rest)) => (b, rest.strip_suffix(']')),
                None => (k, None),
            };
            let rank = match loc {
                None => langs.len(),
                Some(l) => match langs.iter().position(|x| x == l) {
                    Some(i) => i,
                    None => continue,
                },
            };
            let slot = match key {
                "Name" => Some((&mut e.name, 0)),
                "GenericName" => Some((&mut e.generic_name, 1)),
                "Comment" => Some((&mut e.comment, 2)),
                _ => None,
            };
            if let Some((field, i)) = slot {
                if rank < ranks[i] {
                    ranks[i] = rank;
                    *field = unescape(v);
                }
                continue;
            }
            if loc.is_some() {
                continue;
            }
            match key {
                "Exec" => e.exec = v.to_string(),
                "TryExec" => e.try_exec = unescape(v),
                "Icon" => e.icon = unescape(v),
                "Type" => e.is_application = v == "Application",
                "Hidden" => e.hidden = v == "true",
                "NoDisplay" => e.no_display = v == "true",
                "OnlyShowIn" => e.only_show_in = list(v),
                "NotShowIn" => e.not_show_in = list(v),
                _ => {}
            }
        }
        e
    }

    /// Whether the launcher appears in the app menu of the given desktops.
    pub fn shown(&self, desktops: &[String]) -> bool {
        if !self.is_application || self.hidden || self.no_display {
            return false;
        }
        if !self.only_show_in.is_empty() && !self.only_show_in.iter().any(|d| desktops.contains(d)) {
            return false;
        }
        !self.not_show_in.iter().any(|d| desktops.contains(d))
    }

    pub fn argv(&self) -> Vec<String> {
        split_exec(&self.exec)
    }
}

/// Undo the string-level escapes of the desktop entry spec.
pub fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split an `Exec` value into arguments, following the spec's quoting rules,
/// and drop field codes such as `%U`.
pub fn split_exec(exec: &str) -> Vec<String> {
    let s = unescape(exec);
    let mut args = Vec::new();
    let mut cur = String::new();
    let (mut in_arg, mut quoted) = (false, false);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if quoted {
            match c {
                '"' => quoted = false,
                '\\' => match it.peek() {
                    Some(&n @ ('"' | '`' | '$' | '\\')) => {
                        cur.push(n);
                        it.next();
                    }
                    _ => cur.push('\\'),
                },
                _ => cur.push(c),
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                in_arg = true;
            }
            ' ' | '\t' => {
                if in_arg {
                    args.push(std::mem::take(&mut cur));
                    in_arg = false;
                }
            }
            _ => {
                cur.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(cur);
    }
    args.into_iter()
        .filter_map(|a| match a.as_str() {
            "%%" => Some("%".to_string()),
            _ if a.len() == 2 && a.starts_with('%') => None,
            _ => Some(a.replace("%%", "%")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_modifiers() {
        assert_eq!(locale_order("sr_RS.UTF-8@latin"), ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]);
        assert_eq!(locale_order("de_DE.UTF-8"), ["de_DE", "de"]);
        assert!(locale_order("C.UTF-8").is_empty());
        assert_eq!(preferred("de_DE.UTF-8", Some("fr:de")), ["fr", "de", "de_DE"]);
        assert!(preferred("C", Some("fr")).is_empty(), "LANGUAGE is ignored in the C locale");
    }

    #[test]
    fn exec_quoting_and_field_codes() {
        assert_eq!(
            split_exec(r#""/home/u/.local/bin/claude" --handle-uri %u"#),
            ["/home/u/.local/bin/claude", "--handle-uri"]
        );
        assert_eq!(split_exec(r#"sh -c "echo \\"hi\\"" %F"#), ["sh", "-c", r#"echo "hi""#]);
        assert_eq!(split_exec("app 100%%"), ["app", "100%"]);
        assert_eq!(split_exec("a\\sb c"), ["a", "b", "c"]);
    }

    #[test]
    fn only_main_group_is_read() {
        let e = DesktopEntry::parse(
            "[Desktop Entry]\nType=Application\nName=Steam\nExec=/usr/games/steam %U\n\n[Desktop Action Store]\nExec=/usr/games/steam steam://store\n",
            PathBuf::from("/x/steam.desktop"),
            "steam.desktop".into(),
        );
        assert_eq!(e.exec, "/usr/games/steam %U");
        assert!(e.shown(&[]));
    }

    #[test]
    fn hidden_and_show_in() {
        let mut e = DesktopEntry::parse(
            "[Desktop Entry]\nType=Application\nName=X\nOnlyShowIn=KDE;\n",
            PathBuf::new(),
            "x".into(),
        );
        assert!(!e.shown(&["GNOME".into()]));
        assert!(e.shown(&["KDE".into()]));
        e.hidden = true;
        assert!(!e.shown(&["KDE".into()]));
    }
}
