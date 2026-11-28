//! A lock between processes and an atomic file write, for the files of the memory.
//!
//! The lock is a file created with `create_new`, which is atomic on every platform. A lock older than 30 seconds
//! belongs to a dead process and is removed. `write_atomic` writes a temporary file next to the target and renames
//! it, so a reader sees the old file or the new file and never half of one, and a killed writer leaves the old file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// A lock file older than this was left by a process that died.
const STALE_SECS: u64 = 30;
/// How long a call waits for a lock before it gives up with an error. Longer than `STALE_SECS`, so a lock left by a
/// killed process is taken over instead of failing the call.
const WAIT: Duration = Duration::from_secs(STALE_SECS + 5);
/// A folder that refuses the lock file for this many tries is read-only. A Windows file that another thread deletes
/// answers `PermissionDenied` for a moment, so a few tries are normal. The pause between them is fixed and short, so
/// the error comes in about 10 ms on every system.
const DENIED_TRIES: u32 = 5;

/// A held lock. The file holds a token `PID-COUNTER-NANOS`; only the holder of that token removes it.
pub struct Lock {
    path: PathBuf,
    token: String,
}

fn new_token() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), n, nanos)
}

/// Whether the lock file at `path` was left by a dead process: older than `STALE_SECS`, or (Linux) written by a
/// process id that no longer exists. A file whose age or owner cannot be read is not stale.
fn is_stale(path: &Path) -> bool {
    let old = fs::metadata(path).and_then(|m| m.modified()).ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok()).map(|age| age.as_secs() > STALE_SECS).unwrap_or(false);
    if old {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        let pid = fs::read_to_string(path).ok().and_then(|t| t.split('-').next().and_then(|p| p.trim().parse::<u32>().ok()));
        if let Some(pid) = pid {
            return !Path::new(&format!("/proc/{}", pid)).exists();
        }
    }
    false
}

impl Lock {
    /// Takes the lock `dir/name`. Waits up to 35 seconds, with a short growing pause. An error names the file and the reason.
    pub fn take(dir: &Path, name: &str) -> Result<Lock, String> {
        let path = dir.join(name);
        let token = new_token();
        let deadline = Instant::now() + WAIT;
        let (mut pause, mut denied) = (1u64, 0u32);
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = f.write_all(token.as_bytes());
                    return Ok(Lock { path, token });
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied) => {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        denied += 1;
                        if denied >= DENIED_TRIES && !path.exists() {
                            return Err(format!("cannot lock {}: {} (the folder is read-only?)", path.display(), e));
                        }
                        if !path.exists() {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                    }
                    if is_stale(&path) {
                        // Rename first: of two waiters that saw the same stale file only one rename succeeds, so the
                        // second cannot delete the lock the first has just created. The moved file is checked again;
                        // a live lock that slipped in between is put back.
                        let moved = dir.join(format!("{}.stale-{}", name, token));
                        if fs::rename(&path, &moved).is_ok() {
                            if is_stale(&moved) {
                                let _ = fs::remove_file(&moved);
                            } else {
                                let _ = fs::hard_link(&moved, &path);
                                let _ = fs::remove_file(&moved);
                            }
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(pause));
                        pause = (pause * 2).min(20);
                    }
                }
                Err(e) => return Err(format!("cannot lock {}: {}", path.display(), e)),
            }
            if Instant::now() >= deadline {
                return Err(format!("{} stayed locked for {} seconds", path.display(), WAIT.as_secs()));
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // A lock that was taken over as stale belongs to someone else now: leave it.
        if fs::read_to_string(&self.path).map(|t| t == self.token).unwrap_or(false) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Removes the files `dir/PREFIX*` except the `keep` newest by name. Names end in a timestamp, so name order is age.
pub fn prune(dir: &Path, prefix: &str, keep: usize) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut names: Vec<String> = read.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with(prefix)).collect();
    names.sort();
    let drop = names.len().saturating_sub(keep);
    for n in names.into_iter().take(drop) {
        let _ = fs::remove_file(dir.join(n));
    }
}

/// Removes temporary files `.NAME.tmp-*` and stale lock remains `*.lock.stale-*` older than an hour in `dir`: what a
/// killed writer left between its temporary write and its rename.
pub fn sweep(dir: &Path) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !((name.starts_with('.') && name.contains(".tmp-")) || name.contains(".lock.stale-")) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).map(|a| a.as_secs() > 3600).unwrap_or(false);
        if old {
            let _ = fs::remove_file(e.path());
        }
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
        f.sync_all()?; // the bytes are on disk before the rename makes them the file
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

    #[test]
    fn a_lock_of_a_dead_owner_is_taken_over_once() {
        let dir = temp("stale");
        fs::write(dir.join("x.lock"), b"999999999-0-0").unwrap(); // no such process
        let old = SystemTime::now() - Duration::from_secs(STALE_SECS + 5);
        fs::File::options().write(true).open(dir.join("x.lock")).unwrap().set_modified(old).unwrap();
        let t = Instant::now();
        let lock = Lock::take(&dir, "x.lock").unwrap();
        assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
        assert_eq!(fs::read_to_string(dir.join("x.lock")).unwrap(), lock.token);
        let names: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["x.lock".to_string()], "{:?}", names);
        drop(lock);
        assert!(!dir.join("x.lock").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_that_was_taken_over_is_not_removed_by_its_old_owner() {
        let dir = temp("takeover");
        let first = Lock::take(&dir, "x.lock").unwrap();
        fs::write(dir.join("x.lock"), b"someone-else").unwrap(); // the next owner after a takeover
        drop(first);
        assert_eq!(fs::read_to_string(dir.join("x.lock")).unwrap(), "someone-else");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_the_newest_and_sweep_removes_old_temporaries() {
        let dir = temp("prune");
        for t in 100..110 {
            fs::write(dir.join(format!("map.json.corrupt-{}", t)), b"x").unwrap();
        }
        prune(&dir, "map.json.corrupt-", 3);
        let mut left: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left, vec!["map.json.corrupt-107", "map.json.corrupt-108", "map.json.corrupt-109"]);
        let tmp = dir.join(".map.json.tmp-1-1");
        fs::write(&tmp, b"x").unwrap();
        sweep(&dir);
        assert!(tmp.exists(), "a fresh temporary may belong to a running writer");
        fs::File::options().write(true).open(&tmp).unwrap().set_modified(SystemTime::now() - Duration::from_secs(7200)).unwrap();
        sweep(&dir);
        assert!(!tmp.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
