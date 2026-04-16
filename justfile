set positional-arguments

default:
    @just --list

build:
    cargo build --workspace

check:
    cargo check --workspace
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo audit
    cargo deny check all
    cargo test --workspace
    cargo test --workspace --all-features
    cargo test --workspace --no-default-features

install:
    cargo install --path . --bin llm-usage

run *args:
    cargo run --bin llm-usage -- {{args}}

test:
    cargo test --workspace
