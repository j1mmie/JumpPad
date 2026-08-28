use serde::{Deserialize, Serialize};

/// One language's settings: the file extensions it covers, the grammar that
/// highlights them, the names it answers to inside a fenced code block, and
/// how its comments are written.
///
/// The same shape is read from two places. A bundled
/// `syntaxes/<grammar>/config.toml` ships a language's defaults, and a
/// `[[languages]]` entry in the user's `config.toml` patches them by `name`.
/// Every field but `name` is optional so a patch can name the one setting it
/// wants to change and leave the rest of the bundle's alone - an absent field
/// means "keep the bundled value", not "clear it".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageConfig {
    /// What a user entry and a bundle are matched on, ignoring case. Also
    /// what the language is called anywhere JumpPad names it.
    pub name: String,
    /// The grammar directory under `syntaxes/` these extensions highlight
    /// with. A bundle that leaves this out is named by its own directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    /// The names this language answers to in a Markdown fence's info string
    /// (` ```js ` finding the `javascript` grammar). The grammar's own name
    /// always works and needs no entry here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    /// The symbol the `.wasm` exports its parser under, when it isn't
    /// `tree_sitter_<syntax>` - the escape hatch for a grammar built
    /// elsewhere under a name that doesn't match its directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<CommentSyntax>,
}

/// A language's comment syntax: exactly one of `comment.single` or
/// `comment.multi` - defining both fails the whole file's parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCommentSyntax", into = "RawCommentSyntax")]
pub enum CommentSyntax {
    Single(String),
    Multi { left: String, right: String },
}

/// The TOML-facing shape `CommentSyntax` validates from.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommentSyntax {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    single: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    multi: Option<RawMultiComment>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMultiComment {
    left: String,
    right: String,
}

impl TryFrom<RawCommentSyntax> for CommentSyntax {
    type Error = String;

    fn try_from(raw: RawCommentSyntax) -> Result<Self, String> {
        match (raw.single, raw.multi) {
            (Some(prefix), None) => Ok(Self::Single(prefix)),
            (None, Some(multi)) => Ok(Self::Multi { left: multi.left, right: multi.right }),
            (Some(_), Some(_)) => Err(
                "comment.single and comment.multi are mutually exclusive - keep exactly one"
                    .to_string(),
            ),
            (None, None) => Err(
                "comment must set comment.single or comment.multi (or be removed)".to_string(),
            ),
        }
    }
}

impl From<CommentSyntax> for RawCommentSyntax {
    fn from(comment: CommentSyntax) -> Self {
        match comment {
            CommentSyntax::Single(prefix) => Self {
                single: Some(prefix),
                multi: None,
            },
            CommentSyntax::Multi { left, right } => Self {
                single: None,
                multi: Some(RawMultiComment { left, right }),
            },
        }
    }
}
