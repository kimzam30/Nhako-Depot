//! Which package owns which file, cached on disk until the package database changes.
//! Each native backend supplies a slow full lookup; this keeps the answers between runs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Modification time in nanoseconds, 0 if unknown. Package databases change it on every install.
pub fn mtime_ns(p: &Path) -> u128 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// Owners of `wanted` paths. `stamp` identifies the database state; answers cached under an
/// older stamp are thrown away. `scan` looks up the paths the cache doesn't know yet.
///
/// When `scan` fails, the paths it was asked about stay unknown rather than being remembered as
/// unowned (a locked database would otherwise turn every system app into a manual install until
/// the next install), and the error comes back alongside the owners that were already known.
pub fn cached(
    cache: Option<&Path>,
    stamp: &str,
    wanted: &HashSet<PathBuf>,
    scan: impl FnOnce(&HashSet<PathBuf>) -> Result<HashMap<PathBuf, String>, String>,
) -> (HashMap<PathBuf, String>, Option<String>) {
    let mut known: HashMap<PathBuf, Option<String>> = HashMap::new();
    if let Some(text) = cache.and_then(|c| std::fs::read_to_string(c).ok()) {
        let mut lines = text.lines();
        if lines.next() == Some(stamp) {
            for l in lines {
                if let Some((p, o)) = l.split_once('\t') {
                    known.insert(PathBuf::from(p), (!o.is_empty()).then(|| o.to_string()));
                }
            }
        }
    }
    let misses: HashSet<PathBuf> = wanted.iter().filter(|p| !known.contains_key(*p)).cloned().collect();
    let mut error = None;
    if !misses.is_empty() {
        match scan(&misses) {
            Ok(found) => {
                for m in misses {
                    let o = found.get(&m).cloned();
                    known.insert(m, o);
                }
                if let Some(c) = cache {
                    save(c, stamp, &known);
                }
            }
            Err(e) => error = Some(e),
        }
    }
    (wanted.iter().filter_map(|p| Some((p.clone(), known.get(p)?.clone()?))).collect(), error)
}

/// One owner per path from a scan's raw hits. Directories and diverted files can appear in
/// several packages' lists, so the pick is made stable.
pub fn first_owner(mut found: Vec<(PathBuf, String)>) -> HashMap<PathBuf, String> {
    found.sort();
    let mut out = HashMap::new();
    for (p, o) in found {
        out.entry(p).or_insert(o);
    }
    out
}

fn save(c: &Path, stamp: &str, known: &HashMap<PathBuf, Option<String>>) {
    use std::fmt::Write;
    let mut text = format!("{stamp}\n");
    for (p, o) in known {
        let s = p.to_string_lossy();
        if !s.contains(['\t', '\n']) {
            let _ = writeln!(text, "{s}\t{}", o.as_deref().unwrap_or(""));
        }
    }
    if let Some(dir) = c.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // per process, so the app and `depot --list-only` running at once don't write the same file
    let tmp = c.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, c);
    }
}

/// Run `f` over `items` on a few threads and collect the results.
pub fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> Vec<R> + Sync) -> Vec<R> {
    if items.is_empty() {
        return Vec::new();
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunk = items.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> =
            items.chunks(chunk).map(|part| s.spawn(|| part.iter().flat_map(&f).collect::<Vec<R>>())).collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_answers_until_the_stamp_changes() {
        let dir = std::env::temp_dir().join(format!("depot-owners-{}", std::process::id()));
        let c = dir.join("cache");
        let wanted: HashSet<PathBuf> = [PathBuf::from("/a"), PathBuf::from("/b")].into();
        let lookup = |w: &HashSet<PathBuf>| {
            w.iter().filter(|p| *p == Path::new("/a")).map(|p| (p.clone(), "pkg".to_string())).collect()
        };
        let (first, _) = cached(Some(&c), "v1", &wanted, |w| Ok(lookup(w)));
        assert_eq!(first.get(Path::new("/a")).map(String::as_str), Some("pkg"));
        assert!(!first.contains_key(Path::new("/b")));
        // same stamp: served from the cache, including the known miss
        let (again, _) = cached(Some(&c), "v1", &wanted, |_| panic!("should be cached"));
        assert_eq!(again, first);
        // new stamp: looked up again
        let (fresh, _) = cached(Some(&c), "v2", &wanted, |_| Ok(HashMap::new()));
        assert!(fresh.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_lookup_is_not_remembered() {
        let dir = std::env::temp_dir().join(format!("depot-owners-fail-{}", std::process::id()));
        let c = dir.join("cache");
        let wanted: HashSet<PathBuf> = [PathBuf::from("/a")].into();
        let (none, err) = cached(Some(&c), "v1", &wanted, |_| Err("database is locked".into()));
        assert!(none.is_empty());
        assert_eq!(err.as_deref(), Some("database is locked"));
        // the next run asks again instead of trusting a cached "unowned"
        let (found, err) =
            cached(Some(&c), "v1", &wanted, |w| Ok(w.iter().map(|p| (p.clone(), "pkg".into())).collect()));
        assert_eq!(found.get(Path::new("/a")).map(String::as_str), Some("pkg"));
        assert!(err.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
