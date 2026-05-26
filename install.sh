#!/usr/bin/env sh
set -eu

REPO_URL="${HIVEMIND_REPO_URL:-https://github.com/nootr/hivemind}"
BRANCH="${HIVEMIND_BRANCH:-}"
TAG="${HIVEMIND_TAG:-}"
REV="${HIVEMIND_REV:-}"

fail() {
  echo "Error: $*" >&2
  exit 1
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    return 1
  fi
}

if ! need git; then
  fail "git is required to install HIVEMIND from source."
fi

if ! need cargo; then
  cat >&2 <<'EOF'
Error: Rust/Cargo is required to install HIVEMIND from source.
Install Rust with rustup first:
  https://rustup.rs/
EOF
  exit 1
fi

ref_count=0
[ -n "$BRANCH" ] && ref_count=$((ref_count + 1))
[ -n "$TAG" ] && ref_count=$((ref_count + 1))
[ -n "$REV" ] && ref_count=$((ref_count + 1))
if [ "$ref_count" -gt 1 ]; then
  fail "set only one of HIVEMIND_BRANCH, HIVEMIND_TAG or HIVEMIND_REV."
fi

if [ -n "${CARGO_HOME:-}" ]; then
  cargo_bin="$CARGO_HOME/bin"
elif [ -n "${HOME:-}" ]; then
  cargo_bin="$HOME/.cargo/bin"
else
  fail "could not determine Cargo bin directory because HOME and CARGO_HOME are unset."
fi

# Make binaries available to the rest of this script even if the user's shell
# profile does not yet include Cargo's bin directory.
export PATH="$cargo_bin:$PATH"

install_package() {
  package="$1"
  echo "Installing/updating $package from $REPO_URL"
  if [ -n "$BRANCH" ]; then
    cargo install --git "$REPO_URL" --branch "$BRANCH" "$package" --locked --force
  elif [ -n "$TAG" ]; then
    cargo install --git "$REPO_URL" --tag "$TAG" "$package" --locked --force
  elif [ -n "$REV" ]; then
    cargo install --git "$REPO_URL" --rev "$REV" "$package" --locked --force
  else
    cargo install --git "$REPO_URL" "$package" --locked --force
  fi
}

install_package hivemind-cli
install_package hivemind-node

missing=""
if ! command -v hive >/dev/null 2>&1; then
  missing="$missing hive"
fi
if ! command -v hivemind-node >/dev/null 2>&1; then
  missing="$missing hivemind-node"
fi
if [ -n "$missing" ]; then
  fail "installed packages, but expected binaries were not found on PATH after adding $cargo_bin:$missing"
fi

cat <<EOF

HIVEMIND installed.

If your shell cannot find hive later, add Cargo's bin directory to your shell profile:
  export PATH="$cargo_bin:\$PATH"

Next:
  hive node init
  hive node start
  hive node status
  hive setup
EOF
