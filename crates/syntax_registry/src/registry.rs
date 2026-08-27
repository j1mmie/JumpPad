use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tree_sitter::{Language, Query, wasmtime};

use crate::grammar::Grammar;
use crate::loader;

/// Which grammar to load for what, as the configured languages describe it.
///
/// Kept together rather than passed as three maps because they are three
/// views of one answer, and every one of them is keyed by the same grammar
/// names the bundle directories under `syntaxes/` are called.
#[derive(Debug, Clone, Default)]
pub struct GrammarLookup {
    /// File extension (lowercased) -> grammar name.
    pub by_extension: HashMap<String, String>,
    /// A code fence's info string (lowercased) -> grammar name. Also the
    /// set of fence languages worth loading at all.
    pub by_fence_language: HashMap<String, String>,
    /// Grammar name -> its `.wasm` export symbol, for the grammars that
    /// don't export their own name.
    pub symbols: HashMap<String, String>,
}

enum Entry {
    Loading,
    Loaded(Arc<Grammar>),
    // Reason kept for future UI surfacing (logged via `log::warn!` today,
    // not read back).
    Unavailable(#[allow(dead_code)] String),
}

/// Result of checking on a previously-`acquire`d extension.
pub enum PollResult {
    Loading,
    Ready(Arc<Grammar>),
    Unavailable,
}

/// Loads, caches, and reference-counts tree-sitter WASM grammars, keyed by
/// grammar name rather than file extension - several extensions
/// (`yaml`/`yml`) can map to the one grammar and share a loaded instance.
pub struct SyntaxRegistry {
    engine: wasmtime::Engine,
    search_dirs: Vec<PathBuf>,
    lookup: GrammarLookup,
    // Called whenever any grammar load resolves - one shared callback is
    // enough since there's only one UI to wake up.
    on_ready: Box<dyn Fn() + Send + Sync>,
    state: Mutex<HashMap<String, (Entry, usize)>>,
    // Serializes the wasmtime compile step across loader threads - compiling
    // more than one wasm module at a time through the same shared `Engine`
    // was observed to hang indefinitely. Grammars can still be requested
    // concurrently; only the compile itself is serialized.
    compile_lock: Mutex<()>,
    /// Bumped every time a load resolves. A grammar that had to skip an
    /// injection because its target wasn't loaded yet caches an incomplete
    /// result; watching this is how a consumer knows to ask again. Covers
    /// injections nested any number of levels deep, since every load - at
    /// any depth - bumps the same counter.
    revision: AtomicU64,
}

impl SyntaxRegistry {
    pub fn new(
        search_dirs: Vec<PathBuf>,
        lookup: GrammarLookup,
        on_ready: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            engine: wasmtime::Engine::default(),
            search_dirs,
            lookup,
            on_ready: Box::new(on_ready),
            state: Mutex::new(HashMap::new()),
            compile_lock: Mutex::new(()),
            revision: AtomicU64::new(0),
        })
    }

    /// How many grammar loads have resolved so far - see `revision`.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Registers interest in the grammar configured for `extension`. If none
    /// is configured, the returned `Handle` just reports `Unavailable`
    /// forever. Drop the `Handle` to release the reservation.
    ///
    /// Matched without regard to case, so `NOTES.MD` highlights the same as
    /// `notes.md`.
    pub fn acquire(self: &Arc<Self>, extension: &str) -> Handle {
        let extension = extension.to_lowercase();
        let Some(grammar) = self.lookup.by_extension.get(&extension).cloned()
        else {
            return Handle {
                registry: self.clone(),
                grammar: None,
            };
        };
        self.acquire_grammar(&grammar)
    }

    /// Registers interest in the grammar a code fence's info string names.
    ///
    /// `None` for a name no language claims, rather than a `Handle` that
    /// could only resolve to `Unavailable`: a document full of fences in
    /// languages JumpPad has never heard of should cost nothing, not one
    /// loader thread and one cache entry per name.
    pub(crate) fn acquire_fence_language(
        self: &Arc<Self>,
        info_string: &str,
    ) -> Option<Handle> {
        let grammar = self.lookup.by_fence_language.get(info_string)?.clone();
        Some(self.acquire_grammar(&grammar))
    }

    /// Core acquire logic, keyed directly by grammar name - used by
    /// `acquire`, by `acquire_fence_language`, and by `load_injections` (an
    /// injection target is already a resolved grammar name, not a file
    /// extension).
    fn acquire_grammar(self: &Arc<Self>, grammar_name: &str) -> Handle {
        let mut state = self.state.lock().unwrap();
        match state.get_mut(grammar_name) {
            Some((_, refcount)) => {
                log::debug!(
                    "{grammar_name}: reusing cached/in-flight entry (refcount -> {})",
                    *refcount + 1
                );
                *refcount += 1;
            }
            None => {
                log::debug!("{grammar_name}: no cached entry, spawning load");
                state.insert(grammar_name.to_owned(), (Entry::Loading, 1));
                drop(state);

                let registry = self.clone();
                let owned_name = grammar_name.to_owned();
                let spawned = std::thread::Builder::new()
                    // Named, so a panic or a crash anywhere in the load
                    // path reports `grammar-<name>` rather than leaving the
                    // reader to work out which of several threads it was.
                    .name(format!("grammar-{owned_name}"))
                    .spawn(move || registry.finish_load(&owned_name));
                if let Err(err) = spawned {
                    // The entry is already `Loading` and nobody else will
                    // resolve it, so say so here or every future `poll` for
                    // this grammar waits on a thread that never started.
                    log::warn!("{grammar_name}: couldn't start loader: {err}");
                    self.resolve(grammar_name, Err(format!("{err}")));
                }
            }
        }

        Handle {
            registry: self.clone(),
            grammar: Some(grammar_name.to_owned()),
        }
    }

    fn finish_load(self: Arc<Self>, grammar_name: &str) {
        let symbol = self
            .lookup
            .symbols
            .get(grammar_name)
            .map_or(grammar_name, String::as_str);
        let result = loader::find_wasm(&self.search_dirs, grammar_name)
            .ok_or_else(|| "no syntax.wasm in a bundle of this name".to_string())
            .and_then(|path| {
                let _compile_guard = self.compile_lock.lock().unwrap();
                loader::load(&self.engine, &path, symbol)
            });

        let result = result.map(|(language, parser)| {
            let (injections, injected) =
                self.load_injections(grammar_name, &language);
            Grammar::new(
                language,
                parser,
                injections,
                injected,
                Arc::downgrade(&self),
            )
        });

        self.resolve(grammar_name, result);
    }

    /// Installs a load's outcome and wakes whoever was waiting on it.
    ///
    /// Split out of `finish_load` because the loader thread is not the only
    /// way a load ends: failing to spawn one at all has to land the same
    /// way, or the `Loading` entry already in the map is never answered and
    /// every later `poll` waits on a thread that does not exist.
    fn resolve(&self, grammar_name: &str, result: Result<Grammar, String>) {
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
                    log::debug!("{grammar_name}: loaded successfully");
                    Entry::Loaded(Arc::new(grammar))
                }
                Err(reason) => {
                    log::warn!("{grammar_name}: {reason}");
                    Entry::Unavailable(reason)
                }
            };
            // Released while the entry is still under the lock, so anyone who
            // reads the new revision is guaranteed to see the entry it
            // announces rather than the `Loading` it replaced.
            self.revision.fetch_add(1, Ordering::Release);
        } // lock released here, before calling out

        (self.on_ready)();
    }

    /// Finds and compiles `<grammar_name>.injections.scm` if present, and
    /// eagerly acquires every *statically* named injection target
    /// (`(#set! injection.language "yaml")`). Dynamically-named targets
    /// (read from a capture at match time) aren't supported yet.
    fn load_injections(
        self: &Arc<Self>,
        grammar_name: &str,
        language: &Language,
    ) -> (Option<Query>, HashMap<String, Handle>) {
        let Some(source) =
            loader::find_injections_source(&self.search_dirs, grammar_name)
        else {
            return (None, HashMap::new());
        };

        let query = match Query::new(language, &source) {
            Ok(query) => query,
            Err(err) => {
                log::warn!(
                    "{grammar_name}: injections.scm failed to compile, ignoring: {err}"
                );
                return (None, HashMap::new());
            }
        };

        let mut targets = BTreeSet::new();
        for pattern_index in 0..query.pattern_count() {
            for property in query.property_settings(pattern_index) {
                if &*property.key == "injection.language"
                    && let Some(value) = &property.value
                {
                    targets.insert(value.to_string());
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
            Some((Entry::Loaded(grammar), _)) => {
                PollResult::Ready(grammar.clone())
            }
            Some((Entry::Unavailable(_), _)) => PollResult::Unavailable,
        }
    }

    fn release(&self, grammar_name: &str) {
        // Evicting a grammar with injected sub-grammars drops their
        // `Handle`s, which calls back into `release` - so `removed` must be
        // dropped after the lock guard, not while still holding it, or this
        // self-deadlocks (`Mutex` isn't reentrant).
        let removed = {
            let mut state = self.state.lock().unwrap();
            let Some((_, refcount)) = state.get_mut(grammar_name) else {
                return;
            };
            *refcount -= 1;
            log::debug!("{grammar_name}: released (refcount -> {refcount})");
            if *refcount == 0 {
                log::debug!("{grammar_name}: evicting, no tabs left using it");
                state.remove(grammar_name)
            } else {
                None
            }
        }; // lock released here
        drop(removed); // frees the Grammar (and cascades into its injected Handles) outside the lock
    }
}

/// RAII reservation on a grammar - dropping it releases the reservation,
/// unloading the grammar once the last `Handle` for it is gone.
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
