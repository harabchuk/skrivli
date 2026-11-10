# Releasing

Releases are cut with [`cargo-release`](https://github.com/crate-ci/cargo-release) (configured in `[package.metadata.release]` in `Cargo.toml`) and built and published by GitHub Actions.

## Before you start

- Be on a clean, up-to-date `main`. `cargo-release` refuses to run from any other branch.
- Install the tool once: `cargo install cargo-release --locked --version 1.1.7`.

## Commands

```powershell
# Preview: shows what would change, does not modify anything
cargo release patch

# Do it: bump, commit, tag and push
cargo release patch --execute
```

Use `minor` or `major` instead of `patch` to bump a different part of the version.

## What `cargo release` does

1. Runs `cargo clippy --all-targets --locked -- -D warnings` as a pre-release hook. A warning stops the release.
2. Bumps `version` in `Cargo.toml` (and `Cargo.lock`).
3. Commits the bump as `Release vX.Y.Z`.
4. Creates the annotated tag `vX.Y.Z` with the message `Skrivli X.Y.Z`.
5. Pushes the commit and the tag. It does not publish to crates.io.

## What happens after the tag is pushed

Pushing a tag like `vX.Y.Z` starts the `release` workflow (`.github/workflows/release.yml`) on a Windows runner, which checks out the repository with submodules (needed to build parakeet.cpp):

1. **CI.** Runs the full CI workflow first (format, clippy, tests, release build). If it fails, nothing is published.
2. **Version check.** Fails if the tag does not match the `Cargo.toml` version.
3. **Build.** `cargo build --release --locked`, then checks that the exe's `ProductVersion` and `FileVersionRaw` equal the version.
4. **Licenses.** Generates `releases/licenses/THIRD-PARTY.txt` with `cargo about`.
5. **Package.** `releases/package.ps1` builds `target/dist/skrivli-X.Y.Z-windows-x64.zip` and its `.sha256` file. The ZIP holds `skrivli.exe`, `README.txt`, `PRIVACY.txt`, the licenses (parakeet.cpp MIT, Parakeet CC BY 4.0, Nemotron OpenMDW, sonora BSD) and the generated `THIRD-PARTY.txt` notices. It must not contain `.bin`, `.gguf` or `.pdb` files. If `SKRIVLI_SIGN_CMD` is set, the exe is signed first (not used yet).
6. **Release notes.** Takes `releases/release-notes-header.md` (install steps, SmartScreen note, SHA-256) and adds the commit subjects since the previous tag, with merge commits left out.
7. **Publish.** Creates the GitHub release `Skrivli X.Y.Z` immediately (not a draft), marked as latest, with the ZIP and checksum attached. If the release already exists, the assets are replaced instead.
8. **Artifact.** Also uploads the ZIP and checksum as a workflow artifact, kept for 7 days.

To test packaging locally, run `cargo build --release` and then `./releases/package.ps1 -Version X.Y.Z -AllowMissingThirdParty`. The result contains a placeholder third-party file and must not be released.

