use crate::{
    AlphaConfig, CommentSyntax, Config, FilesConfig, FontConfig, HistoryConfig,
    LanguageConfig, ScrollConfig, VisorConfig, WindowConfig,
};

fn lang(
    name: &str,
    syntax: &str,
    extensions: &[&str],
    comment: Option<CommentSyntax>,
) -> LanguageConfig {
    LanguageConfig {
        name: name.to_string(),
        syntax: Some(syntax.to_string()),
        extensions: extensions.iter().map(|ext| ext.to_string()).collect(),
        comment,
    }
}

fn single(prefix: &str) -> Option<CommentSyntax> {
    Some(CommentSyntax::Single(prefix.to_string()))
}

fn multi(left: &str, right: &str) -> Option<CommentSyntax> {
    Some(CommentSyntax::Multi {
        left: left.to_string(),
        right: right.to_string(),
    })
}

/// Built-in defaults, written out as the config file on first run.
/// Languages without a shipped grammar still name a plausible `syntax` -
/// a missing .wasm just means no highlighting until one is added.
pub(crate) fn config() -> Config {
    let languages = vec![
        lang("JSON", "json", &["json"], None),
        lang("Markdown", "markdown", &["md", "markdown"], None),
        lang("YAML", "yaml", &["yaml", "yml"], single("# ")),
        lang("TOML", "toml", &["toml"], single("# ")),
        lang("XML", "xml", &["xml"], multi("<!--", "-->")),
        lang(
            "HTML",
            "html",
            &["htm", "html", "xhtml"],
            multi("<!--", "-->"),
        ),
        lang("DTD", "dtd", &["dtd"], multi("<!--", "-->")),
        lang("Diff", "diff", &["diff", "patch"], None),
        lang("CSV", "csv", &["csv"], None),
        lang("PSV", "psv", &["psv"], None),
        lang("TSV", "tsv", &["tsv"], None),
        lang("PEM", "pem", &["pem", "crt", "cer"], None),
        lang("Make", "make", &["mk"], single("# ")),
        lang("Rust", "rust", &["rs"], single("// ")),
        lang("C", "c", &["c", "h"], single("// ")),
        lang(
            "C++",
            "cpp",
            &["cpp", "hpp", "cc", "hh", "cxx"],
            single("// "),
        ),
        lang(
            "JavaScript",
            "javascript",
            &["js", "mjs", "cjs", "jsx"],
            single("// "),
        ),
        lang("TypeScript", "typescript", &["ts", "tsx"], single("// ")),
        lang("Java", "java", &["java"], single("// ")),
        lang("Go", "go", &["go"], single("// ")),
        lang("Python", "python", &["py"], single("# ")),
        lang("Shell", "bash", &["sh", "bash", "zsh"], single("# ")),
        lang("Ruby", "ruby", &["rb"], single("# ")),
        lang("Lua", "lua", &["lua"], single("-- ")),
        lang("SQL", "sql", &["sql"], single("-- ")),
    ];

    Config {
        theme: "Light".to_string(),
        visor: VisorConfig::default(),
        alpha: AlphaConfig::default(),
        window: WindowConfig::default(),
        scroll: ScrollConfig::default(),
        history: HistoryConfig::default(),
        files: FilesConfig::default(),
        font: FontConfig::default(),
        languages,
    }
}
