use std::path::Path;
use std::time::SystemTime;

/// On-disk identity cheap enough to stat on demand. mtime granularity is
/// filesystem-dependent, so a same-length rewrite inside one tick can slip
/// past a comparison - accepted rather than hashing contents on a path that
/// runs per open tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStamp {
    pub mtime: SystemTime,
    pub len: u64,
}

impl DiskStamp {
    /// `None` when the path doesn't exist or can't be stat'd.
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            mtime: metadata.modified().ok()?,
            len: metadata.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A fresh scratch directory per test, under the OS temp dir.
    fn scratch_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("jumppad-disk-test-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_stamp_changes_when_the_file_is_rewritten() {
        let dir = scratch_dir();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "one").expect("write");
        let before = DiskStamp::of(&file).expect("stamp");

        // Length alone proves the change without depending on the
        // filesystem's mtime granularity.
        std::fs::write(&file, "one two three").expect("rewrite");
        let after = DiskStamp::of(&file).expect("stamp");

        assert_ne!(before, after);
        assert_eq!(after.len, "one two three".len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_path_has_no_stamp() {
        let dir = scratch_dir();
        assert_eq!(DiskStamp::of(&dir.join("never-written.txt")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
