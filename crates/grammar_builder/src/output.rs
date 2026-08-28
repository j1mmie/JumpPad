//! Assembling `syntaxes/output`: every bundle copied together, which is the
//! folder to ship next to a JumpPad binary.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

const OUTPUT: &str = "output";
const CONFIG: &str = "config.toml";

/// What the assembled folder ended up holding, for the closing report.
pub struct Assembled {
    pub path: PathBuf,
    pub with_grammar: usize,
    pub configs_only: usize,
}

/// Copies every bundle under `syntaxes/` into `syntaxes/output`, whether or
/// not a grammar was built for it.
///
/// The grammar-less bundles have to come too. Rust and Python ship a config
/// and no `.wasm`, and their comment styles are how toggle-comment knows
/// what a `.rs` comment looks like - a release without them quietly loses a
/// feature.
///
/// Rebuilt from scratch each run, so a language that has been renamed or
/// dropped doesn't linger in a folder someone is about to ship.
pub fn assemble(syntaxes: &Path) -> Result<Assembled, Box<dyn Error>> {
    // Listed before the destination is created, so the run before last's
    // `output/` is never a candidate for copying into this one.
    let bundles = bundles_in(syntaxes)?;

    let path = syntaxes.join(OUTPUT);
    if path.exists() {
        fs::remove_dir_all(&path)?;
    }
    fs::create_dir_all(&path)?;

    let mut assembled = Assembled {
        path,
        with_grammar: 0,
        configs_only: 0,
    };
    for bundle in bundles {
        let name = bundle
            .file_name()
            .ok_or("a bundle directory with no name")?
            .to_owned();
        let into = assembled.path.join(&name);
        fs::create_dir_all(&into)?;

        copy(&bundle, &into, CONFIG)?;
        copy(&bundle, &into, "injections.scm")?;
        if copy(&bundle, &into, "syntax.wasm")? {
            assembled.with_grammar += 1;
        } else {
            assembled.configs_only += 1;
        }
    }
    Ok(assembled)
}

/// The bundle directories under `syntaxes/`, sorted.
///
/// A directory is a bundle if and only if it holds a `config.toml`, which is
/// what keeps `output/`, the clone workdir and `node_modules` out without
/// naming any of them.
fn bundles_in(syntaxes: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut bundles: Vec<PathBuf> = fs::read_dir(syntaxes)
        .map_err(|error| format!("{}: {error}", syntaxes.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.join(CONFIG).is_file())
        .collect();
    bundles.sort();
    Ok(bundles)
}

/// Copies one file of a bundle across if it has one. Reports whether it did,
/// since a missing `syntax.wasm` is the ordinary case rather than an error.
fn copy(from: &Path, into: &Path, file: &str) -> Result<bool, Box<dyn Error>> {
    let source = from.join(file);
    if !source.is_file() {
        return Ok(false);
    }
    fs::copy(&source, into.join(file))
        .map_err(|error| format!("{}: {error}", source.display()))?;
    Ok(true)
}
