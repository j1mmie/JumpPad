//! Where the `syntaxes/<grammar>/` bundles are looked for, a startup
//! diagnostic for what was found there, and the lookup the syntax registry
//! resolves grammars through.

use std::path::PathBuf;

use jumppad_config::Languages;
use syntax_registry::GrammarLookup;

/// Where syntax-highlighting bundles are looked for. Each is a directory of
/// `<grammar>/config.toml` files, optionally with a `syntax.wasm` beside
/// each one.
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

/// How the registry resolves a grammar: by the extension of the file being
/// opened, or by the language a Markdown code fence names.
pub(crate) fn grammar_lookup(languages: &Languages) -> GrammarLookup {
    GrammarLookup {
        by_extension: languages.extension_to_grammar(),
        by_fence_language: languages.fence_language_to_grammar(),
        symbols: languages.grammar_symbols(),
    }
}

/// Startup diagnostic: which languages were resolved, and which of them
/// actually have a grammar on disk to highlight with.
///
/// Worth saying out loud because the two halves fail differently. A language
/// with no bundle behind it has no comment style either; a bundle with no
/// `syntax.wasm` still toggles comments and just never colors anything.
pub(crate) fn log_bundles_found(dirs: &[PathBuf], languages: &Languages) {
    for dir in dirs {
        match std::fs::read_dir(dir) {
            Ok(_) => log::debug!("{}: searching for bundles", dir.display()),
            Err(err) => {
                log::debug!("{}: couldn't read directory: {err}", dir.display())
            }
        }
    }

    let mut highlighted = Vec::new();
    let mut unhighlighted = Vec::new();
    for language in languages.entries() {
        let Some(grammar) = &language.syntax else {
            unhighlighted.push(language.name.clone());
            continue;
        };
        let found = dirs
            .iter()
            .any(|dir| dir.join(grammar).join("syntax.wasm").is_file());
        if found {
            highlighted.push(grammar.clone());
        } else {
            unhighlighted.push(language.name.clone());
        }
    }
    highlighted.sort();
    unhighlighted.sort();

    log::debug!(
        "{} language(s) with a grammar: {}",
        highlighted.len(),
        highlighted.join(", ")
    );
    log::debug!(
        "{} language(s) without one (comments only): {}",
        unhighlighted.len(),
        unhighlighted.join(", ")
    );
}
