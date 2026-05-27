#!/usr/bin/env sh
set -eu

REPO_URL="${HIVEMIND_REPO_URL:-https://github.com/nootr/hivemind}"
DEFAULT_REPO_URL="https://github.com/nootr/hivemind"
BRANCH="${HIVEMIND_BRANCH:-}"
TAG="${HIVEMIND_TAG:-}"
REV="${HIVEMIND_REV:-}"
FORCE_SOURCE="${HIVEMIND_FORCE_SOURCE:-}"
SKIP_CHECKSUM="${HIVEMIND_SKIP_CHECKSUM:-}"

fail() {
  echo "Error: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1
}

ref_count=0
[ -n "$BRANCH" ] && ref_count=$((ref_count + 1))
[ -n "$TAG" ] && ref_count=$((ref_count + 1))
[ -n "$REV" ] && ref_count=$((ref_count + 1))
if [ "$ref_count" -gt 1 ]; then
  fail "set only one of HIVEMIND_BRANCH, HIVEMIND_TAG or HIVEMIND_REV."
fi

if [ -n "${CARGO_HOME:-}" ]; then
  bin_dir="$CARGO_HOME/bin"
elif [ -n "${HOME:-}" ]; then
  bin_dir="$HOME/.cargo/bin"
else
  fail "could not determine install bin directory because HOME and CARGO_HOME are unset."
fi

# Make binaries available to the rest of this script even if the user's shell
# profile does not yet include Cargo's bin directory.
export PATH="$bin_dir:$PATH"

release_asset() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  arch="$(uname -m 2>/dev/null || echo unknown)"
  case "$os:$arch" in
    Linux:x86_64|Linux:amd64) echo "hivemind-x86_64-unknown-linux-gnu.tar.gz" ;;
    Darwin:x86_64) echo "hivemind-x86_64-apple-darwin.tar.gz" ;;
    Darwin:arm64|Darwin:aarch64) echo "hivemind-aarch64-apple-darwin.tar.gz" ;;
    *) return 1 ;;
  esac
}

release_url() {
  asset="$1"
  if [ -n "$TAG" ]; then
    echo "$REPO_URL/releases/download/$TAG/$asset"
  else
    echo "$REPO_URL/releases/latest/download/$asset"
  fi
}

install_release() {
  [ "$REPO_URL" = "$DEFAULT_REPO_URL" ] || return 1
  [ -z "$BRANCH" ] || return 1
  [ -z "$REV" ] || return 1
  [ -z "$FORCE_SOURCE" ] || return 1
  need curl || return 1
  need tar || return 1
  asset="$(release_asset)" || return 1
  url="$(release_url "$asset")"
  tmp="$(mktemp -d)"
  archive="$tmp/$asset"
  echo "Installing/updating HIVEMIND from release asset: $url"
  if ! curl -fL "$url" -o "$archive"; then
    rm -rf "$tmp"
    return 1
  fi
  if ! verify_release_checksum "$asset" "$archive" "$tmp"; then
    rm -rf "$tmp"
    fail "release checksum verification failed for $asset"
  fi
  if ! tar -xzf "$archive" -C "$tmp"; then
    rm -rf "$tmp"
    return 1
  fi
  mkdir -p "$bin_dir"
  install_binary "$tmp/hive" "$bin_dir/hive"
  install_binary "$tmp/hivemind-node" "$bin_dir/hivemind-node"
  rm -rf "$tmp"
  return 0
}

verify_release_checksum() {
  asset="$1"
  archive="$2"
  tmp="$3"
  [ -z "$SKIP_CHECKSUM" ] || return 0
  sums_url="$(release_url SHA256SUMS)"
  sums="$tmp/SHA256SUMS"
  echo "Verifying release checksum: $sums_url"
  if ! curl -fL "$sums_url" -o "$sums"; then
    return 1
  fi
  expected="$(awk -v asset="$asset" '$2 == asset || $2 == "./" asset || $2 ~ "/" asset "$" { print $1; exit }' "$sums")"
  [ -n "$expected" ] || return 1
  actual="$(sha256_file "$archive")" || return 1
  [ "$expected" = "$actual" ] || {
    echo "Checksum mismatch for $asset" >&2
    echo "expected: $expected" >&2
    echo "actual:   $actual" >&2
    return 1
  }
}

sha256_file() {
  file="$1"
  if need sha256sum; then
    sha256sum "$file" | awk '{print $1}'
  elif need shasum; then
    shasum -a 256 "$file" | awk '{print $1}'
  elif need openssl; then
    openssl dgst -sha256 "$file" | awk '{print $NF}'
  else
    echo "no SHA-256 tool found; set HIVEMIND_SKIP_CHECKSUM=1 to bypass checksum verification" >&2
    return 1
  fi
}

install_binary() {
  src="$1"
  dst="$2"
  tmp_dst="$dst.new.$$"
  cp "$src" "$tmp_dst"
  chmod +x "$tmp_dst"
  mv -f "$tmp_dst" "$dst"
}

install_from_source() {
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

  install_package() {
    package="$1"
    echo "Installing/updating $package from source: $REPO_URL"
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
}

if ! install_release; then
  echo "No compatible release binary found; falling back to source install." >&2
  install_from_source
fi

missing=""
if ! command -v hive >/dev/null 2>&1; then
  missing="$missing hive"
fi
if ! command -v hivemind-node >/dev/null 2>&1; then
  missing="$missing hivemind-node"
fi
if [ -n "$missing" ]; then
  fail "installed packages, but expected binaries were not found on PATH after adding $bin_dir:$missing"
fi

cat <<EOF

HIVEMIND installed.

If your shell cannot find hive later, add the install bin directory to your shell profile:
  export PATH="$bin_dir:\$PATH"

Next:
  hive node init
  hive node start
  hive node status
  hive setup
EOF
