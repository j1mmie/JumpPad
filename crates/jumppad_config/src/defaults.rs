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

    // The default [syntaxes] entries that have a line comment.
    let comment_styles = vec![CommentStyle {
        syntaxes: vec!["toml".to_string(), "yaml".to_string(), "make".to_string()],
        prefix: "# ".to_string(),
    }];

    Config {
        syntaxes: SyntaxesConfig(syntaxes),
        theme: "Light".to_string(),
        visor: VisorConfig::default(),
        alpha: AlphaConfig::default(),
        window: WindowConfig::default(),
        comment_styles,
    }
}
