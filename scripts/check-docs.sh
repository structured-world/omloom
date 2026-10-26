#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
exec lychee --offline --include-fragments --no-progress README.md CONTRIBUTING.md SECURITY.md docs/contracts.md docs/storage.md
