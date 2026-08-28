use std::path::{Path, PathBuf};

use tree_sitter::{Language, Parser, WasmStore, wasmtime};

/// The grammar inside a bundle directory. Generic because the directory
/// already names the grammar: nothing reads this file name, and the symbol
/// the parser is loaded under comes from the directory instead.
const GRAMMAR_WASM: &str = "syntax.wasm";

/// The query naming the languages a grammar embeds in its own content.
/// Most bundles won't have one.
const INJECTIONS_QUERY: &str = "injections.scm";

/// Finds `<grammar_name>/syntax.wasm` in the first search directory that
/// has it.
pub(crate) fn find_wasm(
    dirs: &[PathBuf],
    grammar_name: &str,
) -> Option<PathBuf> {
    find_in_bundle(dirs, grammar_name, GRAMMAR_WASM)
}

/// Finds and reads `<grammar_name>/injections.scm` in the first search
/// directory that has it, if any.
pub(crate) fn find_injections_source(
    dirs: &[PathBuf],
    grammar_name: &str,
) -> Option<String> {
    let path = find_in_bundle(dirs, grammar_name, INJECTIONS_QUERY)?;
    match std::fs::read_to_string(&path) {
        Ok(source) => Some(source),
        Err(err) => {
            log::warn!("reading {}: {err}", path.display());
            None
        }
    }
}

fn find_in_bundle(
    dirs: &[PathBuf],
    grammar_name: &str,
    file: &str,
) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(grammar_name).join(file))
        .find(|path| path.is_file())
}

/// Reads and compiles a wasm grammar file into a ready-to-use language and
/// parser. `symbol` must match the grammar's compiled export
/// (`tree_sitter_<symbol>`), which is the bundle's directory name unless its
/// config named something else. The file's own name never reaches wasmtime.
pub(crate) fn load(
    engine: &wasmtime::Engine,
    path: &Path,
    symbol: &str,
) -> Result<(Language, Parser), String> {
    let bytes = std::fs::read(path)
        .map_err(|err| format!("reading {}: {err}", path.display()))?;

    let mut store = WasmStore::new(engine)
        .map_err(|err| format!("creating wasm store: {err}"))?;
    let language = store.load_language(symbol, &bytes).map_err(|err| {
        format!("loading language {symbol:?} from {}: {err}", path.display())
    })?;

    let mut parser = Parser::new();
    parser
        .set_wasm_store(store)
        .map_err(|err| format!("attaching wasm store to parser: {err}"))?;
    parser.set_language(&language).map_err(|err| {
        format!("setting language {symbol:?} on parser: {err}")
    })?;

    Ok((language, parser))
}
