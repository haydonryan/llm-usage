set positional-arguments

default:
    @just --list

build:
    cargo build --workspace

check:
    cargo check --workspace
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic -W clippy::nursery
    cargo audit
    cargo deny check --config deny.toml all
    cargo test --workspace

install:
    cargo install --path . --bin llm-usage

pre-commit:
    #!/usr/bin/env bash
    set -euo pipefail
    ./scripts/scan-staged-secrets.sh
    before_fmt_diff="$(mktemp)"
    after_fmt_diff="$(mktemp)"
    trap 'rm -f "$before_fmt_diff" "$after_fmt_diff"' EXIT
    git diff --name-only -- . >"$before_fmt_diff"
    cargo fmt --all
    git diff --name-only -- . >"$after_fmt_diff"
    if ! cmp -s "$before_fmt_diff" "$after_fmt_diff"; then
      echo "cargo fmt updated files. Review and stage the formatting changes, then commit again." >&2
      exit 1
    fi
    cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic -W clippy::nursery
    cargo audit
    cargo deny check --config deny.toml all
    cargo test --workspace

release *args:
    git pull --rebase
    cargo release {{args}}

bump version:
    cargo release version {{version}} --execute --no-confirm

run *args:
    cargo run --bin llm-usage -- {{args}}

test:
    cargo test --workspace
