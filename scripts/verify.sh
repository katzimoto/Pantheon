#!/bin/sh
# Pantheon's canonical verification command.
#
# This is the one command a human or an agent runs to prove a change is sound,
# and it is exactly what CI runs on Linux. Keeping local and CI verification the
# same command is deliberate: a repository where CI checks more than the
# developer can run locally teaches people to push and wait.
#
# It requires the pinned toolchain in `rust-toolchain.toml` and ordinary OS build
# prerequisites, and nothing else. No task runner, no test runner, no auditing
# tool and no language runtime has to be installed first. New checks may be added
# here as implementation lands, but a check that needs a separately installed
# tool needs to justify raising the cost of contributing.
#
# Ordering is cheapest-and-most-specific first, so the common failures report in
# a second or two rather than after a full build.
#
# Usage: scripts/verify.sh    (run from anywhere in the repository)

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# Structural checks. All POSIX shell, and together they finish in about a
# second. The action pin check needs no toolchain at all, so it runs first.
./scripts/check-action-pins.sh
./scripts/check-docs-links.sh
./scripts/check-crate-deps.sh
./scripts/check-skill-symlinks.sh
./scripts/check-hooks.sh

cargo fmt --all -- --check

# `--all-targets` covers tests, benches and examples, not just the library.
#
# Deliberately not `--all-features`. Feature-gated code is built to be exercised
# in a specific combination, and a union of every feature is a configuration
# nobody ships: it hides conflicts and reports failures in code paths that no
# real build produces. When Pantheon has features worth testing — failpoints,
# platform backends — each gets an explicit CI job naming its combination.
cargo clippy --workspace --all-targets --locked -- -D warnings

cargo test --workspace --locked

# Documentation is a contract here: every crate root states what it owns and what
# it must not own, and a broken link or malformed doc comment in that statement is
# a defect like any other.
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

# Record success as a local, transient fingerprint (.git/pantheon/), never
# committed repository authority. This is what lets a local Stop hook detect
# that the working tree changed since the last successful run; see
# docs/development/agent-skills-and-hooks.md. Not a second verification
# command — it only remembers this run's own result.
./scripts/hooks/record-verified.sh "$root"

echo "verify: OK"
