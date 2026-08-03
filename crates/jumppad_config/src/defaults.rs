use std::collections::HashMap;

use crate::{AlphaConfig, CommentStyle, Config, SyntaxesConfig, VisorConfig, WindowConfig};

/// Built-in defaults, written out as the config file on first run.
pub(crate) fn config() -> Config {
    let entries: &[(&str, &[&str])] = &[
        ("json", &["json"]),
        ("markdown", &["md", "markdown"]),
        ("yaml", &["yaml", "yml"]),
        ("toml", &["toml"]),
        ("xml", &["xml"]),
        ("dtd", &["dtd"]),
        ("diff", &["diff", "patch"]),
        ("csv", &["csv"]),
        ("psv", &["psv"]),
        ("tsv", &["tsv"]),
        ("pem", &["pem", "crt", "cer"]),
        ("make", &["mk"]),
    ];

    let syntaxes = entries
        .iter()
        .map(|(grammar, extensions)| {
            (
                grammar.to_string(),
                extensions.iter().map(|ext| ext.to_string()).collect(),
            )
        })
        .collect::<HashMap<String, Vec<String>>>();

    // Covers common languages beyond the shipped grammars on purpose -
    // toggle-comment is useful in files JumpPad can't highlight.
    let comment_styles: &[(&[&str], &str)] = &[
        (&["rs", "c", "h", "cpp", "hpp", "js", "ts", "jsx", "tsx", "java", "go"], "// "),
        (&["py", "sh", "bash", "zsh", "rb", "yaml", "yml", "toml", "mk"], "# "),
        (&["lua", "sql"], "-- "),
        (&["ini"], "; "),
    ];

    let comment_styles = comment_styles
        .iter()
        .map(|(languages, prefix)| CommentStyle {
            languages: languages.iter().map(|ext| ext.to_string()).collect(),
            prefix: prefix.to_string(),
        })
        .collect();

    Config {
        syntaxes: SyntaxesConfig(syntaxes),
        theme: "Light".to_string(),
        visor: VisorConfig::default(),
        alpha: AlphaConfig::default(),
        window: WindowConfig::default(),
        comment_styles,
    }
}
