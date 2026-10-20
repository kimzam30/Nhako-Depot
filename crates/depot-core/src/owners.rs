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
pub fn cached(
    cache: Option<&Path>,
    stamp: &str,
    wanted: &HashSet<PathBuf>,
    scan: impl FnOnce(&HashSet<PathBuf>) -> HashMap<PathBuf, String>,
) -> HashMap<PathBuf, String> {
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
    if !misses.is_empty() {
        let found = scan(&misses);
        for m in misses {
            let o = found.get(&m).cloned();
            known.insert(m, o);
        }
        if let Some(c) = cache {
            save(c, stamp, &known);
        }
    }
    wanted.iter().filter_map(|p| Some((p.clone(), known.get(p)?.clone()?))).collect()
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
    let tmp = c.with_extension("tmp");
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
        let first = cached(Some(&c), "v1", &wanted, lookup);
        assert_eq!(first.get(Path::new("/a")).map(String::as_str), Some("pkg"));
        assert!(!first.contains_key(Path::new("/b")));
        // same stamp: served from the cache, including the known miss
        let again = cached(Some(&c), "v1", &wanted, |_| panic!("should be cached"));
        assert_eq!(again, first);
        // new stamp: looked up again
        let fresh = cached(Some(&c), "v2", &wanted, |_| HashMap::new());
        assert!(fresh.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
