#!/bin/sh
set -eu

tool=${1:?Specify mold or protoc}
case "$tool:$(uname -s):$(uname -m)" in
  mold:Linux:x86_64)
    repo=rui314/mold
    asset=612122001 # mold-3.0.0-x86_64-linux.tar.gz
    checksum=6c90d4a474c7c0409dfb575be03a5345878ac14fdba18de8b40fa58c60121189
    ;;
  protoc:Linux:x86_64)
    repo=protocolbuffers/protobuf
    asset=570742434 # protoc-36.2-linux-x86_64.zip
    checksum=121f6c7afe1d4d0e3ea6aab9432038599250134cbf4474cb1167d2c7decd4278
    ;;
  protoc:Darwin:arm64)
    repo=protocolbuffers/protobuf
    asset=570742436 # protoc-36.2-osx-aarch_64.zip
    checksum=9cd98a532c5c5e0c4161314de0225de27e4c8a323917b6ea7b1b714d3ae23466
    ;;
  protoc:Darwin:x86_64)
    repo=protocolbuffers/protobuf
    asset=570742448 # protoc-36.2-osx-x86_64.zip
    checksum=228cc7add4616cc14ca5e80dee83209d44449a7aee95a914ae748fa374efb078
    ;;
  *)
    printf 'Unsupported CI tool or platform: %s\n' "$tool" >&2
    exit 2
    ;;
esac

destination=$(mktemp -d "${RUNNER_TEMP:?}/omloom-$tool.XXXXXX")
archive=$(mktemp "${RUNNER_TEMP}/omloom-release.XXXXXX")
trap 'rm -f "$archive"' EXIT

# Use the release asset API because browser download URLs can fail at the GitHub edge.
gh api -H 'Accept: application/octet-stream' "repos/$repo/releases/assets/$asset" > "$archive"
printf '%s  %s\n' "$checksum" "$archive" | shasum -a 256 -c -
case "$tool" in
  mold) tar --strip-components=1 -xzf "$archive" -C "$destination" ;;
  protoc) unzip -q "$archive" -d "$destination" ;;
esac
"$destination/bin/$tool" --version
printf '%s\n' "$destination/bin" >> "${GITHUB_PATH:?}"
