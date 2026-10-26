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

/// What a full lookup found, and why it couldn't look everywhere if it couldn't. Owners it did
/// find are right even then; only the paths it didn't find are in doubt.
pub type Found = (HashMap<PathBuf, String>, Option<String>);

/// Owners of `wanted` paths. `stamp` identifies the database state; answers cached under an
/// older stamp are thrown away. `scan` looks up the paths the cache doesn't know yet.
///
/// When `scan` couldn't look everywhere, the owners it found are kept but the paths it didn't
/// find stay unknown rather than being remembered as unowned (a locked database would otherwise
/// turn every system app into a manual install until the next install), and the error comes
/// back alongside the owners.
pub fn cached(
    cache: Option<&Path>,
    stamp: &str,
    wanted: &HashSet<PathBuf>,
    scan: impl FnOnce(&HashSet<PathBuf>) -> Found,
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
        let (mut found, e) = scan(&misses);
        for m in misses {
            match found.remove(&m) {
                Some(o) => {
                    known.insert(m, Some(o));
                }
                None if e.is_none() => {
                    known.insert(m, None);
                }
                None => {}
            }
        }
        if let Some(c) = cache {
            save(c, stamp, &known);
        }
        error = e;
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
    // per process and per call, so two scans at once (the app and `depot --list-only`, or a
    // refresh while a scan is still running) never write the same file
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = c.with_extension(format!("tmp{}-{n}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, c);
    }
}

/// Run `f` over `items` on a few threads and collect the results. Items `f` fails on, and parts
/// whose thread crashed, are left out, and the first such error comes back too.
pub fn parallel<T: Sync, R: Send>(
    items: &[T],
    f: impl Fn(&T) -> Result<Vec<R>, String> + Sync,
) -> (Vec<R>, Option<String>) {
    if items.is_empty() {
        return (Vec::new(), None);
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunk = items.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| {
                let f = &f;
                s.spawn(move || {
                    let mut out = Vec::new();
                    let mut error = None;
                    for item in part {
                        match f(item) {
                            Ok(v) => out.extend(v),
                            Err(e) => error = error.or(Some(e)),
                        }
                    }
                    (out, error)
                })
            })
            .collect();
        let mut out = Vec::new();
        let mut error = None;
        for h in handles {
            let (part, e) = h.join().unwrap_or_else(|_| (Vec::new(), Some("A file-list reader crashed.".into())));
            out.extend(part);
            error = error.or(e);
        }
        (out, error)
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
        let (first, _) = cached(Some(&c), "v1", &wanted, |w| (lookup(w), None));
        assert_eq!(first.get(Path::new("/a")).map(String::as_str), Some("pkg"));
        assert!(!first.contains_key(Path::new("/b")));
        // same stamp: served from the cache, including the known miss
        let (again, _) = cached(Some(&c), "v1", &wanted, |_| panic!("should be cached"));
        assert_eq!(again, first);
        // new stamp: looked up again
        let (fresh, _) = cached(Some(&c), "v2", &wanted, |_| (HashMap::new(), None));
        assert!(fresh.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_lookup_is_not_remembered() {
        let dir = std::env::temp_dir().join(format!("depot-owners-fail-{}", std::process::id()));
        let c = dir.join("cache");
        let wanted: HashSet<PathBuf> = [PathBuf::from("/a")].into();
        let (none, err) = cached(Some(&c), "v1", &wanted, |_| (HashMap::new(), Some("database is locked".into())));
        assert!(none.is_empty());
        assert_eq!(err.as_deref(), Some("database is locked"));
        // the next run asks again instead of trusting a cached "unowned"
        let (found, err) =
            cached(Some(&c), "v1", &wanted, |w| (w.iter().map(|p| (p.clone(), "pkg".into())).collect(), None));
        assert_eq!(found.get(Path::new("/a")).map(String::as_str), Some("pkg"));
        assert!(err.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_partial_lookup_keeps_what_it_found() {
        let dir = std::env::temp_dir().join(format!("depot-owners-partial-{}", std::process::id()));
        let c = dir.join("cache");
        let wanted: HashSet<PathBuf> = [PathBuf::from("/a"), PathBuf::from("/b")].into();
        let hit: HashMap<PathBuf, String> = [(PathBuf::from("/a"), "pkg".to_string())].into();
        let (found, err) = cached(Some(&c), "v1", &wanted, |_| (hit.clone(), Some("one list unreadable".into())));
        assert_eq!(found, hit);
        assert!(err.is_some());
        // the owner found is remembered; the path it didn't find is asked about again
        let (again, _) = cached(Some(&c), "v1", &wanted, |w| {
            assert_eq!(w, &[PathBuf::from("/b")].into());
            (HashMap::new(), None)
        });
        assert_eq!(again, hit);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parallel_reports_failures_and_crashes() {
        let items: Vec<u32> = (0..64).collect();
        let (out, err) = parallel(&items, |&i| if i == 7 { Err("seven".into()) } else { Ok(vec![i]) });
        assert_eq!(out.len(), 63);
        assert_eq!(err.as_deref(), Some("seven"));
        let (_, err) = parallel(&items, |&i| if i == 9 { panic!("boom") } else { Ok(vec![i]) });
        assert!(err.is_some(), "a crashed reader is an error, not \"no owner\"");
    }
}
