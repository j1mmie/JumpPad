//! Where syntax-highlighting wasm grammars (`<extension>.wasm`) are looked
//! for, and a startup diagnostic for what was found there.

use std::path::PathBuf;

/// Where syntax-highlighting wasm grammars (`<extension>.wasm`) are looked for.
pub(crate) fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.join("syntaxes"));
    }
    dirs.push(PathBuf::from("syntaxes")); // convenience for `cargo run`
    dirs
}

/// Startup diagnostic: lists the `.wasm` grammar files found in each search directory.
pub(crate) fn log_wasm_files_found(dirs: &[PathBuf]) {
    for dir in dirs {
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                let mut wasm_files: Vec<String> = entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension().and_then(|ext| ext.to_str())
                            == Some("wasm")
                    })
                    .filter_map(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .collect();
                wasm_files.sort();
                if wasm_files.is_empty() {
                    log::debug!(
                        "{}: exists but no .wasm files found",
                        dir.display()
                    );
                } else {
                    log::debug!(
                        "{}: found {} .wasm file(s): {}",
                        dir.display(),
                        wasm_files.len(),
                        wasm_files.join(", ")
                    );
                }
            }
            Err(err) => {
                log::debug!(
                    "{}: couldn't read directory: {err}",
                    dir.display()
                );
            }
        }
    }
}
