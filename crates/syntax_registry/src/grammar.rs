use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use crate::highlight::{self, HighlightSpan};
use crate::{Handle, PollResult};

/// A loaded, ready-to-use tree-sitter grammar: a `Parser` with its wasm
/// module already attached (via `set_wasm_store`/`set_language`), an
/// optional compiled injection query, and eagerly-acquired handles to any
/// statically-named injection target grammars - plus a small cache so
/// repeated calls with unchanged text are nearly free.
///
/// `egui::TextEdit`'s layouter callback runs at least once per frame
/// unconditionally, so this cache is load-bearing for the "don't reparse
/// every frame" efficiency goal, not just a nice-to-have.
pub struct Grammar {
    #[allow(dead_code)] // kept alive alongside the parser; not read directly by Grammar itself
    language: Language,
    injections: Option<Query>,
    injected: HashMap<String, Handle>,
    inner: Mutex<GrammarInner>,
}

struct GrammarInner {
    parser: Parser,
    last_source: String,
    last_spans: Arc<Vec<HighlightSpan>>,
    // Set when the last computation skipped an injection because its target
    // grammar was still `Loading` - so a cache hit on unchanged text must
    // NOT short-circuit, or a target that finishes loading later would
    // never get picked up (the outer source text never changes, so nothing
    // else would ever invalidate the cache).
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

    /// Returns highlight spans for `source`, reparsing only if `source`
    /// differs from the last call (or an injection target that was
    /// pending last time might have resolved since) - otherwise returns
    /// the cached result.
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

            // Stage (range, offset-corrected inner spans) per match without
            // mutating `spans` yet: `QueryCursor::matches` isn't guaranteed
            // to yield matches in byte order, so suppressing base spans and
            // splicing in injected ones interleaved, per match, could let a
            // later match's suppression pass wrongly strip an earlier
            // match's already-spliced injected spans.
            let mut staged: Vec<(usize, usize, Vec<HighlightSpan>)> = Vec::new();

            let mut cursor = QueryCursor::new();
            let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
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
                    let (start, end) = (capture.node.start_byte(), capture.node.end_byte());
                    let Some(sub_source) = source.get(start..end) else {
                        continue;
                    };
                    let inner_spans = inner_grammar
                        .highlight(sub_source)
                        .iter()
                        .map(|span| HighlightSpan {
                            start: start + span.start,
                            end: start + span.end,
                            category: span.category,
                        })
                        .collect();
                    staged.push((start, end, inner_spans));
                }
            }

            if !staged.is_empty() {
                // Cut each injection range out of every base span it
                // overlaps, keeping whatever parts of that span fall
                // outside the injection (rather than dropping the whole
                // span on any overlap) - a heading like `# **bold**` should
                // still show heading color on the `# ` part even though
                // `**bold**` gets overridden by the injected emphasis span.
                let mut result = Vec::with_capacity(spans.len() + staged.len());
                for span in &spans {
                    let mut pieces = vec![(span.start, span.end)];
                    for &(inj_start, inj_end, _) in &staged {
                        pieces = pieces
                            .into_iter()
                            .flat_map(|piece| subtract_range(piece, (inj_start, inj_end)))
                            .collect();
                    }
                    result.extend(pieces.into_iter().map(|(start, end)| HighlightSpan {
                        start,
                        end,
                        category: span.category,
                    }));
                }
                for (_, _, inner_spans) in staged {
                    result.extend(inner_spans);
                }
                // Two different injection matches' content ranges overlapping
                // each other isn't de-overlapped here - doesn't occur in the
                // real markdown query (frontmatter and inline nodes are
                // structurally disjoint), so left as an assumption, not a
                // runtime check.
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
fn subtract_range(piece: (usize, usize), remove: (usize, usize)) -> Vec<(usize, usize)> {
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
