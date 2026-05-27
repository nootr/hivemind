# Releases

GitHub Actions builds prebuilt HIVEMIND binaries for tagged releases.

## Targets

Release assets are named:

- `hivemind-x86_64-unknown-linux-gnu.tar.gz`
- `hivemind-x86_64-apple-darwin.tar.gz`
- `hivemind-aarch64-apple-darwin.tar.gz`
- `hivemind-x86_64-pc-windows-msvc.zip`

Each archive contains:

- `hive` / `hive.exe`
- `hivemind-node` / `hivemind-node.exe`
- `README.md`

Tagged releases also include `SHA256SUMS`.

## Publishing

Push a tag to publish a GitHub release with assets:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The release workflow can also be run manually to build artifacts without publishing a release.

## Installer behavior

`install.sh` uses prebuilt release binaries by default on supported Unix platforms and verifies the downloaded archive against `SHA256SUMS`. Windows users should download the `.zip` release asset manually and place `hive.exe` and `hivemind-node.exe` on `PATH`.

The installer falls back to `cargo install --git` when:

- no compatible release asset exists;
- `HIVEMIND_BRANCH` or `HIVEMIND_REV` is set;
- `HIVEMIND_REPO_URL` points at a custom repository;
- `HIVEMIND_FORCE_SOURCE=1` is set.

Set `HIVEMIND_SKIP_CHECKSUM=1` only for local debugging or unreleased test artifacts.

`HIVEMIND_TAG=v0.1.0` installs from that release tag when a matching asset exists, otherwise it falls back to source install from the same tag.

`hive update` uses the installer by default, so normal updates also prefer release binaries.
