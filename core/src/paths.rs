//! Where the portable build keeps its data.
//!
//! This is a green/portable app: the data folder lives next to the executable, so the whole
//! thing can be copied to a USB stick or a shared drive and carry its plans along.
//!
//! That only works where the app can actually write. Unpacked into `Program Files`, onto a
//! read-only share, or into a folder a group policy has locked down, writing beside the exe
//! fails — and silently losing the user's edits is the worst possible outcome. So the
//! location is *probed*, not assumed, and falls back to the per-user application data folder,
//! which is always writable. The UI is told which one is in use so the person can see where
//! their data actually is.

use std::fs;
use std::path::{Path, PathBuf};

/// Folder name used both beside the exe and under %APPDATA%.
const DATA_DIR_NAME: &str = "data";
const APP_DIR_NAME: &str = "MilestoneCommand";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataLocation {
    /// `<exe folder>/data` — the portable case.
    BesideExe,
    /// `%APPDATA%/MilestoneCommand/data` — used when the exe folder is not writable.
    AppData,
}

#[derive(Debug, Clone)]
pub struct DataDir {
    pub path: PathBuf,
    pub location: DataLocation,
}

/// Can we create and write a file inside `dir`? Creates `dir` if needed.
///
/// Existence and permission bits are not enough to answer this on Windows: a folder can look
/// writable and still reject writes because of virtualisation or policy. The only reliable
/// test is to actually write something and remove it.
pub fn is_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-probe");
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Decide where data lives, given the folder holding the executable and the APPDATA root.
///
/// The writability test is a parameter rather than a hard-coded call so the fallback can be
/// exercised deterministically. Testing it through real file permissions is unreliable: it
/// depends on the OS and on who is running the suite — as root, for instance, the permission
/// bits are bypassed entirely and a "read-only" folder stays writable, so the test would pass
/// without ever reaching the branch it claims to cover.
pub fn resolve_with_probe(
    exe_dir: &Path,
    appdata: Option<&Path>,
    writable: impl Fn(&Path) -> bool,
) -> DataDir {
    let beside = exe_dir.join(DATA_DIR_NAME);
    if writable(&beside) {
        return DataDir {
            path: beside,
            location: DataLocation::BesideExe,
        };
    }
    if let Some(ad) = appdata {
        let fallback = ad.join(APP_DIR_NAME).join(DATA_DIR_NAME);
        if writable(&fallback) {
            return DataDir {
                path: fallback,
                location: DataLocation::AppData,
            };
        }
    }
    // Nothing is writable. Hand back the portable path anyway: the store will surface a clear
    // error on the first save, which is better than pretending a different folder worked.
    DataDir {
        path: beside,
        location: DataLocation::BesideExe,
    }
}

/// As above, probing the real filesystem.
pub fn resolve_with(exe_dir: &Path, appdata: Option<&Path>) -> DataDir {
    resolve_with_probe(exe_dir, appdata, |p| is_writable(p))
}

/// Production entry point: resolve against the real executable location and %APPDATA%.
pub fn resolve() -> DataDir {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    resolve_with(&exe_dir, appdata.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!(
            "pm_paths_{}_{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn prefers_the_folder_beside_the_exe() {
        let exe_dir = tmp("beside");
        let appdata = tmp("beside_ad");
        let d = resolve_with(&exe_dir, Some(&appdata));
        assert_eq!(d.location, DataLocation::BesideExe);
        assert_eq!(d.path, exe_dir.join("data"));
    }

    #[test]
    fn falls_back_to_appdata_when_exe_folder_is_not_writable() {
        let exe_dir = tmp("ro");
        let appdata = tmp("ro_ad");
        let blocked = exe_dir.join("data");
        // Deterministic: the probe reports the portable folder as unwritable, whatever the
        // filesystem and whoever is running the tests.
        let d = resolve_with_probe(&exe_dir, Some(&appdata), |p| p != blocked);
        assert_eq!(
            d.location,
            DataLocation::AppData,
            "exe 目录不可写时应回落到 APPDATA"
        );
        assert!(d.path.starts_with(&appdata));
        assert!(d.path.ends_with("data"));
    }

    #[test]
    fn reports_portable_path_when_nothing_is_writable() {
        let exe_dir = tmp("none");
        let appdata = tmp("none_ad");
        // Both unwritable: we still return the portable path so the first save fails loudly
        // there, rather than silently writing somewhere the user is not looking.
        let d = resolve_with_probe(&exe_dir, Some(&appdata), |_| false);
        assert_eq!(d.location, DataLocation::BesideExe);
        assert_eq!(d.path, exe_dir.join("data"));
    }

    #[test]
    fn real_permissions_also_trigger_the_fallback() {
        let exe_dir = tmp("roreal");
        let appdata = tmp("roreal_ad");

        let mut perms = fs::metadata(&exe_dir).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o500); // r-x: cannot create the data subfolder
        }
        #[cfg(not(unix))]
        perms.set_readonly(true);
        fs::set_permissions(&exe_dir, perms).unwrap();

        // Root ignores the permission bits, so this check cannot run meaningfully there.
        let really_blocked = !is_writable(&exe_dir.join("data"));

        let d = resolve_with(&exe_dir, Some(&appdata));

        let mut back = fs::metadata(&exe_dir).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            back.set_mode(0o700);
        }
        #[cfg(not(unix))]
        back.set_readonly(false);
        let _ = fs::set_permissions(&exe_dir, back);

        if really_blocked {
            assert_eq!(d.location, DataLocation::AppData);
        } else {
            eprintln!("跳过：当前用户无视目录权限（多半是 root），改由 probe 版用例覆盖该分支");
        }
    }

    #[test]
    fn writable_probe_leaves_nothing_behind() {
        let dir = tmp("probe");
        assert!(is_writable(&dir));
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(leftovers.is_empty(), "探测文件没有清理: {:?}", leftovers);
    }

    #[test]
    fn survives_having_no_appdata() {
        let exe_dir = tmp("noad");
        let d = resolve_with(&exe_dir, None);
        assert_eq!(d.location, DataLocation::BesideExe);
    }
}
