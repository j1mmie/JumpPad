use std::path::{Path, PathBuf};

use tree_sitter::{wasmtime, Language, Parser, WasmStore};

/// Finds `<grammar_name>.wasm` in the first search directory that has it.
pub(crate) fn find_wasm(dirs: &[PathBuf], grammar_name: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(format!("{grammar_name}.wasm")))
        .find(|path| path.is_file())
}

/// Finds and reads `<grammar_name>.injections.scm` in the first search
/// directory that has it, if any. Most grammars won't have one.
pub(crate) fn find_injections_source(dirs: &[PathBuf], grammar_name: &str) -> Option<String> {
    let path = dirs
        .iter()
        .map(|dir| dir.join(format!("{grammar_name}.injections.scm")))
        .find(|path| path.is_file())?;
    match std::fs::read_to_string(&path) {
        Ok(source) => Some(source),
        Err(err) => {
            eprintln!("syntax_registry: reading {}: {err}", path.display());
            None
        }
    }
}

/// Reads and compiles a wasm grammar file into a ready-to-use language and
/// parser (not yet wrapped in a `Grammar` - the caller assembles that once
/// it's also resolved any injection targets).
///
/// `name` is passed to `WasmStore::load_language` and must match the
/// grammar's compiled export symbol (`tree_sitter_<name>`). This is why
/// `SyntaxRegistry` resolves file extensions to a grammar name via config
/// first (e.g. a `.md` file resolving to grammar name `"markdown"`) rather
/// than assuming the extension itself is always the right name - upstream
/// `tree-sitter-markdown` exports `tree_sitter_markdown`, not
/// `tree_sitter_md`. A mismatch here still surfaces as an ordinary load
/// error if a grammar's actual export doesn't match its configured name.
pub(crate) fn load(
    engine: &wasmtime::Engine,
    path: &Path,
    name: &str,
) -> Result<(Language, Parser), String> {
    let bytes = std::fs::read(path).map_err(|err| format!("reading {}: {err}", path.display()))?;

    let mut store = WasmStore::new(engine).map_err(|err| format!("creating wasm store: {err}"))?;
    let language = store
        .load_language(name, &bytes)
        .map_err(|err| format!("loading language {name:?} from {}: {err}", path.display()))?;

    let mut parser = Parser::new();
    parser
        .set_wasm_store(store)
        .map_err(|err| format!("attaching wasm store to parser: {err}"))?;
    parser
        .set_language(&language)
        .map_err(|err| format!("setting language {name:?} on parser: {err}"))?;

    Ok((language, parser))
}
