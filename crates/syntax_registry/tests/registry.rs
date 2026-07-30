use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use syntax_registry::{HighlightCategory, PollResult, SyntaxRegistry};

fn fixtures_dir() -> Vec<PathBuf> {
    vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")]
}

fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(ext, grammar)| (ext.to_string(), grammar.to_string()))
        .collect()
}

fn wait_ready(rx: &mpsc::Receiver<()>) {
    rx.recv_timeout(Duration::from_secs(5))
        .expect("on_ready callback should fire within 5s");
}

#[test]
fn reuses_already_loaded_grammar_and_unloads_when_last_tab_closes() {
    let (tx, rx) = mpsc::channel();
    let registry = SyntaxRegistry::new(fixtures_dir(), map(&[("json", "json")]), move || {
        let _ = tx.send(());
    });

    // First tab opens a .json file: nothing cached yet, triggers a load.
    let handle1 = registry.acquire("json");
    wait_ready(&rx);
    let grammar1 = match handle1.poll() {
        PollResult::Ready(g) => g,
        _ => panic!("expected Ready after the load completed"),
    };

    // Second tab opens another .json file: must reuse the cached grammar
    // immediately (a synchronous cache hit - no waiting required).
    let handle2 = registry.acquire("json");
    let grammar2 = match handle2.poll() {
        PollResult::Ready(g) => g,
        _ => panic!("expected an immediate cache hit on the second acquire"),
    };
    assert!(
        Arc::ptr_eq(&grammar1, &grammar2),
        "second tab should reuse the exact same loaded grammar, not reload it"
    );

    // Closing the first tab must not unload the grammar while the second
    // tab still holds a handle.
    drop(handle1);
    assert!(matches!(handle2.poll(), PollResult::Ready(_)));

    // Closing the last tab must unload it.
    drop(handle2);

    // Reacquiring after full eviction must trigger a fresh load - proving
    // it was actually unloaded, not just still cached.
    let handle3 = registry.acquire("json");
    wait_ready(&rx);
    assert!(matches!(handle3.poll(), PollResult::Ready(_)));
}

#[test]
fn multiple_extensions_mapped_to_one_grammar_share_a_single_load() {
    // "yaml" and "yml" both configured to use the same "yaml" grammar,
    // mirroring jumppad_config's default `yaml = ["yaml", "yml"]` entry.
    let (tx, rx) = mpsc::channel();
    let registry = SyntaxRegistry::new(
        fixtures_dir(),
        map(&[("yaml", "yaml"), ("yml", "yaml")]),
        move || {
            let _ = tx.send(());
        },
    );

    let handle_yaml = registry.acquire("yaml");
    wait_ready(&rx);
    let grammar_yaml = match handle_yaml.poll() {
        PollResult::Ready(g) => g,
        _ => panic!("expected the .yaml tab's grammar to load"),
    };

    // A .yml tab must reuse the exact same loaded grammar as the .yaml
    // tab, not trigger a second load of the same underlying file.
    let handle_yml = registry.acquire("yml");
    let grammar_yml = match handle_yml.poll() {
        PollResult::Ready(g) => g,
        _ => panic!("expected an immediate cache hit for the .yml tab"),
    };
    assert!(
        Arc::ptr_eq(&grammar_yaml, &grammar_yml),
        ".yaml and .yml should share the exact same loaded grammar"
    );

    // Only closing *both* tabs should unload it.
    drop(handle_yaml);
    assert!(matches!(handle_yml.poll(), PollResult::Ready(_)));
    drop(handle_yml);

    let handle_again = registry.acquire("yaml");
    wait_ready(&rx);
    assert!(matches!(handle_again.poll(), PollResult::Ready(_)));
}

#[test]
fn unconfigured_extension_is_unavailable_without_touching_disk_or_hanging() {
    // "md" is deliberately absent from the map - no grammar configured for
    // it at all, distinct from "configured but the file is missing".
    let registry = SyntaxRegistry::new(fixtures_dir(), map(&[("json", "json")]), || {
        panic!("on_ready should never fire - nothing should ever be loaded");
    });

    let handle = registry.acquire("md");
    assert!(matches!(handle.poll(), PollResult::Unavailable));
}

