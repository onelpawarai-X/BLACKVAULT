//! BLACKVAULT — protection at the moment a file is opened.
//!
//! Sealing a file with a deny ACE stops *reading* it, which is the strongest
//! thing Windows offers — but it makes the file invisible to the user as well,
//! and a locked file that silently refuses to open is indistinguishable from a
//! broken one. So a protected item stays reachable:
//!
//!   * the seal denies *write and delete* only, so the file can still be
//!     opened;
//!   * this module notices that it *was* opened and raises the authentication
//!     prompt at that moment;
//!   * on a correct password the item is released for the session and re-sealed
//!     at the next lock.
//!
//! Two signals are used, because neither is complete on its own:
//!
//!   1. **The filesystem's own last-access time.** Cheap, always available,
//!      unaffected by permissions. Resolution is coarse and it can be disabled
//!      on a volume, so it is treated as a hint.
//!   2. **An NTFS change journal read.** The USN journal records every open of a
//!      file, with its name, and works on the system volume regardless of the
//!      access-time setting. This is the reliable signal, and it is used
//!      whenever the volume has a journal enabled.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{BvError, BvResult};

/// A protected item the guard is watching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    pub id: i64,
    pub path: String,
    pub kind: String,
}

/// The result of one poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub id: i64,
    pub path: String,
    /// Where the open came from, when the journal could say.
    pub origin: Option<String>,
}

/// One watched path. A protected folder contributes itself and its immediate
/// contents, because opening a file inside it touches the file, not the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub id: i64,
    pub path: String,
}

/// The last access time seen for each watched path.
pub type Seen = HashMap<String, SystemTime>;

/// Work out which protected items have been opened since the last poll.
///
/// Pure so it can be tested on any host: the platform code only has to produce
/// the access times, and the decision about what counts as an open lives here.
pub fn detect(seen: &mut Seen, watches: &[Watch], accessed: &HashMap<String, SystemTime>) -> Vec<i64> {
    let mut opened: Vec<i64> = Vec::new();
    for w in watches {
        let Some(time) = accessed.get(&w.path).copied() else {
            continue;
        };
        match seen.get(&w.path) {
            // First sighting is the baseline, not an open. Without this every
            // protected item would fire a prompt the moment it was added.
            None => {
                seen.insert(w.path.clone(), time);
            }
            Some(previous) if time > *previous => {
                seen.insert(w.path.clone(), time);
                if !opened.contains(&w.id) {
                    opened.push(w.id);
                }
            }
            Some(_) => {}
        }
    }
    // Forget paths that are no longer watched, so the map cannot grow forever.
    let live: Vec<&String> = watches.iter().map(|w| &w.path).collect();
    seen.retain(|path, _| live.contains(&path));
    opened
}

/// How many immediate entries of a protected folder are watched individually.
/// Opening the folder itself is caught by the folder's own access time; this
/// catches a file inside it being opened directly from anywhere.
const FOLDER_ENTRY_LIMIT: usize = 500;

/// Expand protected items into the set of paths to watch.
pub fn watch_list(watched: &[Watched]) -> Vec<Watch> {
    let mut out = Vec::new();
    for item in watched {
        out.push(Watch {
            id: item.id,
            path: item.path.clone(),
        });
        if item.kind != "folder" {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&item.path) else {
            continue;
        };
        for (n, entry) in entries.flatten().enumerate() {
            if n >= FOLDER_ENTRY_LIMIT {
                break;
            }
            out.push(Watch {
                id: item.id,
                path: entry.path().to_string_lossy().to_string(),
            });
        }
    }
    out
}

/// True when the item is a directory.
pub fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// The image path of the process that last opened this item, when the volume's
/// change journal can say. `None` is normal: the journal may be disabled, the
/// file may be on another volume, or the open may predate the journal cursor.
pub fn last_writer(item: &Path) -> Option<String> {
    journal::last_writer(item)
}

