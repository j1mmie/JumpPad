use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use crate::highlight::{self, HighlightSpan};
use crate::{Handle, PollResult};

/// A loaded, ready-to-use tree-sitter grammar - a `Parser` with its wasm
/// module attached, an optional compiled injection query, handles to any
/// injection target grammars, and a small cache so repeated calls with
/// unchanged text (the common case, since highlighting is recomputed on
/// every render) are nearly free.
pub struct Grammar {
    #[allow(dead_code)]
    // kept alive alongside the parser; not read directly by Grammar itself
    language: Language,
    injections: Option<Query>,
    injected: HashMap<String, Handle>,
    inner: Mutex<GrammarInner>,
}

struct GrammarInner {
    parser: Parser,
    last_source: String,
    last_spans: Arc<Vec<HighlightSpan>>,
    // Set when an injection was skipped because its target grammar was
    // still loading - forces a recompute even on unchanged text, since
    // nothing else would invalidate the cache once the target's ready.
    injections_pending: bool,
}

impl Grammar {
    pub(crate) fn new(
        language: Language,
        parser: Parser,
        injections: Option<Query>,
        injected: HashMap<String, Handle>,
    ) -> Self {
        Self {
            language,
            injections,
            injected,
            inner: Mutex::new(GrammarInner {
                parser,
                last_source: String::new(),
                last_spans: Arc::new(Vec::new()),
                injections_pending: false,
            }),
        }
    }

    /// Whether the spans this grammar produces are missing injected content:
    /// either the last highlight had to skip an injection target that was
    /// still loading, or one is still loading right now. True on both sides
    /// of the first highlight, so a caller polling on it can't slip through
    /// the gap between the grammar becoming ready and its first parse.
    pub fn injections_unresolved(&self) -> bool {
        self.inner.lock().unwrap().injections_pending
            || self
                .injected
                .values()
                .any(|handle| matches!(handle.poll(), PollResult::Loading))
    }

    /// Returns highlight spans for `source`, reparsing only if `source`
    /// changed or a pending injection might have resolved since.
    pub fn highlight(&self, source: &str) -> Arc<Vec<HighlightSpan>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.last_source == source && !inner.injections_pending {
            return inner.last_spans.clone();
        }

        let Some(tree) = inner.parser.parse(source, None) else {
            let spans = Arc::new(Vec::new());
            inner.last_source = source.to_owned();
            inner.last_spans = spans.clone();
            inner.injections_pending = false;
            return spans;
        };

        let mut spans = highlight::walk(&tree);
        let mut injections_pending = false;

        if let Some(query) = &self.injections {
            let content_capture_index = query
                .capture_names()
                .iter()
                .position(|&name| name == "injection.content");

            // Collected rather than spliced in per-match, since `QueryCursor::matches`
            // isn't guaranteed to yield matches in byte order.
            let mut injected_spans: Vec<HighlightSpan> = Vec::new();

            let mut cursor = QueryCursor::new();
            let mut matches =
                cursor.matches(query, tree.root_node(), source.as_bytes());
            while let Some(m) = matches.next() {
                let language_name = query
                    .property_settings(m.pattern_index)
                    .iter()
                    .find(|property| &*property.key == "injection.language")
                    .and_then(|property| property.value.as_deref());
                let Some(language_name) = language_name else {
                    continue; // dynamic pattern (language read from a capture) - not supported yet
                };
                let Some(handle) = self.injected.get(language_name) else {
                    continue;
                };
                let inner_grammar = match handle.poll() {
                    PollResult::Ready(grammar) => grammar,
                    PollResult::Loading => {
                        injections_pending = true;
                        continue;
                    }
                    PollResult::Unavailable => continue,
                };
                let Some(content_capture_index) = content_capture_index else {
                    continue;
                };

                for capture in m.captures {
                    if capture.index as usize != content_capture_index {
                        continue;
                    }
                    let (start, end) =
                        (capture.node.start_byte(), capture.node.end_byte());
                    let Some(sub_source) = source.get(start..end) else {
                        continue;
                    };
                    injected_spans.extend(
                        inner_grammar.highlight(sub_source).iter().map(
                            |span| HighlightSpan {
                                start: start + span.start,
                                end: start + span.end,
                                category: span.category,
                            },
                        ),
                    );
                    // An injected grammar can have injections of its own, and
                    // its result is just as incomplete while they load.
                    injections_pending |= inner_grammar.injections_unresolved();
                }
            }

            if !injected_spans.is_empty() {
                // Only the bytes an injected span actually covers are cut out
                // of the base spans, not the whole injected region - e.g. a
                // heading like `# **bold**` keeps heading color on `# `
                // where `**bold**` is overridden, and a plain `# Title`, which
                // the inline grammar has nothing to say about, stays heading
                // colored end to end.
                let mut result =
                    Vec::with_capacity(spans.len() + injected_spans.len());
                for span in &spans {
                    let mut pieces = vec![(span.start, span.end)];
                    for injected in &injected_spans {
                        pieces = pieces
                            .into_iter()
                            .flat_map(|piece| {
                                subtract_range(
                                    piece,
                                    (injected.start, injected.end),
                                )
                            })
                            .collect();
                    }
                    result.extend(pieces.into_iter().map(|(start, end)| {
                        HighlightSpan {
                            start,
                            end,
                            category: span.category,
                        }
                    }));
                }
                result.extend(injected_spans);
                result.sort_by_key(|span| span.start);
                spans = result;
            }
        }

        let spans = Arc::new(spans);
        inner.last_source = source.to_owned();
        inner.last_spans = spans.clone();
        inner.injections_pending = injections_pending;
        spans
    }
}

/// Removes the `[remove.0, remove.1)` byte range from `piece`, returning
/// the 0, 1, or 2 remaining sub-ranges that don't overlap it.
fn subtract_range(
    piece: (usize, usize),
    remove: (usize, usize),
) -> Vec<(usize, usize)> {
    let (start, end) = piece;
    let (remove_start, remove_end) = remove;
    if remove_end <= start || remove_start >= end {
        return vec![piece]; // no overlap at all
    }
    let mut out = Vec::new();
    if start < remove_start {
        out.push((start, remove_start));
    }
    if end > remove_end {
        out.push((remove_end, end));
    }
    out
}