#[test]
fn configured_extension_with_missing_wasm_file_resolves_to_unavailable() {
    // "nope" maps to a grammar with no .wasm file in the fixtures dir -
    // the disk-search-fails path, distinct from "not configured at all" above.
    let (tx, rx) = mpsc::channel();
    let registry = SyntaxRegistry::new(
        fixtures_dir(),
        map(&[("nope", "does-not-exist")]),
        move || {
            let _ = tx.send(());
        },
    );

    let handle = registry.acquire("nope");
    wait_ready(&rx);
    assert!(matches!(handle.poll(), PollResult::Unavailable));
}

#[test]
fn highlighting_parses_json_into_spans_and_caches_unchanged_text() {
    let (tx, rx) = mpsc::channel();
    let registry = SyntaxRegistry::new(fixtures_dir(), map(&[("json", "json")]), move || {
        let _ = tx.send(());
    });
    let handle = registry.acquire("json");
    wait_ready(&rx);
    let grammar = match handle.poll() {
        PollResult::Ready(g) => g,
        _ => panic!("expected grammar to be ready"),
    };

    let source = r#"{"a": "b", "n": 1}"#;
    let spans1 = grammar.highlight(source);
    assert!(
        !spans1.is_empty(),
        "expected at least one highlight span for real JSON content"
    );

    // Same source string again -> must be the literal same cached Arc,
    // proving unchanged text doesn't trigger a reparse.
    let spans2 = grammar.highlight(source);
    assert!(Arc::ptr_eq(&spans1, &spans2));

    // Different source -> must actually reparse.
    let spans3 = grammar.highlight(r#"{"different": true}"#);
    assert!(!Arc::ptr_eq(&spans1, &spans3));
}

#[test]
fn highlighting_parses_xml_despite_pascal_case_node_kinds() {
    // Regression test: tree-sitter-xml names nodes in PascalCase, unlike
    // every other bundled grammar - see `classify`'s case-folding.
    let (tx, rx) = mpsc::channel();
    let registry = SyntaxRegistry::new(fixtures_dir(), map(&[("xml", "xml")]), move || {
        let _ = tx.send(());
    });
    let handle = registry.acquire("xml");
    wait_ready(&rx);
    let grammar = match handle.poll() {
        PollResult::Ready(g) => g,
        _ => panic!("expected grammar to be ready"),
    };

    let source = r#"<!-- a comment --><a b="c"></a>"#;
    let spans = grammar.highlight(source);
    assert!(
        spans.iter().any(|span| span.category == HighlightCategory::Comment),
        "expected the XML comment to be classified as Comment: {spans:?}"
    );
    assert!(
        spans.iter().any(|span| span.category == HighlightCategory::String),
        "expected the quoted attribute value to be classified as String: {spans:?}"
    );
    assert!(
        spans.iter().any(|span| span.category == HighlightCategory::Keyword),
        "expected the tag/attribute names to be classified as Keyword: {spans:?}"
    );
}

#[test]
fn static_injection_highlights_yaml_frontmatter_via_markdown() {
    // Injection targets resolve by grammar name (via markdown.injections.scm),
    // not through this test's extension_to_grammar map - "toml" is deliberately unconfigured.
    let (tx, rx) = mpsc::channel();
    let registry = SyntaxRegistry::new(
        fixtures_dir(),
        map(&[("markdown", "markdown")]),
        move || {
            let _ = tx.send(());
        },
    );

    let handle = registry.acquire("markdown");

    let source = "---\n# a comment\ntitle: Test\n---\n\nSome **bold** text.\n";
    let frontmatter_end = source.rfind("---").unwrap() + "---".len();

    // Repeatedly highlights the same source as injection targets load in
    // the background - exercises `injections_pending`: without it, the
    // cache would freeze at the first result and never pick up the yaml
    // frontmatter's comment once its grammar resolves.
    let mut last_spans = Arc::new(Vec::new());
    for _ in 0..8 {
        if let PollResult::Ready(grammar) = handle.poll() {
            last_spans = grammar.highlight(source);
            let has_comment_in_frontmatter = last_spans.iter().any(|span| {
                span.category == HighlightCategory::Comment && span.end <= frontmatter_end
            });
            if has_comment_in_frontmatter {
                return; // success
            }
        }
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }

    panic!(
        "expected a Comment-category span inside the YAML frontmatter (bytes 0..{frontmatter_end}) \
         after waiting for markdown and its injection targets to load; got: {last_spans:?}"
    );
}
