use tree_sitter::{Node, Tree};

/// Coarse highlighting buckets - not a full theme/scope system, but broad
/// enough to cover both code-like and markup-like grammars.
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

/// A run of bytes to color. Spans are ordered by `start` and never overlap -
/// consumers binary-search them per line rather than scanning the document's
/// whole list, so both properties have to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub category: HighlightCategory,
}

pub fn walk(tree: &Tree) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    visit(tree.root_node(), &mut spans);
    // A pre-order walk that stops at each colored node already emits these
    // in order; sorting makes the ordering consumers rely on a guarantee
    // rather than a property of the traversal. Free on an already-sorted
    // run, which is the only case that reaches here.
    spans.sort_by_key(|span| span.start);
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
    // tree-sitter-xml/tree-sitter-dtd name nodes in PascalCase (`Comment`),
    // which the substring checks below would otherwise miss.
    let kind = kind.to_ascii_lowercase();
    let kind = kind.as_str();

    if kind.contains("comment") {
        Some(HighlightCategory::Comment)
    } else if kind.contains("string") || is_xml_quoted_literal(kind) {
        Some(HighlightCategory::String)
    } else if kind.contains("number") {
        Some(HighlightCategory::Number)
    } else if kind.contains("keyword") || kind == "name" {
        // XML/DTD reuse one `Name` node kind for tag, attribute, and
        // entity/PI names alike - bucketed as Keyword so they still read
        // distinct from plain element text.
        Some(HighlightCategory::Keyword)
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

/// XML/DTD's quoted-literal node kinds (attribute values, CDATA, ...) read
/// as strings but don't have "string" in their name.
fn is_xml_quoted_literal(kind: &str) -> bool {
    matches!(
        kind,
        "attvalue" | "cdata" | "systemliteral" | "pubidliteral" | "entityvalue"
    )
}
