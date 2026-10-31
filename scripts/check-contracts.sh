#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
# One verification run owns these outputs; failures retain evidence for diagnosis.
yarn generate
git diff --exit-code -- web/src/generated
yarn typecheck
yarn build
yarn build:app
cargo fmt --all -- --check
cargo build --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo nextest run --locked --workspace --all-features
cargo test --locked --workspace --all-features --doc
cargo check --locked -p omloom-store --no-default-features --target thumbv7em-none-eabihf
# Keep the default-feature runtime checks, using the same workspace selection
# so Cargo reuses dependency features instead of building package-specific graphs.
cargo build --locked --workspace --all-targets
du -sh target
df -h .
target/debug/examples/smoke
target/debug/examples/corpus dist/corpus
yarn test:contracts
target/debug/examples/verify dist/corpus
yarn test:browser
if [ "${CI:-}" = true ]; then
  # Keep dependency artifacts for the cache action; remove this workspace's outputs.
  cargo clean -p omloom-contracts -p omloom-store -p omloom-app -p omloom-observe
else
  cargo clean
fi
yarn clean
