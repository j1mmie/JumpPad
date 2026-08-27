//! The languages JumpPad actually runs with: the bundles found under
//! `syntaxes/`, each patched by the user's own `[[languages]]` entry.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::bundle::{self, SyntaxBundle};
use crate::{CommentSyntax, LanguageConfig};

/// One language with every setting settled - a bundle's shipped defaults
/// under the user's patch, or a user entry that matched no bundle at all.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLanguage {
    pub name: String,
    pub syntax: Option<String>,
    pub extensions: Vec<String>,
    pub aliases: Vec<String>,
    pub symbol: Option<String>,
    pub comment: Option<CommentSyntax>,
}

impl ResolvedLanguage {
    /// Overwrites only the settings the patch actually names. An absent
    /// field leaves the bundled value alone, which is what lets a user
    /// change one language's extensions without restating its comment style.
    fn patched_with(&mut self, patch: &LanguageConfig) {
        if let Some(syntax) = &patch.syntax {
            self.syntax = Some(syntax.clone());
        }
        if let Some(extensions) = &patch.extensions {
            self.extensions = extensions.clone();
        }
        if let Some(aliases) = &patch.aliases {
            self.aliases = aliases.clone();
        }
        if let Some(symbol) = &patch.symbol {
            self.symbol = Some(symbol.clone());
        }
        if let Some(comment) = &patch.comment {
            self.comment = Some(comment.clone());
        }
    }

    /// Every name a Markdown fence can call this language by: its grammar's
    /// own name, the aliases it declares, and its file extensions - ```rs
    /// and ```yml are as common in the wild as the grammar's real name.
    fn fence_names(&self) -> impl Iterator<Item = String> + '_ {
        self.syntax
            .iter()
            .chain(self.aliases.iter())
            .chain(self.extensions.iter())
            .map(|name| name.to_lowercase())
    }
}

/// Every language JumpPad knows about, ordered so that later entries win a
/// contested extension: the bundles first, by grammar name, then whatever
/// the user's config added on top of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Languages {
    entries: Vec<ResolvedLanguage>,
}

impl Languages {
    /// The bundles under `search_dirs`, each patched by the `[[languages]]`
    /// entry naming it, plus any user entry that named no bundle.
    pub fn resolve(user: &[LanguageConfig], search_dirs: &[PathBuf]) -> Self {
        Self::merge(&bundle::discover(search_dirs), user)
    }

    /// The disk-free half of [`resolve`] - bundles already in hand.
    pub(crate) fn merge(
        bundles: &[SyntaxBundle],
        user: &[LanguageConfig],
    ) -> Self {
        let mut entries: Vec<ResolvedLanguage> =
            bundles.iter().map(shipped).collect();

        for patch in user {
            // Matched on `name` ignoring case, so `name = "yaml"` in a
            // config patches the bundle that calls itself "YAML" rather
            // than quietly adding a second language beside it.
            let matched = entries
                .iter_mut()
                .find(|entry| entry.name.eq_ignore_ascii_case(&patch.name));
            match matched {
                Some(entry) => entry.patched_with(patch),
                None => entries.push(added(patch)),
            }
        }

        Self { entries }
    }

    pub fn entries(&self) -> &[ResolvedLanguage] {
        &self.entries
    }

    /// Extension (lowercased) -> grammar name, for the syntax registry. A
    /// language without a `syntax` contributes nothing; a later entry wins
    /// an extension.
    pub fn extension_to_grammar(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for language in &self.entries {
            let Some(syntax) = &language.syntax else {
                continue;
            };
            for extension in &language.extensions {
                map.insert(extension.to_lowercase(), syntax.clone());
            }
        }
        map
    }

    /// Extension (lowercased) -> comment style, for toggle-comment. Built
    /// separately from the grammar map because a language can have one
    /// without the other: a comment style needs no `.wasm` behind it.
    pub fn comment_styles_by_extension(&self) -> HashMap<String, CommentSyntax> {
        let mut map = HashMap::new();
        for language in &self.entries {
            let Some(comment) = &language.comment else {
                continue;
            };
            for extension in &language.extensions {
                map.insert(extension.to_lowercase(), comment.clone());
            }
        }
        map
    }

    /// Fence info string (lowercased) -> grammar name, for the language a
    /// Markdown code fence names. Doubles as the set of names worth trying
    /// at all: a fence naming anything absent from here resolves to nothing
    /// rather than starting a load that could only fail.
    pub fn fence_language_to_grammar(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for language in &self.entries {
            let Some(syntax) = &language.syntax else {
                continue;
            };
            for name in language.fence_names() {
                map.insert(name, syntax.clone());
            }
        }
        map
    }

    /// Grammar name -> the symbol its `.wasm` exports, for the few that
    /// don't export `tree_sitter_<grammar>`. Absent means the default.
    pub fn grammar_symbols(&self) -> HashMap<String, String> {
        self.entries
            .iter()
            .filter_map(|language| {
                let syntax = language.syntax.as_ref()?;
                let symbol = language.symbol.as_ref()?;
                Some((syntax.clone(), symbol.clone()))
            })
            .collect()
    }
}

/// A bundle with nothing patching it: its own `syntax` already filled in
/// from its directory name by `bundle::read`.
fn shipped(bundle: &SyntaxBundle) -> ResolvedLanguage {
    let language = &bundle.language;
    ResolvedLanguage {
        name: language.name.clone(),
        syntax: language.syntax.clone(),
        extensions: language.extensions.clone().unwrap_or_default(),
        aliases: language.aliases.clone().unwrap_or_default(),
        symbol: language.symbol.clone(),
        comment: language.comment.clone(),
    }
}

/// A user entry naming no bundle - a language JumpPad only knows from the
/// config file. Its `syntax` is whatever it named, if anything.
fn added(patch: &LanguageConfig) -> ResolvedLanguage {
    ResolvedLanguage {
        name: patch.name.clone(),
        syntax: patch.syntax.clone(),
        extensions: patch.extensions.clone().unwrap_or_default(),
        aliases: patch.aliases.clone().unwrap_or_default(),
        symbol: patch.symbol.clone(),
        comment: patch.comment.clone(),
    }
}

#[cfg(test)]
#[path = "languages_tests.rs"]
mod tests;