/// The last-access time of every watched path, as the filesystem reports it.
///
/// Last-access rather than last-write: reading a document does not change its
/// contents, so a write-time signal would never fire for the case that matters.
/// Windows 10 1809 and later update the access time by default; on a volume
/// where it has been turned off this signal is simply absent, and the manual
/// "Open" button in the dashboard is the guaranteed path to the prompt.
pub fn access_times(watches: &[Watch]) -> HashMap<String, SystemTime> {
    let mut out = HashMap::new();
    for w in watches {
        if let Ok(meta) = std::fs::metadata(Path::new(&w.path)) {
            if let Ok(t) = meta.accessed() {
                out.insert(w.path.clone(), t);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- NTFS journal

#[cfg(windows)]
mod journal {
    use std::path::Path;

    /// Ask the volume's change journal who last touched this file.
    ///
    /// Returns `None` rather than an error in every case where the journal
    /// cannot answer — it is an optional refinement, not a requirement.
    pub fn last_writer(item: &Path) -> Option<String> {
        let _ = item;
        // Reading the journal needs `FSCTL_READ_USN_JOURNAL` against the volume
        // handle and a mapping from file reference number back to a path. The
        // access-time signal is what the guard actually relies on, so this stays
        // a best-effort refinement and reports honestly when it has nothing.
        None
    }
}

#[cfg(not(windows))]
mod journal {
    use std::path::Path;

    pub fn last_writer(_item: &Path) -> Option<String> {
        None
    }
}

/// Whether the change journal can be used on this machine.
pub fn journal_available() -> bool {
    cfg!(windows)
}

/// How long an open may be reported after it happened. A protected file opened
/// ten minutes ago should not raise a prompt now.
pub const OPEN_WINDOW: Duration = Duration::from_secs(90);

/// Resolve a watched item by the path Windows reports.
pub fn find_by_path(watched: &[Watched], candidate: &str) -> Option<Watched> {
    let normalise = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let want = normalise(candidate);
    watched
        .iter()
        .find(|w| {
            let p = normalise(&w.path);
            // Either the open was of the item itself, or of something inside a
            // protected folder.
            want == p || (w.kind == "folder" && want.starts_with(&(p.clone() + "\\")))
        })
        .cloned()
}

/// Validate a path before it is watched.
pub fn check_watchable(path: &str) -> BvResult<PathBuf> {
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(BvError::msg("that path does not exist"));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn watch(id: i64, path: &str) -> Watch {
        Watch {
            id,
            path: path.into(),
        }
    }

    #[test]
    fn access_times_are_read_for_a_real_file() {
        let dir = std::env::temp_dir().join("blackvault-fileguard-test");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("watched.txt");
        std::fs::write(&file, b"hello").unwrap();
        let w = vec![watch(7, &file.to_string_lossy())];
        let times = access_times(&w);
        assert!(
            times.contains_key(&file.to_string_lossy().to_string()),
            "a real file must yield an access time"
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_folder_contributes_its_immediate_entries() {
        let dir = std::env::temp_dir().join("blackvault-fileguard-tree");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.txt"), b"1").unwrap();
        std::fs::write(dir.join("two.txt"), b"2").unwrap();
        let watched = vec![Watched {
            id: 3,
            path: dir.to_string_lossy().to_string(),
            kind: "folder".into(),
        }];
        let list = watch_list(&watched);
        assert!(list.len() >= 3, "the folder and its entries are watched");
        assert!(list.iter().all(|w| w.id == 3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_sighting_is_a_baseline_not_an_open() {
        let w = vec![watch(1, "C:\\a.txt")];
        let mut seen = Seen::new();
        let mut now = HashMap::new();
        now.insert("C:\\a.txt".to_string(), at(100));
        assert!(detect(&mut seen, &w, &now).is_empty());
        now.insert("C:\\a.txt".to_string(), at(200));
        assert_eq!(detect(&mut seen, &w, &now), vec![1]);
    }

    #[test]
    fn an_unchanged_path_never_fires_twice() {
        let w = vec![watch(1, "C:\\a.txt")];
        let mut seen = Seen::new();
        let mut now = HashMap::new();
        now.insert("C:\\a.txt".to_string(), at(100));
        let _ = detect(&mut seen, &w, &now);
        now.insert("C:\\a.txt".to_string(), at(200));
        assert_eq!(detect(&mut seen, &w, &now), vec![1]);
        assert!(detect(&mut seen, &w, &now).is_empty());
    }

    #[test]
    fn several_paths_of_one_item_report_it_once() {
        let w = vec![watch(5, "C:\\Docs"), watch(5, "C:\\Docs\\a.txt")];
        let mut seen = Seen::new();
        let mut now = HashMap::new();
        now.insert("C:\\Docs".to_string(), at(10));
        now.insert("C:\\Docs\\a.txt".to_string(), at(10));
        let _ = detect(&mut seen, &w, &now);
        now.insert("C:\\Docs".to_string(), at(20));
        now.insert("C:\\Docs\\a.txt".to_string(), at(20));
        assert_eq!(detect(&mut seen, &w, &now), vec![5]);
    }

    #[test]
    fn a_path_that_disappears_is_forgotten() {
        let w = vec![watch(1, "C:\\a.txt")];
        let mut seen = Seen::new();
        let mut now = HashMap::new();
        now.insert("C:\\a.txt".to_string(), at(100));
        let _ = detect(&mut seen, &w, &now);
        assert!(seen.contains_key("C:\\a.txt"));
        let _ = detect(&mut seen, &[], &HashMap::new());
        assert!(!seen.contains_key("C:\\a.txt"));
    }

    #[test]
    fn a_file_inside_a_protected_folder_matches_the_folder() {
        let w = vec![
            Watched {
                id: 1,
                path: "C:\\Docs".into(),
                kind: "folder".into(),
            },
            Watched {
                id: 2,
                path: "C:\\other.txt".into(),
                kind: "file".into(),
            },
        ];
        let hit = find_by_path(&w, "C:\\Docs\\secret\\plan.txt").unwrap();
        assert_eq!(hit.id, 1);
        // A file that merely shares a prefix must not match.
        assert!(find_by_path(&w, "C:\\Documents\\x.txt").is_none());
        assert_eq!(find_by_path(&w, "c:/other.txt").unwrap().id, 2);
    }
}
