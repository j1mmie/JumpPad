use tree_sitter::{Node, Tree};

/// Coarse highlighting buckets. Deliberately not a full theme/scope system
/// (see `.scm` query-based highlighting in a future slice), but broad
/// enough to cover both code-like grammars (string/comment/number/keyword)
/// and markup-like ones (heading/emphasis/link/quote/code) - markdown's own
/// node-kind vocabulary shares none of the first four words at all, so it
/// needs these to show any highlighting whatsoever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightCategory {
    String,
    Comment,
    Number,
    Keyword,
    Heading,
    Emphasis,
    Link,
    Quote,
    Code,
}

#[derive(Debug, Clone, Copy)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub category: HighlightCategory,
}

pub fn walk(tree: &Tree) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    visit(tree.root_node(), &mut spans);
    spans
}

fn visit(node: Node, spans: &mut Vec<HighlightSpan>) {
    if let Some(category) = classify(node.kind()) {
        spans.push(HighlightSpan {
            start: node.start_byte(),
            end: node.end_byte(),
            category,
        });
        // Don't recurse into a node we've already colored as a whole -
        // avoids nested children (e.g. escape sequences inside a string)
        // getting a second, conflicting span.
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, spans);
    }
}

fn classify(kind: &str) -> Option<HighlightCategory> {
    // Lowercased once up front: tree-sitter-xml/tree-sitter-dtd name their
    // nodes in PascalCase (`Comment`, not `comment`), which the substring
    // checks below would otherwise silently never match. Checked against
    // every other bundled grammar's actual node-types.json - none of them
    // have a kind that only matches once case is folded, so this is a pure
    // addition, not a behavior change, for anything already working.
    let kind = kind.to_ascii_lowercase();
    let kind = kind.as_str();

    if kind.contains("comment") {
        Some(HighlightCategory::Comment)
    } else if kind.contains("string") || is_xml_quoted_literal(kind) {
        Some(HighlightCategory::String)
    } else if kind.contains("number") {
        Some(HighlightCategory::Number)
    } else if kind.contains("keyword") || kind == "name" {
        // XML/DTD reuse one `Name` node kind for element tag names,
        // attribute names, and entity/PI names alike (confirmed by walking
        // a real parse tree - there's no separate "tag name" vs "attribute
        // name" node kind to tell them apart by kind alone). Bucketing all
        // of it as Keyword still gets tags and attribute names visually
        // distinct from plain element text, which is most of what reads as
        // "no highlighting" otherwise.
        Some(HighlightCategory::Keyword)
    // Markup-oriented node kinds (confirmed against tree-sitter-markdown's
    // and tree-sitter-markdown-inline's actual node-types.json - neither
    // grammar has any node kind matching the four checks above at all).
    } else if kind.contains("heading") {
        Some(HighlightCategory::Heading)
    } else if kind.contains("emphasis") {
        Some(HighlightCategory::Emphasis)
    } else if kind.contains("link") || kind.contains("image") {
        Some(HighlightCategory::Link)
    } else if kind.contains("quote") {
        Some(HighlightCategory::Quote)
    } else if kind.contains("code") {
        Some(HighlightCategory::Code)
    } else {
        None
    }
}

/// XML/DTD's quoted-literal node kinds - attribute values, CDATA sections,
/// and the handful of other places the grammars accept a quoted literal -
/// read as strings but, per tree-sitter-xml's and tree-sitter-dtd's actual
/// node-types.json, none of their names contain the word "string" (already
/// lowercased by the caller).
fn is_xml_quoted_literal(kind: &str) -> bool {
    matches!(
        kind,
        "attvalue" | "cdata" | "systemliteral" | "pubidliteral" | "entityvalue"
    )
}
