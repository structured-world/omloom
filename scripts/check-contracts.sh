#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
# One verification run owns these outputs; failures retain evidence for diagnosis.
npm run generate
git diff --exit-code -- web/src/generated
npm run typecheck
npm run build
cargo fmt --all -- --check
cargo build --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo nextest run --locked --workspace
cargo test --locked --workspace --doc
cargo run --locked --example corpus -- dist/corpus
npm run test:contracts
cargo run --locked --example verify -- dist/corpus
npm run test:browser
cargo clean
npm run clean
