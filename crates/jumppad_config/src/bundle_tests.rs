use super::*;

/// A throwaway `syntaxes/` tree that cleans up after itself, so discovery is
/// exercised against real directories rather than a stand-in.
struct TempSyntaxes(PathBuf);

impl TempSyntaxes {
    fn named(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("jumppad_bundle_test_{name}_{:?}", std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn with_bundle(self, grammar: &str, config: &str) -> Self {
        let dir = self.0.join(grammar);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), config).unwrap();
        self
    }

    /// A directory under `syntaxes/` that is not a bundle - `node_modules`
    /// and the build script's clone workdir both live there.
    fn with_stray_dir(self, name: &str) -> Self {
        std::fs::create_dir_all(self.0.join(name)).unwrap();
        self
    }

    fn dirs(&self) -> Vec<PathBuf> {
        vec![self.0.clone()]
    }
}

impl Drop for TempSyntaxes {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_bundle_is_named_by_its_directory_and_read_from_its_config() {
    let syntaxes = TempSyntaxes::named("named")
        .with_bundle("yaml", "name = \"YAML\"\nextensions = [\"yaml\", \"yml\"]\ncomment.single = \"# \"\n");

    let found = discover(&syntaxes.dirs());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].grammar, "yaml");
    assert_eq!(found[0].language.name, "YAML");
    assert_eq!(
        found[0].language.syntax.as_deref(),
        Some("yaml"),
        "a bundle saying nothing about `syntax` is named by its directory"
    );
}

#[test]
fn a_bundle_can_point_at_a_grammar_that_is_not_its_directory() {
    let syntaxes = TempSyntaxes::named("borrowed").with_bundle(
        "jsonc",
        "name = \"JSONC\"\nsyntax = \"json\"\nextensions = [\"jsonc\"]\n",
    );
    let found = discover(&syntaxes.dirs());
    assert_eq!(found[0].language.syntax.as_deref(), Some("json"));
}

#[test]
fn a_directory_without_a_config_is_not_a_bundle() {
    let syntaxes = TempSyntaxes::named("stray")
        .with_bundle("json", "name = \"JSON\"\nextensions = [\"json\"]\n")
        .with_stray_dir("node_modules")
        .with_stray_dir("tmp");

    let found = discover(&syntaxes.dirs());
    assert_eq!(found.len(), 1, "only the real bundle should be found");
    assert_eq!(found[0].grammar, "json");
}

#[test]
fn a_bundle_that_wont_parse_costs_only_its_own_language() {
    let syntaxes = TempSyntaxes::named("broken")
        .with_bundle("json", "name = \"JSON\"\nextensions = [\"json\"]\n")
        .with_bundle("wat", "name = \"Wat\"\nextensions = not-toml\n");

    let found = discover(&syntaxes.dirs());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].grammar, "json");
}

#[test]
fn bundles_come_back_sorted_however_the_directory_listed_them() {
    let syntaxes = TempSyntaxes::named("sorted")
        .with_bundle("yaml", "name = \"YAML\"\nextensions = [\"yaml\"]\n")
        .with_bundle("json", "name = \"JSON\"\nextensions = [\"json\"]\n")
        .with_bundle("toml", "name = \"TOML\"\nextensions = [\"toml\"]\n");

    let grammars: Vec<_> = discover(&syntaxes.dirs())
        .into_iter()
        .map(|bundle| bundle.grammar)
        .collect();
    assert_eq!(grammars, ["json", "toml", "yaml"]);
}

#[test]
fn the_first_search_dir_to_define_a_grammar_wins_it_outright() {
    // Mirrors how the `.wasm` beside it is found: a bundle next to the
    // binary shadows one in the working directory, rather than merging.
    let next_to_binary = TempSyntaxes::named("first")
        .with_bundle("json", "name = \"JSON\"\nextensions = [\"json\"]\n");
    let working_dir = TempSyntaxes::named("second")
        .with_bundle("json", "name = \"Shadowed\"\nextensions = [\"nope\"]\n")
        .with_bundle("toml", "name = \"TOML\"\nextensions = [\"toml\"]\n");

    let dirs = vec![next_to_binary.0.clone(), working_dir.0.clone()];
    let found = discover(&dirs);

    assert_eq!(found.len(), 2, "the shadowed bundle must not be listed twice");
    let json = found.iter().find(|b| b.grammar == "json").unwrap();
    assert_eq!(json.language.name, "JSON");
    assert!(
        found.iter().any(|b| b.grammar == "toml"),
        "a grammar only the later directory defines is still picked up"
    );
}

#[test]
fn a_missing_search_dir_is_simply_empty() {
    let missing = vec![PathBuf::from("does_not_exist/syntaxes")];
    assert!(discover(&missing).is_empty());
}
