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
#[path = "disk_tests.rs"]
mod tests;
