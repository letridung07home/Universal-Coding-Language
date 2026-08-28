#!/usr/bin/env sh
# Verify that release metadata stays synchronized with Cargo package metadata.
# Major-version documentation and compatibility checks keep active contracts aligned.
# This script intentionally uses only POSIX shell tools available in CI.
set -eu

root_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root_dir"

fail() {
    printf '%s\n' "release metadata check failed: $*" >&2
    exit 1
}

package_version_from_lockfile() {
    awk '
        /^\[\[package\]\]$/ { in_ucl_package = 0 }
        /^name = "universal-coding-language"$/ { in_ucl_package = 1; next }
        in_ucl_package && /^version = "/ {
            line = $0
            sub(/^version = "/, "", line)
            sub(/"$/, "", line)
            print line
            exit
        }
    ' "$1"
}

version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || fail "could not read the package version from Cargo.toml"

major=${version%%.*}
root_lock_version=$(package_version_from_lockfile Cargo.lock)
[ "$root_lock_version" = "$version" ] || fail "Cargo.lock records $root_lock_version, expected $version"

fuzz_lock_version=$(package_version_from_lockfile fuzz/Cargo.lock)
[ "$fuzz_lock_version" = "$version" ] || fail "fuzz/Cargo.lock records $fuzz_lock_version, expected $version"

release_notes=$(awk -v heading="## $version" '
    index($0, heading) == 1 { found = 1; next }
    /^## / && found { exit }
    found { print }
' CHANGELOG.md)
[ -n "$release_notes" ] || fail "CHANGELOG.md has no entry for $version"

grep -F "UCL $version" README.md >/dev/null || fail "README.md does not identify UCL $version"

if [ "$major" = "1" ]; then
    if printf '%s\n' "$release_notes" | grep -E '^#{2,6}[[:space:]]+Breaking([[:space:]]|$)' >/dev/null; then
        fail "the $version changelog entry declares a breaking change"
    fi
    grep -F "version $version" docs/guarantees.md >/dev/null || fail "guarantees.md does not identify version $version"
    grep -F "Current version:** \`$version\`" docs/v1-release-plan.md >/dev/null || fail "v1 release plan is not updated to $version"
    grep -F "UCL $version" docs/roadmap.md >/dev/null || fail "roadmap.md does not record UCL $version"
elif [ "$major" = "2" ]; then
    printf '%s\n' "$release_notes" | grep -E '^#{2,6}[[:space:]]+Breaking([[:space:]]|$)' >/dev/null || fail "the $version changelog entry must declare breaking changes"
    grep -F "\`$version\`" docs/guarantees.md >/dev/null || fail "guarantees.md does not identify version $version"
    grep -F "Released version:** \`$version\`" docs/v2-goal.md >/dev/null || fail "v2 goal is not finalized for $version"
    grep -F "UCL $version" docs/roadmap.md >/dev/null || fail "roadmap.md does not record UCL $version"
fi

printf 'release metadata is consistent for UCL %s\n' "$version"
