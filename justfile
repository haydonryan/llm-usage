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
    cargo deny check --config deny.toml all
    cargo test --workspace

install:
    cargo install --path . --bin llm-usage

run *args:
    cargo run --bin llm-usage -- {{args}}

test:
    cargo test --workspace
