//! Local JSON document store for the desktop build.
//!
//! The web build keeps its data behind `window.storage`, an async key/value API. This is the
//! desktop counterpart: one JSON file per key inside the app's data directory.
//!
//! Two properties matter more than speed here, because this holds the user's only copy of
//! their plans:
//!
//! * **Writes are atomic.** The payload goes to a temp file which is flushed to disk and only
//!   then renamed over the real one. A crash or a pulled power cord can lose the *new*
//!   content but can never leave a half-written file in its place — which, for a portable app
//!   living on a USB stick, is a realistic failure to design against.
//! * **A missing key is not an error.** The web API used to raise on a key that had never been
//!   written, which cost a long debugging detour. Here it is simply `Ok(None)`.
//!
//! Every write also rotates a small number of timestamped backups, so a bad import or a
//! mistaken bulk edit is recoverable without any cloud service.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// How many previous versions of each document to keep.
pub const BACKUP_KEEP: usize = 10;

#[derive(Debug)]
pub enum StoreError {
    /// The key contained characters that could escape the data directory.
    BadKey(String),
    Io(std::io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::BadKey(k) => write!(f, "无效的存储键名：{}", k),
            StoreError::Io(e) => write!(f, "文件读写失败：{}", e),
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A key becomes a file name, so it must not be able to walk out of the data directory.
/// Only the shapes the app actually uses are allowed: letters, digits, `_`, `-` and `.`,
/// and never a `..` sequence.
fn safe_key(key: &str) -> Result<&str> {
    let ok = !key.is_empty()
        && key.len() <= 128
        && !key.contains("..")
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if ok {
        Ok(key)
    } else {
        Err(StoreError::BadKey(key.to_string()))
    }
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Opens (creating if needed) a store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        fs::create_dir_all(root.join("backups"))?;
        Ok(Store { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn doc_path(&self, key: &str) -> Result<PathBuf> {
        Ok(self.root.join(format!("{}.json", safe_key(key)?)))
    }

    /// Returns the stored text, or `None` when nothing has been saved under this key yet.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let path = self.doc_path(key)?;
        match fs::read_to_string(&path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes `value` atomically, keeping the previous content as a backup.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        let path = self.doc_path(key)?;

        // Keep the version we are about to replace.
        if path.exists() {
            self.push_backup(key, &path)?;
        }

        // Write to a sibling temp file, flush it to the physical disk, then rename over the
        // target. Rename within one directory is atomic on NTFS, so a reader either sees the
        // whole old file or the whole new one.
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(value.as_bytes())?;
            f.flush()?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        let path = self.doc_path(key)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Keys that currently have a document, sorted.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".json") {
                keys.push(stem.to_string());
            }
        }
        keys.sort();
        Ok(keys)
    }

    fn backup_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// Copies the current document aside under a sortable timestamped name, then trims the
    /// oldest ones so the folder cannot grow without bound.
    fn push_backup(&self, key: &str, current: &Path) -> Result<()> {
        let dir = self.backup_dir();
        fs::create_dir_all(&dir)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let dest = dir.join(format!("{}.{:013}.json", safe_key(key)?, stamp));
        // A copy, not a rename: the live file must stay readable throughout.
        fs::copy(current, &dest)?;
        self.trim_backups(key)?;
        Ok(())
    }

    fn trim_backups(&self, key: &str) -> Result<()> {
        let key = safe_key(key)?;
        let dir = self.backup_dir();
        let prefix = format!("{}.", key);
        let mut mine: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix) && n.ends_with(".json"))
                    .unwrap_or(false)
            })
            .collect();
        // Names embed a zero-padded timestamp, so lexical order is chronological order.
        mine.sort();
        while mine.len() > BACKUP_KEEP {
            let oldest = mine.remove(0);
            let _ = fs::remove_file(oldest); // a backup we fail to delete is not fatal
        }
        Ok(())
    }

    /// Backups for one key, newest first — what a "restore previous version" UI would list.
    pub fn backups(&self, key: &str) -> Result<Vec<String>> {
        let key = safe_key(key)?;
        let prefix = format!("{}.", key);
        let dir = self.backup_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut names: Vec<String> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .filter(|n| n.starts_with(&prefix) && n.ends_with(".json"))
            .collect();
        names.sort();
        names.reverse();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(tag: &str) -> (Store, PathBuf) {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "pm_store_test_{}_{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let s = Store::open(&dir).expect("open");
        (s, dir)
    }

    #[test]
    fn missing_key_is_none_not_an_error() {
        let (s, _d) = tmp_store("missing");
        assert!(s.get("pm_dashboard_v2").unwrap().is_none());
    }

    #[test]
    fn round_trips_utf8_content() {
        let (s, _d) = tmp_store("roundtrip");
        let payload = r#"{"projects":[{"name":"C12E02·比亚迪","platform":"X9"}]}"#;
        s.set("pm_dashboard_v2", payload).unwrap();
        assert_eq!(s.get("pm_dashboard_v2").unwrap().as_deref(), Some(payload));
    }

    #[test]
    fn list_reports_written_keys_only() {
        let (s, _d) = tmp_store("list");
        assert!(s.list().unwrap().is_empty());
        s.set("pm_dashboard_v2", "{}").unwrap();
        s.set("pm_dashboard_v2_calendar", "{}").unwrap();
        let keys = s.list().unwrap();
        assert_eq!(keys, vec!["pm_dashboard_v2", "pm_dashboard_v2_calendar"]);
    }

    #[test]
    fn delete_is_idempotent() {
        let (s, _d) = tmp_store("delete");
        s.set("k", "v").unwrap();
        s.delete("k").unwrap();
        s.delete("k").unwrap(); // deleting again must not fail
        assert!(s.get("k").unwrap().is_none());
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let (s, d) = tmp_store("notmp");
        s.set("k", "hello").unwrap();
        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "留下了临时文件: {:?}", leftovers);
    }

    #[test]
    fn overwrite_keeps_previous_version_as_backup() {
        let (s, _d) = tmp_store("backup");
        s.set("k", "第一版").unwrap();
        s.set("k", "第二版").unwrap();
        assert_eq!(s.get("k").unwrap().as_deref(), Some("第二版"));
        let b = s.backups("k").unwrap();
        assert_eq!(b.len(), 1, "应保留一份上一版");
        let restored = fs::read_to_string(s.root().join("backups").join(&b[0])).unwrap();
        assert_eq!(restored, "第一版");
    }

    #[test]
    fn backups_are_capped() {
        let (s, _d) = tmp_store("cap");
        for i in 0..(BACKUP_KEEP + 8) {
            s.set("k", &format!("版本{}", i)).unwrap();
            // distinct timestamps so ordering is well defined
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let b = s.backups("k").unwrap();
        assert!(
            b.len() <= BACKUP_KEEP,
            "备份数 {} 超过上限 {}",
            b.len(),
            BACKUP_KEEP
        );
    }

    #[test]
    fn backups_are_newest_first() {
        let (s, _d) = tmp_store("order");
        s.set("k", "A").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        s.set("k", "B").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
        s.set("k", "C").unwrap();
        let b = s.backups("k").unwrap();
        let newest = fs::read_to_string(s.root().join("backups").join(&b[0])).unwrap();
        assert_eq!(newest, "B", "最新备份应是被 C 覆盖掉的 B");
    }

    #[test]
    fn rejects_keys_that_escape_the_data_directory() {
        let (s, _d) = tmp_store("traversal");
        for bad in [
            "../secret",
            "..\\secret",
            "a/b",
            "a\\b",
            "",
            "key with space",
            "名字",
        ] {
            assert!(
                s.get(bad).is_err() || s.set(bad, "x").is_err(),
                "键名 {:?} 不应被接受",
                bad
            );
        }
        // and the legitimate keys the app uses must pass
        for good in [
            "pm_dashboard_v2",
            "pm_dashboard_v2_theme",
            "pm_dashboard_v2_calendar",
            "pm_dashboard_v2_templates",
        ] {
            assert!(s.set(good, "{}").is_ok(), "键名 {:?} 应被接受", good);
        }
    }

    #[test]
    fn large_payload_survives_round_trip() {
        let (s, _d) = tmp_store("large");
        // roughly the size of a 100-project plan
        let big = "x".repeat(2 * 1024 * 1024);
        s.set("pm_dashboard_v2", &big).unwrap();
        assert_eq!(s.get("pm_dashboard_v2").unwrap().unwrap().len(), big.len());
    }
}
