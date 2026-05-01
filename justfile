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
    ./scripts/scan-staged-secrets.sh
    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::pedantic -W clippy::nursery
    cargo audit
    cargo deny check --config deny.toml all
    cargo test --workspace

release *args:
    git pull --rebase
    cargo release {{args}}

run *args:
    cargo run --bin llm-usage -- {{args}}

test:
    cargo test --workspace
