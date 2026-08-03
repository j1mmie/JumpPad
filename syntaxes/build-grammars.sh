#!/usr/bin/env bash
set -euo pipefail

# Rebuilds every tree-sitter grammar `.wasm` file (plus injection queries)
# into syntaxes/ at the repo root, from the upstream sources listed below.
#
# Requires `git` and a way to run the tree-sitter CLI - this uses
# `npx tree-sitter-cli`, which npm downloads into its own cache on first
# use.
#
# These are the actual sources used for what's already committed in
# syntaxes/,

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="output"
work="tmp"

echo "Building in $work"

trap 'rm -rf "$work"' EXIT

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

build() {
    local src="$1" name="$2"
    echo "building $name.wasm"
    ts build --wasm -o "$out/$name.wasm" "$src"
}

npm install
mkdir -p "$out"

clone "ikatyang/tree-sitter-toml" toml
build "$work/toml" toml

clone "tree-sitter-grammars/tree-sitter-yaml" yaml
build "$work/yaml" yaml

clone "tree-sitter/tree-sitter-json" json
build "$work/json" json

clone "tree-sitter-grammars/tree-sitter-diff" diff
build "$work/diff" diff

clone "tree-sitter-grammars/tree-sitter-make" make
build "$work/make" make

clone "tree-sitter-grammars/tree-sitter-pem" pem
build "$work/pem" pem

clone "tree-sitter-grammars/tree-sitter-html" html
build "$work/html" html

# xml and csv each bundle multiple grammars as subdirectories of one repo.
clone "tree-sitter-grammars/tree-sitter-xml" xml
build "$work/xml/xml" xml
build "$work/xml/dtd" dtd

clone "tree-sitter-grammars/tree-sitter-csv" csv
build "$work/csv/csv" csv
build "$work/csv/psv" psv
build "$work/csv/tsv" tsv

# Likewise markdown/markdown-inline - plus each has an injections.scm
# (used by syntax_registry to embed one grammar's content inside another,
# e.g. a YAML frontmatter block inside Markdown) worth keeping in sync.
clone "tree-sitter-grammars/tree-sitter-markdown" markdown
build "$work/markdown/tree-sitter-markdown" markdown
build "$work/markdown/tree-sitter-markdown-inline" markdown_inline
cp "$work/markdown/tree-sitter-markdown/queries/injections.scm" "$out/markdown.injections.scm"
cp "$work/markdown/tree-sitter-markdown-inline/queries/injections.scm" "$out/markdown_inline.injections.scm"

echo "done: $(ls "$out"/*.wasm | wc -l) grammars in $out"
