//! Draft autosave / crash-recovery persistence.
//!
//! One `session.toml` manifest lists every open tab's id/path/dirty state
//! plus which was active; each dirty tab also gets a `<id>.draft` file
//! holding its unsaved content.

use std::path::{Path, PathBuf};

use editor_core::Tab;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionManifest {
    pub active: usize,
    pub tabs: Vec<TabEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabEntry {
    pub id: u64,
    pub path: Option<PathBuf>,
    pub dirty: bool,
}

const MANIFEST_FILE: &str = "session.toml";

/// Where the session manifest and draft files live: next to the running
/// executable, then `./drafts` (a `cargo run` convenience).
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.join("drafts"));
    }
    dirs.push(PathBuf::from("drafts"));
    dirs
}

/// Reads and parses the first `session.toml` found among `candidates`.
/// Returns `None` if none exist or it's unparseable - never blocks startup.
pub fn load_manifest(candidates: &[PathBuf]) -> Option<SessionManifest> {
    for dir in candidates {
        let path = dir.join(MANIFEST_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        return match toml::from_str(&text) {
            Ok(manifest) => Some(manifest),
            Err(err) => {
                log::warn!(
                    "{}: {err}, starting without a restored session",
                    path.display()
                );
                None
            }
        };
    }
    None
}

pub fn draft_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.draft"))
}

/// Snapshots the current tab list into a `SessionManifest` - pure, no I/O.
pub fn build_manifest(tabs: &[Tab], active: usize) -> SessionManifest {
    SessionManifest {
        active,
        tabs: tabs
            .iter()
            .map(|tab| TabEntry {
                id: tab.id,
                path: tab.document.path.clone(),
                dirty: tab.dirty,
            })
            .collect(),
    }
}

/// Writes `manifest` to `dir/session.toml`, then deletes any `<id>.draft`
/// file that no longer belongs to a dirty tab in `manifest`.
pub fn write_manifest_sync(dir: &Path, manifest: &SessionManifest) {
    if let Err(err) = std::fs::create_dir_all(dir) {
        log::warn!("couldn't create {}: {err}", dir.display());
        return;
    }
    match toml::to_string_pretty(manifest) {
        Ok(text) => {
            if let Err(err) = std::fs::write(dir.join(MANIFEST_FILE), text) {
                log::warn!("couldn't write session manifest: {err}");
            }
        }
        Err(err) => {
            log::warn!("couldn't serialize session manifest: {err}")
        }
    }

    prune_orphaned_drafts(dir, manifest);
}

fn prune_orphaned_drafts(dir: &Path, manifest: &SessionManifest) {
    let live_ids: std::collections::HashSet<u64> = manifest
        .tabs
        .iter()
        .filter(|entry| entry.dirty)
        .map(|entry| entry.id)
        .collect();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("draft") {
            continue;
        }
        let is_live = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok())
            .is_some_and(|id| live_ids.contains(&id));
        if !is_live && let Err(err) = std::fs::remove_file(&path) {
            log::warn!("couldn't remove stale draft {}: {err}", path.display());
        }
    }
}

/// Every tab whose content has changed since its draft was last written -
/// `(id, generation, text)`.
pub fn stale_tabs(tabs: &[Tab]) -> Vec<(u64, u64, String)> {
    tabs.iter()
        .filter(|tab| {
            tab.dirty && tab.draft_generation != tab.flushed_generation
        })
        .map(|tab| (tab.id, tab.draft_generation, tab.editor.text()))
        .collect()
}

/// Writes one tab's draft content asynchronously. Best-effort: errors are
/// logged, not surfaced. Returns `(id, generation)` for the caller to mark flushed.
pub async fn flush_draft_async(
    dir: PathBuf,
    id: u64,
    generation: u64,
    text: String,
) -> (u64, u64) {
    if let Err(err) = tokio::fs::create_dir_all(&dir).await {
        log::warn!("couldn't create {}: {err}", dir.display());
        return (id, generation);
    }
    if let Err(err) = tokio::fs::write(draft_path(&dir, id), text).await {
        log::warn!("couldn't write draft for tab {id}: {err}");
    }
    (id, generation)
}

/// Synchronous last-ditch flush run when the app is quitting: writes the
/// manifest and every stale tab's draft before the window closes.
pub fn flush_on_exit(dir: &Path, tabs: &[Tab], active: usize) {
    let manifest = build_manifest(tabs, active);
    write_manifest_sync(dir, &manifest);
    for (id, _generation, text) in stale_tabs(tabs) {
        if let Err(err) = std::fs::write(draft_path(dir, id), text) {
            log::warn!("couldn't write draft for tab {id} on exit: {err}");
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
