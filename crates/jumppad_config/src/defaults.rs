use std::collections::HashMap;

use crate::{AlphaConfig, Config, SyntaxesConfig, VisorConfig, WindowConfig};

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

    Config {
        syntaxes: SyntaxesConfig(syntaxes),
        theme: "Light".to_string(),
        visor: VisorConfig::default(),
        alpha: AlphaConfig::default(),
        window: WindowConfig::default(),
    }
}
