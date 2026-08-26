use serde::{Deserialize, Serialize};

/// One `[[languages]]` entry: file extensions plus an optional grammar and
/// an optional toggle-comment style. `name` is for the file's readability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageConfig {
    pub name: String,
    /// The `<syntax>.wasm` grammar these extensions highlight with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<String>,
    pub extensions: Vec<String>,
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
