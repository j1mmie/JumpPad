//! Reading the `syntaxes/<grammar>/` bundles that ship a grammar's `.wasm`
//! next to the defaults for the language it highlights.

use std::path::{Path, PathBuf};

use crate::LanguageConfig;

/// The file a directory needs before JumpPad reads it as a bundle at all.
const BUNDLE_CONFIG: &str = "config.toml";

/// One language's shipped defaults, read from a single
/// `syntaxes/<grammar>/config.toml`.
///
/// The directory's own name is the grammar name, and it is load-bearing in
/// two directions: sibling bundles' injection queries name this grammar by
/// it, and the `syntax.wasm` beside this file is loaded under it. Renaming
/// the directory renames the grammar.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxBundle {
    pub grammar: String,
    pub language: LanguageConfig,
}

/// Every bundle found under the search directories, sorted by grammar name.
///
/// Directories are searched in order and the first one to define a grammar
/// wins it outright, so a bundle next to the binary shadows one in the
/// working directory rather than merging with it - the same precedence the
/// `.wasm` beside it is found by.
///
/// A bundle that won't parse is logged and skipped: a broken one costs its
/// own language, not the rest of them.
pub fn discover(search_dirs: &[PathBuf]) -> Vec<SyntaxBundle> {
    let mut found: Vec<SyntaxBundle> = Vec::new();
    for dir in search_dirs {
        for grammar in grammar_names_in(dir) {
            if found.iter().any(|bundle| bundle.grammar == grammar) {
                continue;
            }
            if let Some(language) = read(dir, &grammar) {
                found.push(SyntaxBundle { grammar, language });
            }
        }
    }
    found.sort_by(|left, right| left.grammar.cmp(&right.grammar));
    found
}

/// The subdirectory names of `dir` that hold a bundle config, sorted so a
/// directory listing's arbitrary order can't reorder the languages.
fn grammar_names_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join(BUNDLE_CONFIG).is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Parses one bundle's config, filling in the `syntax` its directory name
/// already implies.
fn read(dir: &Path, grammar: &str) -> Option<LanguageConfig> {
    let path = dir.join(grammar).join(BUNDLE_CONFIG);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            log::warn!("reading {}: {err}", path.display());
            return None;
        }
    };
    match toml::from_str::<LanguageConfig>(&text) {
        Ok(mut language) => {
            language.syntax.get_or_insert_with(|| grammar.to_string());
            Some(language)
        }
        Err(err) => {
            log::warn!("{}: {err}, skipping this language", path.display());
            None
        }
    }
}

#[cfg(test)]
#[path = "bundle_tests.rs"]
mod tests;
