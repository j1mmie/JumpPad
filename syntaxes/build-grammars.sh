#!/usr/bin/env bash
set -euo pipefail

# Builds every tree-sitter grammar, then assembles the folder to ship.
#
# A language is a directory: `<grammar>/config.toml` names its extensions,
# comment style and code-fence aliases, `syntax.wasm` highlights them, and
# `injections.scm` names the other grammars it embeds. The config files are
# committed; this script produces the other two.
#
# It writes to two places, and both are gitignored:
#
#   syntaxes/<grammar>/syntax.wasm   built in place, so a checkout highlights
#                                    under `cargo run` with nothing to copy
#   syntaxes/output/                 every bundle assembled together - the
#                                    whole folder to ship, config files
#                                    included. Rename it `syntaxes` next to
#                                    a JumpPad binary and it is found.
#
# `output/` carries the grammar-less bundles too. Rust and Python ship a
# config and no `.wasm`: no highlighting, but their comment styles are how
# toggle-comment knows what a `.rs` comment looks like, so leaving them out
# would quietly drop a feature.
#
# Requires `git` and a way to run the tree-sitter CLI - this uses
# `npx tree-sitter-cli`, which npm downloads into its own cache on first use.

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
syntaxes="$root/syntaxes"
out="$syntaxes/output"
work="$syntaxes/tmp"

trap 'rm -rf "$work"' EXIT
mkdir -p "$work"

ts() {
    npx --yes tree-sitter-cli "$@";
}

clone() {
    local repo="$1" dir="$2"
    if [ ! -d "$work/$dir" ]; then
        echo "cloning $repo"
        git clone --quiet --depth 1 "https://github.com/$repo.git" "$work/$dir"
    fi
}

# Refuses to write into a directory with no config.toml: the bundle's config
# is what names the grammar's extensions and comment style, and a `.wasm`
# without one is a language JumpPad would never look up.
bundle() {
    local grammar="$1"
    if [ ! -f "$syntaxes/$grammar/config.toml" ]; then
        echo "no syntaxes/$grammar/config.toml - add one before building it" >&2
        exit 1
    fi
    echo "$syntaxes/$grammar"
}

build() {
    local src="$1" grammar="$2"
    echo "building $grammar/syntax.wasm"
    ts build --wasm -o "$(bundle "$grammar")/syntax.wasm" "$src"
}

# An injection query names the other grammars a grammar embeds, by the same
# directory names used here - `injection.language "yaml"` finds syntaxes/yaml.
injections() {
    local src="$1" grammar="$2"
    cp "$src/queries/injections.scm" "$(bundle "$grammar")/injections.scm"
}

# Copies every bundle into `output/`, whether or not a grammar was built for
# it. Rebuilt from scratch each run so a language that has been renamed or
# dropped doesn't linger in a folder someone is about to ship.
assemble() {
    rm -rf "$out"
    mkdir -p "$out"
    local grammars=0 configs_only=0
    for dir in "$syntaxes"/*/; do
        local grammar
        grammar="$(basename "$dir")"
        # Skips output/, tmp/ and node_modules/ without naming them: a
        # directory is a bundle if and only if it has a config.
        [ -f "$dir/config.toml" ] || continue
        mkdir -p "$out/$grammar"
        cp "$dir/config.toml" "$out/$grammar/config.toml"
        if [ -f "$dir/syntax.wasm" ]; then
            cp "$dir/syntax.wasm" "$out/$grammar/syntax.wasm"
            grammars=$((grammars + 1))
        else
            configs_only=$((configs_only + 1))
        fi
        if [ -f "$dir/injections.scm" ]; then
            cp "$dir/injections.scm" "$out/$grammar/injections.scm"
        fi
    done
    echo
    echo "assembled $out"
    echo "  $grammars language(s) with a grammar"
    echo "  $configs_only without one (comment styles only)"
}

cd "$syntaxes"
npm install

clone "ikatyang/tree-sitter-toml" toml
build "$work/toml" toml

clone "tree-sitter/tree-sitter-json" json
build "$work/json" json

clone "tree-sitter/tree-sitter-html" html
build "$work/html" html

clone "tree-sitter-grammars/tree-sitter-yaml" yaml
build "$work/yaml" yaml

clone "tree-sitter-grammars/tree-sitter-diff" diff
build "$work/diff" diff

clone "tree-sitter-grammars/tree-sitter-make" make
build "$work/make" make

clone "tree-sitter-grammars/tree-sitter-pem" pem
build "$work/pem" pem

# xml and csv each bundle multiple grammars as subdirectories of one repo.
clone "tree-sitter-grammars/tree-sitter-xml" xml
build "$work/xml/xml" xml
build "$work/xml/dtd" dtd

clone "tree-sitter-grammars/tree-sitter-csv" csv
build "$work/csv/csv" csv
build "$work/csv/psv" psv
build "$work/csv/tsv" tsv

# Markdown is split in two, and needs both halves: the block grammar leaves
# every link and every bold run to markdown_inline, so a markdown bundle on
# its own colors headings and nothing else. Their injection queries reach
# further still - into yaml, toml and html above, and into whatever language
# a fenced code block names.
clone "tree-sitter-grammars/tree-sitter-markdown" markdown
build "$work/markdown/tree-sitter-markdown" markdown
build "$work/markdown/tree-sitter-markdown-inline" markdown_inline
injections "$work/markdown/tree-sitter-markdown" markdown
injections "$work/markdown/tree-sitter-markdown-inline" markdown_inline

assemble
