#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
# One verification run owns these outputs; failures retain evidence for diagnosis.
npm run generate
git diff --exit-code -- web/src/generated
npm run typecheck
npm run build
cargo fmt --all -- --check
cargo build --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo nextest run --locked --workspace --all-features
cargo test --locked --workspace --all-features --doc
cargo check --locked -p omloom-store --no-default-features --target thumbv7em-none-eabihf
cargo run --locked -p omloom-store --example smoke
cargo run --locked --example corpus -- dist/corpus
npm run test:contracts
cargo run --locked --example verify -- dist/corpus
npm run test:browser
cargo clean
npm run clean
