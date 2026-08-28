//! Builds JumpPad's syntax-highlighting grammars.
//!
//! Run it with `cargo build_grammars`. A language is a directory:
//! `syntaxes/<grammar>/config.toml` names its extensions, comment style and
//! code-fence aliases and is committed; this fills in the `syntax.wasm` that
//! highlights them and the `injections.scm` naming the grammars it embeds.
//!
//! It writes to two places, and both are gitignored:
//!
//!   - `syntaxes/<grammar>/syntax.wasm`, built in place, so a checkout
//!     highlights under `cargo run` with nothing to copy.
//!   - `syntaxes/output/`, every bundle assembled together - the folder to
//!     ship. Rename it `syntaxes` next to a JumpPad binary and it is found.
//!
//! Needs `git` and Node on `PATH`. Everything else the tree-sitter CLI
//! fetches for itself.

mod output;
mod tools;

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const SYNTAXES: &str = "syntaxes";
/// Where the grammar sources are cloned. Removed on the way out, however
/// the build ended.
const WORKDIR: &str = "tmp";

/// One grammar to build: where its source comes from, and which bundle it
/// lands in.
struct Grammar {
    /// The GitHub repository, as `owner/name`.
    repository: &'static str,
    /// The subdirectory of the clone holding this grammar, for the
    /// repositories carrying several - `tree-sitter-xml` has both `xml` and
    /// `dtd`. Empty when the grammar is the repository root.
    subdirectory: &'static str,
    /// The `syntaxes/<bundle>` directory it builds into. Also the name every
    /// injection query resolves this grammar by, so it is not free to change.
    bundle: &'static str,
    /// Whether upstream ships an `injections.scm` naming the grammars this
    /// one embeds.
    injections: bool,
}

/// A repository whose root is the grammar.
const fn whole(repository: &'static str, bundle: &'static str) -> Grammar {
    Grammar {
        repository,
        subdirectory: "",
        bundle,
        injections: false,
    }
}

/// One of several grammars sharing a repository.
const fn part(
    repository: &'static str,
    subdirectory: &'static str,
    bundle: &'static str,
) -> Grammar {
    Grammar {
        repository,
        subdirectory,
        bundle,
        injections: false,
    }
}

impl Grammar {
    const fn with_injections(self) -> Self {
        Self {
            injections: true,
            ..self
        }
    }
}

/// Markdown is the one that needs company: its block grammar leaves every
/// link and bold run to `markdown_inline`, and the two injection queries
/// reach further still - into yaml, toml and html here, and into whatever
/// language a fenced code block names.
const GRAMMARS: &[Grammar] = &[
    whole("ikatyang/tree-sitter-toml", "toml"),
    whole("tree-sitter/tree-sitter-json", "json"),
    whole("tree-sitter/tree-sitter-html", "html"),
    whole("tree-sitter-grammars/tree-sitter-yaml", "yaml"),
    whole("tree-sitter-grammars/tree-sitter-diff", "diff"),
    whole("tree-sitter-grammars/tree-sitter-make", "make"),
    whole("tree-sitter-grammars/tree-sitter-pem", "pem"),
    part("tree-sitter-grammars/tree-sitter-xml", "xml", "xml"),
    part("tree-sitter-grammars/tree-sitter-xml", "dtd", "dtd"),
    part("tree-sitter-grammars/tree-sitter-csv", "csv", "csv"),
    part("tree-sitter-grammars/tree-sitter-csv", "psv", "psv"),
    part("tree-sitter-grammars/tree-sitter-csv", "tsv", "tsv"),
    part(
        "tree-sitter-grammars/tree-sitter-markdown",
        "tree-sitter-markdown",
        "markdown",
    )
    .with_injections(),
    part(
        "tree-sitter-grammars/tree-sitter-markdown",
        "tree-sitter-markdown-inline",
        "markdown_inline",
    )
    .with_injections(),
];

fn main() -> ExitCode {
    match build() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("build_grammars: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build() -> Result<(), Box<dyn Error>> {
    tools::check_available()?;
    let syntaxes = workspace_root().join(SYNTAXES);
    tools::install_tree_sitter(&syntaxes)?;

    let workdir = Workdir::fresh(syntaxes.join(WORKDIR))?;
    let mut cloned = BTreeSet::new();
    for grammar in GRAMMARS {
        let checkout = workdir.path.join(checkout_name(grammar.repository));
        if cloned.insert(grammar.repository) {
            tools::clone(grammar.repository, &checkout)?;
        }
        build_one(&syntaxes, grammar, &checkout)?;
    }

    let assembled = output::assemble(&syntaxes)?;
    println!("\nassembled {}", assembled.path.display());
    println!("  {} language(s) with a grammar", assembled.with_grammar);
    println!(
        "  {} without one (comment styles only)",
        assembled.configs_only
    );
    Ok(())
}

fn build_one(
    syntaxes: &Path,
    grammar: &Grammar,
    checkout: &Path,
) -> Result<(), Box<dyn Error>> {
    let bundle = bundle_dir(syntaxes, grammar.bundle)?;
    let source = match grammar.subdirectory {
        "" => checkout.to_path_buf(),
        subdirectory => checkout.join(subdirectory),
    };

    println!("building {}/syntax.wasm", grammar.bundle);
    tools::build_wasm(syntaxes, &source, &bundle.join("syntax.wasm"))?;

    if grammar.injections {
        let query = source.join("queries").join("injections.scm");
        fs::copy(&query, bundle.join("injections.scm"))
            .map_err(|error| format!("{}: {error}", query.display()))?;
    }
    Ok(())
}

/// The bundle a grammar builds into, refusing one that isn't there.
///
/// The config is what names a grammar's extensions and comment style, so a
/// `.wasm` without one beside it is a language JumpPad would never look up -
/// better to say so than to build something nothing can reach.
fn bundle_dir(syntaxes: &Path, bundle: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = syntaxes.join(bundle);
    if !path.join("config.toml").is_file() {
        return Err(format!(
            "no {SYNTAXES}/{bundle}/config.toml - add one before building it"
        )
        .into());
    }
    Ok(path)
}

/// The clone directory's name, taken from the repository's.
fn checkout_name(repository: &str) -> &str {
    repository.rsplit('/').next().unwrap_or(repository)
}

/// An empty directory for the clones that removes itself afterwards,
/// however the build ended.
struct Workdir {
    path: PathBuf,
}

impl Workdir {
    fn fresh(path: PathBuf) -> Result<Self, Box<dyn Error>> {
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for Workdir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two directories under the workspace root")
        .to_path_buf()
}
