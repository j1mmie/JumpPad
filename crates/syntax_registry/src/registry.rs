use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tree_sitter::{wasmtime, Language, Query};

use crate::grammar::Grammar;
use crate::loader;

enum Entry {
    Loading,
    Loaded(Arc<Grammar>),
    // Reason kept for future UI surfacing (logged via eprintln! today, not read back).
    Unavailable(#[allow(dead_code)] String),
}

/// Result of checking on a previously-`acquire`d extension.
pub enum PollResult {
    Loading,
    Ready(Arc<Grammar>),
    Unavailable,
}

/// Loads, caches, and reference-counts tree-sitter WASM grammars, keyed by
/// grammar name (the `<name>.wasm` file) rather than by file extension -
/// several extensions (`yaml`/`yml`) can map to the one grammar and must
/// share a single loaded instance. Deliberately has no `egui` dependency so
/// a future, different `TextEditorWidget` implementation could reuse it.
pub struct SyntaxRegistry {
    engine: wasmtime::Engine,
    search_dirs: Vec<PathBuf>,
    extension_to_grammar: HashMap<String, String>,
    // Called (on a background thread) whenever any grammar load - top-level
    // or one loaded internally as another grammar's injection target -
    // resolves. A single shared callback rather than one per `acquire` call
    // is enough: there's only one UI to wake up, regardless of which
    // grammar just became ready.
    on_ready: Box<dyn Fn() + Send + Sync>,
    state: Mutex<HashMap<String, (Entry, usize)>>,
    // Serializes the actual wasmtime compilation step (`loader::load`)
    // across all background loader threads. Multiple grammars can be
    // *requested* concurrently (dedup/refcounting in `state` still happens
    // independently per grammar), but letting more than one thread compile
    // a different wasm module at the same time through the same shared
    // `wasmtime::Engine` was observed to hang indefinitely in practice
    // (reproduced directly: 4 concurrent first-time `acquire` calls with no
    // injection queries involved at all). One grammar compiles at a time;
    // this only serializes the one-time load, not repeated highlighting.
    compile_lock: Mutex<()>,
}

impl SyntaxRegistry {
    pub fn new(
        search_dirs: Vec<PathBuf>,
        extension_to_grammar: HashMap<String, String>,
        on_ready: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            engine: wasmtime::Engine::default(),
            search_dirs,
            extension_to_grammar,
            on_ready: Box::new(on_ready),
            state: Mutex::new(HashMap::new()),
            compile_lock: Mutex::new(()),
        })
    }

    /// Registers interest in the grammar configured for `extension` (see
    /// `xizor_config`'s `syntaxes` table). If `extension` has no configured
    /// grammar, the returned `Handle` just reports `Unavailable` forever -
    /// no search or thread spawn happens for it. Drop the `Handle` (e.g. by
    /// dropping the owning widget) to release this reservation.
    pub fn acquire(self: &Arc<Self>, extension: &str) -> Handle {
        let Some(grammar) = self.extension_to_grammar.get(extension).cloned() else {
            return Handle {
                registry: self.clone(),
                grammar: None,
            };
        };
        self.acquire_grammar(&grammar)
    }

    /// Core acquire logic, keyed directly by grammar name rather than file
    /// extension. Used by `acquire` (after resolving an extension via
    /// config) and internally by `load_injections` - an injection target
    /// named in a query (e.g. `"yaml"`) is already a resolved grammar name,
    /// not a file extension, and must bypass `extension_to_grammar` (which
    /// only coincidentally has matching entries for some grammar names).
    fn acquire_grammar(self: &Arc<Self>, grammar_name: &str) -> Handle {
        let mut state = self.state.lock().unwrap();
        match state.get_mut(grammar_name) {
            Some((_, refcount)) => {
                eprintln!(
                    "syntax_registry: {grammar_name}: reusing cached/in-flight entry (refcount -> {})",
                    *refcount + 1
                );
                *refcount += 1;
            }
            None => {
                eprintln!("syntax_registry: {grammar_name}: no cached entry, spawning load");
                state.insert(grammar_name.to_owned(), (Entry::Loading, 1));
                drop(state);

                let registry = self.clone();
                let grammar_name = grammar_name.to_owned();
                std::thread::spawn(move || registry.finish_load(&grammar_name));
            }
        }

        Handle {
            registry: self.clone(),
            grammar: Some(grammar_name.to_owned()),
        }
    }

    fn finish_load(self: Arc<Self>, grammar_name: &str) {
        let result = loader::find_wasm(&self.search_dirs, grammar_name)
            .ok_or_else(|| "no matching .wasm file found".to_string())
            .and_then(|path| {
                // See `compile_lock`'s doc comment: only one thread may be
                // inside wasmtime compilation at a time, whole-registry-wide.
                let _compile_guard = self.compile_lock.lock().unwrap();
                loader::load(&self.engine, &path, grammar_name)
            });

        let result = result.map(|(language, parser)| {
            let (injections, injected) = self.load_injections(grammar_name, &language);
            Grammar::new(language, parser, injections, injected)
        });

        {
            let mut state = self.state.lock().unwrap();
            let Some((entry, _)) = state.get_mut(grammar_name) else {
                return; // shouldn't happen: our own insert put this here
            };
            if !matches!(entry, Entry::Loading) {
                return; // already resolved by someone else - nothing to do
            }
            *entry = match result {
                Ok(grammar) => {
                    eprintln!("syntax_registry: {grammar_name}: loaded successfully");
                    Entry::Loaded(Arc::new(grammar))
                }
                Err(reason) => {
                    eprintln!("syntax_registry: {grammar_name}: {reason}");
                    Entry::Unavailable(reason)
                }
            };
        } // lock released here, before calling out

        (self.on_ready)();
    }

    /// Finds and compiles `<grammar_name>.injections.scm` if present, and
    /// eagerly acquires every *statically* named injection target it
    /// declares (`(#set! injection.language "yaml")`). Targets whose
    /// language name is only known dynamically (read from a capture at
    /// match time, e.g. a fenced code block's info string) are skipped
    /// entirely - not supported this slice.
    fn load_injections(
        self: &Arc<Self>,
        grammar_name: &str,
        language: &Language,
    ) -> (Option<Query>, HashMap<String, Handle>) {
        let Some(source) = loader::find_injections_source(&self.search_dirs, grammar_name) else {
            return (None, HashMap::new());
        };

        let query = match Query::new(language, &source) {
            Ok(query) => query,
            Err(err) => {
                eprintln!(
                    "syntax_registry: {grammar_name}: injections.scm failed to compile, ignoring: {err}"
                );
                return (None, HashMap::new());
            }
        };

        let mut targets = BTreeSet::new();
        for pattern_index in 0..query.pattern_count() {
            for property in query.property_settings(pattern_index) {
                if &*property.key == "injection.language" {
                    if let Some(value) = &property.value {
                        targets.insert(value.to_string());
                    }
                }
            }
        }

        let injected = targets
            .into_iter()
            .map(|name| {
                let handle = self.acquire_grammar(&name);
                (name, handle)
            })
            .collect();

        (Some(query), injected)
    }

    fn poll(&self, grammar_name: &str) -> PollResult {
        match self.state.lock().unwrap().get(grammar_name) {
            Some((Entry::Loading, _)) | None => PollResult::Loading,
            Some((Entry::Loaded(grammar), _)) => PollResult::Ready(grammar.clone()),
            Some((Entry::Unavailable(_), _)) => PollResult::Unavailable,
        }
    }

    fn release(&self, grammar_name: &str) {
        // Evicting a grammar with injected sub-grammars drops its `Handle`s
        // for them, which calls back into `release` for those - so the
        // removed entry must be dropped *after* the lock guard below is
        // released, not while still holding it. `std::sync::Mutex` isn't
        // reentrant: dropping it while still locked would self-deadlock
        // this thread forever the moment any grammar with injections is
        // actually evicted (reproduced directly while building this).
        let removed = {
            let mut state = self.state.lock().unwrap();
            let Some((_, refcount)) = state.get_mut(grammar_name) else {
                return;
            };
            *refcount -= 1;
            eprintln!("syntax_registry: {grammar_name}: released (refcount -> {refcount})");
            if *refcount == 0 {
                eprintln!("syntax_registry: {grammar_name}: evicting, no tabs left using it");
                state.remove(grammar_name)
            } else {
                None
            }
        }; // lock released here
        drop(removed); // frees the Grammar (and cascades into its injected Handles) outside the lock
    }
}

/// RAII reservation on a grammar. Dropping it releases the reservation,
/// unloading the grammar once the last `Handle` for it is gone. Holds
/// `None` if the extension it was acquired for has no configured grammar
/// at all - `poll` then just reports `Unavailable` with no registry
/// bookkeeping involved.
pub struct Handle {
    registry: Arc<SyntaxRegistry>,
    grammar: Option<String>,
}

impl Handle {
    pub fn poll(&self) -> PollResult {
        match &self.grammar {
            Some(grammar) => self.registry.poll(grammar),
            None => PollResult::Unavailable,
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(grammar) = &self.grammar {
            self.registry.release(grammar);
        }
    }
}
