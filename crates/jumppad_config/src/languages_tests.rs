use super::*;
use crate::bundle::SyntaxBundle;

fn bundle(grammar: &str, language: LanguageConfig) -> SyntaxBundle {
    SyntaxBundle {
        grammar: grammar.to_string(),
        // `bundle::read` is what fills this in from the directory name, so a
        // hand-built bundle has to do the same to stand in for a real one.
        language: LanguageConfig {
            syntax: language
                .syntax
                .or_else(|| Some(grammar.to_string())),
            ..language
        },
    }
}

fn yaml_bundle() -> SyntaxBundle {
    bundle(
        "yaml",
        LanguageConfig {
            name: "YAML".to_string(),
            extensions: Some(vec!["yaml".to_string(), "yml".to_string()]),
            comment: Some(CommentSyntax::Single("# ".to_string())),
            ..LanguageConfig::default()
        },
    )
}

fn patch(name: &str) -> LanguageConfig {
    LanguageConfig {
        name: name.to_string(),
        ..LanguageConfig::default()
    }
}

#[test]
fn a_bundle_with_nothing_patching_it_keeps_everything_it_shipped() {
    let languages = Languages::merge(&[yaml_bundle()], &[]);
    assert_eq!(
        languages.extension_to_grammar().get("yml").map(String::as_str),
        Some("yaml")
    );
    assert_eq!(
        languages.comment_styles_by_extension().get("yaml"),
        Some(&CommentSyntax::Single("# ".to_string()))
    );
}

#[test]
fn a_user_entry_patches_only_the_fields_it_names() {
    // The whole point of merging by name: changing one language's extensions
    // must not cost it the comment style the bundle shipped alongside them.
    let languages = Languages::merge(
        &[yaml_bundle()],
        &[LanguageConfig {
            extensions: Some(vec!["yaml".to_string(), "sls".to_string()]),
            ..patch("YAML")
        }],
    );

    let grammars = languages.extension_to_grammar();
    assert_eq!(grammars.get("sls").map(String::as_str), Some("yaml"));
    assert_eq!(
        grammars.get("yml"),
        None,
        "a named extensions list replaces the bundle's rather than adding to it"
    );
    assert_eq!(
        languages.comment_styles_by_extension().get("sls"),
        Some(&CommentSyntax::Single("# ".to_string())),
        "the comment style the patch never mentioned should have survived"
    );
}

#[test]
fn a_user_entry_finds_its_bundle_whatever_case_it_names_it_in() {
    let languages = Languages::merge(
        &[yaml_bundle()],
        &[LanguageConfig {
            comment: Some(CommentSyntax::Single("## ".to_string())),
            ..patch("yaml")
        }],
    );
    assert_eq!(
        languages.entries().len(),
        1,
        "`yaml` should have patched the `YAML` bundle, not added a language"
    );
    assert_eq!(
        languages.comment_styles_by_extension().get("yml"),
        Some(&CommentSyntax::Single("## ".to_string()))
    );
}

#[test]
fn a_user_entry_naming_no_bundle_becomes_a_language_of_its_own() {
    let languages = Languages::merge(
        &[yaml_bundle()],
        &[LanguageConfig {
            syntax: Some("ini".to_string()),
            extensions: Some(vec!["ini".to_string()]),
            comment: Some(CommentSyntax::Single("; ".to_string())),
            ..patch("INI")
        }],
    );
    assert_eq!(languages.entries().len(), 2);
    assert_eq!(
        languages.extension_to_grammar().get("ini").map(String::as_str),
        Some("ini")
    );
}

#[test]
fn a_bundle_names_its_own_directory_as_its_grammar() {
    // The directory is the grammar name, so a bundle that says nothing about
    // `syntax` still highlights - and one that does gets to disagree, which
    // is how JSONC borrows the json grammar.
    let borrowed = bundle(
        "jsonc",
        LanguageConfig {
            name: "JSONC".to_string(),
            syntax: Some("json".to_string()),
            extensions: Some(vec!["jsonc".to_string()]),
            ..LanguageConfig::default()
        },
    );
    let languages = Languages::merge(&[yaml_bundle(), borrowed], &[]);
    let grammars = languages.extension_to_grammar();
    assert_eq!(grammars.get("yaml").map(String::as_str), Some("yaml"));
    assert_eq!(grammars.get("jsonc").map(String::as_str), Some("json"));
}

#[test]
fn the_maps_lowercase_their_keys_and_let_a_later_entry_win() {
    let languages = Languages::merge(
        &[bundle(
            "cpp",
            LanguageConfig {
                name: "C++".to_string(),
                extensions: Some(vec!["cpp".to_string(), "HPP".to_string()]),
                comment: Some(CommentSyntax::Single("// ".to_string())),
                ..LanguageConfig::default()
            },
        )],
        &[LanguageConfig {
            extensions: Some(vec!["cpp".to_string()]),
            comment: Some(CommentSyntax::Single("# ".to_string())),
            ..patch("Rewrap")
        }],
    );

    let styles = languages.comment_styles_by_extension();
    assert_eq!(
        styles.get("cpp"),
        Some(&CommentSyntax::Single("# ".to_string())),
        "the user's own language is later, so it wins the extension"
    );
    assert_eq!(
        styles.get("hpp"),
        Some(&CommentSyntax::Single("// ".to_string())),
        "an extension only the bundle claims keeps the bundle's style"
    );

    let grammars = languages.extension_to_grammar();
    assert_eq!(
        grammars.get("hpp").map(String::as_str),
        Some("cpp"),
        "extensions are matched lowercased however the config spelled them"
    );
    assert_eq!(
        grammars.get("cpp").map(String::as_str),
        Some("cpp"),
        "a syntax-less entry contributes nothing to the grammar map, so it \
         takes the comment style without also unsetting the highlighting"
    );
}

#[test]
fn a_fence_can_name_a_language_by_grammar_alias_or_extension() {
    let languages = Languages::merge(
        &[bundle(
            "javascript",
            LanguageConfig {
                name: "JavaScript".to_string(),
                extensions: Some(vec!["js".to_string(), "mjs".to_string()]),
                aliases: Some(vec!["node".to_string()]),
                ..LanguageConfig::default()
            },
        )],
        &[],
    );

    let fences = languages.fence_language_to_grammar();
    for named in ["javascript", "node", "js", "mjs"] {
        assert_eq!(
            fences.get(named).map(String::as_str),
            Some("javascript"),
            "```{named} should reach the javascript grammar"
        );
    }
    assert_eq!(fences.get("rust"), None);
}

#[test]
fn only_a_language_that_asks_for_one_reports_a_symbol() {
    let languages = Languages::merge(
        &[
            yaml_bundle(),
            bundle(
                "php_only",
                LanguageConfig {
                    name: "PHP".to_string(),
                    extensions: Some(vec!["php".to_string()]),
                    symbol: Some("php".to_string()),
                    ..LanguageConfig::default()
                },
            ),
        ],
        &[],
    );
    let symbols = languages.grammar_symbols();
    assert_eq!(symbols.get("php_only").map(String::as_str), Some("php"));
    assert_eq!(
        symbols.get("yaml"),
        None,
        "a grammar exporting its own name needs no entry"
    );
}
