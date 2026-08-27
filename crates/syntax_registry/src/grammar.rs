use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tree_sitter::{
    Language, Parser, Query, QueryCursor, QueryMatch, StreamingIterator,
};

use crate::highlight::{self, HighlightSpan};
use crate::{Handle, PollResult, SyntaxRegistry};

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
    /// The grammars named by a code fence rather than by the query, found
    /// only once there is a document to read them out of. Holds only the
    /// names that resolved - see `fence_language`.
    fence_injected: Mutex<HashMap<String, Handle>>,
    /// How the fence grammars above get acquired. Weak because the registry
    /// owns every loaded `Grammar`, and an owning handle back would be a
    /// cycle it could never break.
    registry: Weak<SyntaxRegistry>,
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
        registry: Weak<SyntaxRegistry>,
    ) -> Self {
        Self {
            language,
            injections,
            injected,
            fence_injected: Mutex::new(HashMap::new()),
            registry,
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
        let loading =
            |handle: &Handle| matches!(handle.poll(), PollResult::Loading);
        self.inner.lock().unwrap().injections_pending
            || self.injected.values().any(loading)
            || self.fence_injected.lock().unwrap().values().any(loading)
    }

    /// Returns highlight spans for `source`, reparsing only if `source`
    /// changed or a pending injection might have resolved since.
    pub fn highlight(&self, source: &str) -> Arc<Vec<HighlightSpan>> {
        // Taken before the parser lock and released after it, so a grammar
        // that ends up injecting itself is recognized rather than waited on.
        let _highlighting = Highlighting::entered(self);
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
            let content_capture_index = capture_index(query, "injection.content");
            let language_capture_index =
                capture_index(query, "injection.language");

            // Collected rather than spliced in per-match, since `QueryCursor::matches`
            // isn't guaranteed to yield matches in byte order.
            let mut injected_spans: Vec<HighlightSpan> = Vec::new();

            let mut cursor = QueryCursor::new();
            let mut matches =
                cursor.matches(query, tree.root_node(), source.as_bytes());
            while let Some(matched) = matches.next() {
                let target = self.injection_target(
                    query,
                    matched,
                    source,
                    language_capture_index,
                );
                let inner_grammar = match target {
                    Some(PollResult::Ready(grammar)) => grammar,
                    Some(PollResult::Loading) => {
                        injections_pending = true;
                        continue;
                    }
                    Some(PollResult::Unavailable) | None => continue,
                };
                // A ```markdown fence inside a Markdown file asks this
                // grammar to highlight itself, part-way through its own
                // parse. That fence goes uncolored rather than deadlocking.
                if already_highlighting(&inner_grammar) {
                    continue;
                }
                let Some(content_capture_index) = content_capture_index else {
                    continue;
                };

                for capture in matched.captures {
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

    /// The grammar one query match wants its content highlighted with.
    ///
    /// Two kinds of pattern reach here. Most name their language outright
    /// (`(#set! injection.language "yaml")`) and were acquired when this
    /// grammar loaded. A code fence instead captures the name out of the
    /// document (` ```rust `), which can only be read now.
    ///
    /// `None` means the match named no language this registry has.
    fn injection_target(
        &self,
        query: &Query,
        matched: &QueryMatch,
        source: &str,
        language_capture_index: Option<usize>,
    ) -> Option<PollResult> {
        if let Some(named) = static_language(query, matched.pattern_index) {
            return Some(self.injected.get(named)?.poll());
        }
        let captured =
            capture_text(matched, language_capture_index?, source)?;
        Some(self.fence_language(captured))
    }

    /// The grammar a fence's info string names, acquired the first time this
    /// grammar sees that name and remembered afterwards - a document with a
    /// hundred `json` fences asks the registry once.
    ///
    /// A name nothing claims is deliberately *not* remembered: the lookup
    /// that missed was already cheap, and a document naming a thousand
    /// languages JumpPad has never heard of would otherwise keep a row for
    /// every one of them.
    fn fence_language(&self, info_string: &str) -> PollResult {
        // ```rust,no_run and ```js {highlight=2} both name their language
        // first; the rest of the info string is the fence's own business.
        let name = info_string
            .split(|character: char| {
                character.is_whitespace() || character == ','
            })
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if name.is_empty() {
            return PollResult::Unavailable;
        }

        let mut fence_injected = self.fence_injected.lock().unwrap();
        if let Some(handle) = fence_injected.get(&name) {
            return handle.poll();
        }
        let acquired = self
            .registry
            .upgrade()
            .and_then(|registry| registry.acquire_fence_language(&name));
        let Some(handle) = acquired else {
            return PollResult::Unavailable;
        };
        fence_injected.entry(name).or_insert(handle).poll()
    }
}

fn capture_index(query: &Query, name: &str) -> Option<usize> {
    query
        .capture_names()
        .iter()
        .position(|&capture| capture == name)
}

/// The language a pattern names outright, if it names one at all.
fn static_language(query: &Query, pattern_index: usize) -> Option<&str> {
    query
        .property_settings(pattern_index)
        .iter()
        .find(|property| &*property.key == "injection.language")
        .and_then(|property| property.value.as_deref())
}

/// The source text one of a match's captures covers.
fn capture_text<'a>(
    matched: &QueryMatch,
    capture_index: usize,
    source: &'a str,
) -> Option<&'a str> {
    matched
        .captures
        .iter()
        .find(|capture| capture.index as usize == capture_index)
        .and_then(|capture| {
            source.get(capture.node.start_byte()..capture.node.end_byte())
        })
}

thread_local! {
    /// The grammars part-way through a `highlight` call on this thread, by
    /// address.
    ///
    /// A grammar reached through an injection is normally a different one,
    /// but nothing stops a document from naming the grammar reading it - a
    /// ```markdown fence inside Markdown is the ordinary case, and a longer
    /// cycle between two grammars' injections is possible in principle.
    /// Recursing would re-enter a parser this thread already holds.
    static HIGHLIGHTING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Marks a grammar as being highlighted for as long as it is held.
struct Highlighting(usize);

impl Highlighting {
    fn entered(grammar: &Grammar) -> Self {
        let address = std::ptr::from_ref(grammar) as usize;
        HIGHLIGHTING.with(|active| active.borrow_mut().push(address));
        Self(address)
    }
}

impl Drop for Highlighting {
    fn drop(&mut self) {
        HIGHLIGHTING.with(|active| {
            let mut active = active.borrow_mut();
            if let Some(index) =
                active.iter().rposition(|&address| address == self.0)
            {
                active.remove(index);
            }
        });
    }
}

fn already_highlighting(grammar: &Arc<Grammar>) -> bool {
    let address = Arc::as_ptr(grammar) as usize;
    HIGHLIGHTING.with(|active| active.borrow().contains(&address))
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
