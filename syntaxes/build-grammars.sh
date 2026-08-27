#!/usr/bin/env bash
set -euo pipefail

# Builds every tree-sitter grammar into the bundle directory that already
# describes it - `syntaxes/<grammar>/syntax.wasm`, beside the `config.toml`
# committed there, plus the `injections.scm` for the grammars that embed
# others.
#
# The bundle directories are the committed half and this script fills in the
# half that isn't: what it writes is gitignored, and `syntaxes/` is the whole
# folder to ship next to a JumpPad binary once this has run.
#
# Requires `git` and a way to run the tree-sitter CLI - this uses
# `npx tree-sitter-cli`, which npm downloads into its own cache on first use.

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/syntaxes"
work="$out/tmp"

echo "building into $out"

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
    if [ ! -f "$out/$grammar/config.toml" ]; then
        echo "no syntaxes/$grammar/config.toml - add one before building it" >&2
        exit 1
    fi
    echo "$out/$grammar"
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

cd "$out"
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

echo "done: $(ls -d "$out"/*/syntax.wasm | wc -l) grammars built"
