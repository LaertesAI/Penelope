//! A lock between processes and an atomic file write, for the files of the memory.
//!
//! The lock is a file created with `create_new`, which is atomic on every platform. A lock older than 30 seconds
//! belongs to a dead process and is removed. `write_atomic` writes a temporary file next to the target and renames
//! it, so a reader sees the old file or the new file and never half of one, and a killed writer leaves the old file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How long a call waits for a lock before it gives up with an error.
const WAIT: Duration = Duration::from_secs(10);
/// A lock file older than this was left by a process that died.
const STALE_SECS: u64 = 30;
/// A folder that refuses the lock file for this many tries is read-only. A Windows file that another thread deletes
/// answers `PermissionDenied` for a moment, so a few tries are normal.
const DENIED_TRIES: u32 = 25;

pub struct Lock(PathBuf);

impl Lock {
    /// Takes the lock `dir/name`. Waits up to 10 seconds, with a short growing pause. An error names the file and the reason.
    pub fn take(dir: &Path, name: &str) -> Result<Lock, String> {
        let path = dir.join(name);
        let deadline = Instant::now() + WAIT;
        let (mut pause, mut denied) = (1u64, 0u32);
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Lock(path)),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied) => {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        denied += 1;
                        if denied >= DENIED_TRIES && !path.exists() {
                            return Err(format!("cannot lock {}: {} (the folder is read-only?)", path.display(), e));
                        }
                    }
                    // An age that cannot be read (the file is gone or locked) is not stale.
                    let stale = fs::metadata(&path).and_then(|m| m.modified()).ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok()).map(|age| age.as_secs() > STALE_SECS).unwrap_or(false);
                    if stale {
                        let _ = fs::remove_file(&path);
                    } else {
                        std::thread::sleep(Duration::from_millis(pause));
                        pause = (pause * 2).min(20);
                    }
                }
                Err(e) => return Err(format!("cannot lock {}: {}", path.display(), e)),
            }
            if Instant::now() >= deadline {
                return Err(format!("{} stayed locked for 10 seconds", path.display()));
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Writes `data` to `path` through a temporary file in the same folder and a rename. An existing file keeps its permissions.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{}.tmp-{}-{}", name, std::process::id(), n));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        if let Ok(meta) = fs::metadata(path) {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("penelope-fslock-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_lock_excludes_a_second_taker_until_it_drops() {
        let dir = temp("lock");
        let first = Lock::take(&dir, "x.lock").unwrap();
        assert!(dir.join("x.lock").exists());
        let dir2 = dir.clone();
        let waiter = std::thread::spawn(move || Lock::take(&dir2, "x.lock").map(|_| Instant::now()));
        std::thread::sleep(Duration::from_millis(60));
        let freed = Instant::now();
        drop(first);
        let got = waiter.join().unwrap().unwrap();
        assert!(got >= freed, "the second taker got the lock before the first dropped it");
        assert!(!dir.join("x.lock").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_replaces_the_file_and_leaves_no_temporary() {
        let dir = temp("atomic");
        let file = dir.join("map.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        let names: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["map.json".to_string()], "{:?}", names);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_fails_fast_with_the_reason() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("ro");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        if fs::write(dir.join("probe"), b"x").is_ok() {
            // running as root: the folder is writable anyway
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let t = Instant::now();
        let err = Lock::take(&dir, "m.lock").err().expect("no lock in a read-only folder");
        assert!(err.contains("cannot lock") && err.contains("read-only"), "{}", err);
        assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
        assert!(write_atomic(&dir.join("f"), b"x").is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
