# Skrivli — Release pipeline plan

**Status:** Proposed (not implemented).
**Goal:** One command, `cargo release <major|minor|patch|X.Y.Z> --execute`, turns a clean `main` into a published GitHub release that holds the portable Windows ZIP described in M9.2 of `skrivli-implementation-plan.md`.
**Related:** `.github/workflows/ci.yml` (existing checks), `build.rs` (VERSIONINFO), `src/views/settings.rs` (shows `CARGO_PKG_VERSION`), `.specs/decisions.md` (record outcomes there).

---

## 1. Decisions taken for this plan

These came from the planning conversation on 2026-11-08. Change them here first if they change.

| Topic | Decision |
| --- | --- |
| Who drives the release | The developer runs [`cargo-release`](https://github.com/crate-ci/cargo-release) on `main`. It checks the repository, bumps the version, commits, tags and pushes. There is no hand-written release script. |
| Who builds and publishes | GitHub Actions. Pushing a `vX.Y.Z` tag starts `release.yml`. It runs the CI checks, builds on a clean runner, packages the ZIP and creates the GitHub release. Nothing is built for release on the developer machine. |
| Publish mode | Publish right away. No draft step. |
| Code signing | None for now. The exe is unsigned, and the release notes say how SmartScreen will react. Section 9 keeps space to add signing later. |
| Version scheme | SemVer `MAJOR.MINOR.PATCH` in `Cargo.toml`. Tag is `vMAJOR.MINOR.PATCH`. While the app is pre-1.0, `minor` is for features and `patch` is for fixes. |
| Artifact | `skrivli-<version>-windows-x64.zip` plus `skrivli-<version>-windows-x64.zip.sha256`. |
| Release notes | Commit subjects since the previous tag, under a fixed header. No hand-written `CHANGELOG.md` for now. |

---

## 2. What a release looks like when it is done

- On the `main` branch, `Cargo.toml` and `Cargo.lock` are at the new version, and commit `Release vX.Y.Z` is the newest commit.
- Annotated tag `vX.Y.Z` points at that commit.
- The `release` workflow run for that tag is green.
- `https://github.com/harabchuk/skrivli/releases/tag/vX.Y.Z` is published as the latest release and has:
  - the ZIP and its SHA-256 file,
  - release notes: the fixed header (install, SmartScreen, models, SHA-256) and the list of commits since the last tag.
- Settings → footer in the unzipped app shows `Version X.Y.Z`. Explorer → Properties → Details on `skrivli.exe` shows File version `X.Y.Z.0` and Product version `X.Y.Z`.
- The ZIP has no `ggml-*.bin` model file (D-08).

---

## 3. Prerequisites (one-time setup)

On the developer machine:

1. The Rust toolchain from `rust-toolchain.toml` and `git`, signed in with push rights to `harabchuk/skrivli`.
2. `cargo install cargo-release --locked --version <pinned>`. Pin the version in `decisions.md` (1.1.7 is current on 2026-11-08).
3. Optional: PowerShell 7 (`pwsh`), to run `scripts/package.ps1` locally when testing packaging. The release itself does not need it on the developer machine.

In the repository:

4. Branch protection on `main` must let the maintainer push the release commit. If it does not, see open question Q1.

The `release` workflow asks for `contents: write` itself, so no repository-wide token setting is needed.

---

## 4. Code changes needed before the first release

### 4.1 One source of truth for the version

`build.rs` hardcodes `"0.1.0.0"` today. `winresource` already fills in the version from Cargo when the crate does not set it:

- the `FileVersion` and `ProductVersion` strings become `CARGO_PKG_VERSION` (for example `0.1.1`),
- the fixed `VS_FIXEDFILEINFO` version becomes `MAJOR.MINOR.PATCH.0`.

So the change is to **delete** the `FileVersion` and `ProductVersion` lines from `build.rs`. Then `Cargo.toml` is the only file a bump edits, and `cargo-release` updates `Cargo.lock`.

Check: an integration test in `tests/` cannot easily read the PE resource, so `release.yml` checks it (section 6): `VersionInfo.ProductVersion` equals `X.Y.Z` and `VersionInfo.FileVersionRaw` equals `X.Y.Z.0`.

### 4.2 Packaging script

Add `scripts/package.ps1 -Version X.Y.Z`. It takes an existing `target\release\skrivli.exe` and:

1. runs the signing hook (section 9),
2. puts the layout from 4.3 together under `target\dist\skrivli-X.Y.Z-windows-x64\`,
3. zips it to `target\dist\skrivli-X.Y.Z-windows-x64.zip`,
4. writes `<zip>.sha256` in the `sha256sum` format `<hash>  <filename>`,
5. opens the ZIP and fails if any expected file is missing or if it holds any `*.bin` or `*.pdb`.

`release.yml` calls it. A developer can run it locally to try out packaging changes.

### 4.3 Files that go in the ZIP

```text
skrivli-X.Y.Z-windows-x64/
├── skrivli.exe
├── README.txt                 # short user readme: unzip, run, where data lives, models
├── PRIVACY.txt                # matches the privacy promise in skrivli-product.md
├── LICENSE-MIT.txt
├── LICENSE-APACHE.txt
└── licenses/
    ├── sonora-BSD-3-Clause.txt
    ├── whisper.cpp-MIT.txt    # whisper.cpp + ggml notice (decisions.md: Licenses / notices)
    └── THIRD-PARTY.txt        # generated, see below
```

- `THIRD-PARTY.txt` is generated with `cargo about` from `Cargo.lock`, in `release.yml` before packaging. The tool and its version get pinned in `decisions.md`.
- Runtime DLLs: link the MSVC CRT statically, so the ZIP needs no VC++ redistributable. Cargo cannot set `rustflags` for only one profile on stable, so set it for the target in `.cargo/config.toml`, which applies to debug and release builds alike:

  ```toml
  [target.x86_64-pc-windows-msvc]
  rustflags = ["-C", "target-feature=+crt-static"]
  ```

  A clean-VM dependency check confirms this (M9.2). If static CRT breaks whisper.cpp or GPUI, fall back to shipping the app-local CRT DLLs and write that down in `decisions.md`.
- Repository files that are missing today (`LICENSE-MIT`, `LICENSE-APACHE`, the whisper.cpp licence text, user README/PRIVACY text) get added as part of this work.

---

## 5. Cutting a release with `cargo-release`

### 5.1 Configuration

`release.toml` in the repository root:

```toml
allow-branch = ["main"]
publish = false                       # not a crates.io crate
tag-name = "v{{version}}"
tag-message = "Skrivli {{version}}"
pre-release-commit-message = "Release v{{version}}"
pre-release-hook = ["cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings"]
```

`push = true` and `push-remote = "origin"` are the defaults.

### 5.2 Usage

```text
cargo release patch                # dry run: shows what 0.1.0 -> 0.1.1 would do
cargo release patch --execute      # do it
cargo release minor --execute      # 0.1.0 -> 0.2.0
cargo release major --execute      # 0.1.0 -> 1.0.0
cargo release 0.3.0 --execute      # explicit version
```

Without `--execute` (`-x`), `cargo-release` is a dry run. It prints each step and changes nothing. With `--execute` it asks for confirmation before it changes anything.

### 5.3 What it does

1. **Preflight.** These change nothing:
   - the working tree is clean,
   - the branch is `main`,
   - `git fetch origin`, then local `main` must not be behind `origin/main`,
   - tag `vX.Y.Z` does not exist yet,
   - the new version is greater than the current one.
2. **Bump.** Writes the new `[package] version` to `Cargo.toml` and updates `Cargo.lock`.
3. **Hook.** Runs clippy. Clippy compiles the tests too, so this catches build and lint errors before anything is pushed. Tests and the release build run in CI (section 6).
4. **Commit and tag.** Commit `Release vX.Y.Z` and annotated tag `vX.Y.Z`.
5. **Push.** Pushes `main` and the tag to `origin`. The tag push starts `release.yml`.

The developer then watches the `release` run in the Actions tab (`gh run watch` works too) and checks that the release page appears.

---

## 6. Workflows

### 6.1 `ci.yml` changes

- Add `--locked` to `cargo test` and `cargo build --release`, so a stale `Cargo.lock` fails CI.
- Add a `workflow_call` trigger, so `release.yml` can run the same checks without copying them.
- Restrict `push` to branches (`push: branches: ['**']`). Today a tag push would run `ci.yml` a second time, next to `release.yml`.

### 6.2 New workflow: `release.yml`

**Trigger:** `push` of tags matching `v[0-9]+.[0-9]+.[0-9]+`.
**Permissions:** `contents: write`.
**Runner:** `windows-latest`, with the toolchain from `rust-toolchain.toml`.

```yaml
jobs:
  ci:
    uses: ./.github/workflows/ci.yml        # fmt, clippy, test, release build
  release:
    needs: ci
    runs-on: windows-latest
    steps: [...]                            # below
```

`release` job steps:

1. Check out the tag with full history (`fetch-depth: 0`, for the notes).
2. Fail unless the tag equals `v` + `[package] version` from `Cargo.toml`.
3. `cargo build --release --locked`. Then check the exe: `VersionInfo.ProductVersion` is `X.Y.Z` and `VersionInfo.FileVersionRaw` is `X.Y.Z.0`.
4. Generate `licenses/THIRD-PARTY.txt` with `cargo about`.
5. `scripts/package.ps1 -Version X.Y.Z`.
6. Build the notes file: `.github/release-notes-header.md` with the ZIP's SHA-256 filled in, then the commits since the previous tag. Find the previous tag with `git describe --tags --abbrev=0 vX.Y.Z^`, and list the commits with `git log --no-merges --pretty="- %s" <prev>..vX.Y.Z`. For the first release, with no previous tag, the list is replaced by "First public release."
7. `gh release create vX.Y.Z <zip> <zip.sha256> --title "Skrivli X.Y.Z" --notes-file <file> --latest --verify-tag`. If the release already exists because this is a re-run, run `gh release upload vX.Y.Z <zip> <zip.sha256> --clobber` instead. Then re-running the job is always safe.
8. Upload the ZIP as a workflow artifact too (kept 7 days).

The header in `.github/release-notes-header.md` covers:

- how to install (unzip anywhere, run `skrivli.exe`),
- SmartScreen: "Windows may say it protected your PC. Choose More info → Run anyway. The app is not code-signed yet.",
- models are a separate download inside the app,
- the SHA-256 of the ZIP.

---

## 7. When something goes wrong

| Where it failed | State | What to do |
| --- | --- | --- |
| Preflight | Nothing changed. | Fix the cause (dirty tree, wrong branch, behind origin) and run again. |
| Clippy hook | `Cargo.toml`/`Cargo.lock` are bumped in the working tree. Nothing is committed. | `git checkout -- Cargo.toml Cargo.lock`, fix, run again. |
| Push rejected | Local commit and tag exist, nothing on origin. | `git tag -d vX.Y.Z`, `git reset --hard HEAD~1`, `git pull`, run again. |
| `ci` job in `release.yml` red | Commit and tag are on origin. No release. | Fix on `main` with a normal commit, then release the **next patch** version. Do not move or reuse a tag that has been pushed. |
| `release` job failed for an infrastructure reason (network, runner, `gh`) | Tag is on origin. There may be a partial release. | Re-run the failed job from the Actions tab. Step 7 makes re-runs safe. |
| Release is out but broken | Public release. | Mark it pre-release with `gh release edit vX.Y.Z --prerelease`, add a note, and ship a patch release. Do not delete published tags. |

---

## 8. Signing (later, not in this plan's scope)

`package.ps1` has a hook that runs before zipping: `if ($env:SKRIVLI_SIGN_CMD) { & $env:SKRIVLI_SIGN_CMD target\release\skrivli.exe }`. When signing is picked (Azure Trusted Signing or an OV/EV certificate), record the choice in `decisions.md`, set the command and its secrets in `release.yml`, and remove the SmartScreen line from the notes header.

---

## 9. Out of scope for now

- An installer (MSI/MSIX/winget). The product spec promises a portable ZIP only.
- Auto-update inside the app.
- Reproducible, byte-identical builds.
- ARM64 builds.
- A generated `CHANGELOG.md` (for example with `git-cliff`). Add it only if the commit list stops being enough.

---

## 10. Work packages

Do them in this order. Each is one reviewable change. Write the failing check first, as `AGENTS.md` asks.

- [ ] **R1 — Version source.** Remove the hardcoded versions from `build.rs`. Check: after `cargo build --release`, the exe's `ProductVersion` equals the `Cargo.toml` version and `FileVersionRaw` equals that version plus `.0`.
- [ ] **R2 — Licence and user files.** Add `LICENSE-MIT`, `LICENSE-APACHE`, the whisper.cpp licence, `README.txt` and `PRIVACY.txt` text for the ZIP. Decide on static CRT (`.cargo/config.toml`) and record it in `decisions.md`.
- [ ] **R3 — `scripts/package.ps1`.** Builds the ZIP and the SHA-256 and checks the contents. Check: the contents list matches 4.3 and has no `*.bin`/`*.pdb`. A missing file makes the script fail.
- [ ] **R4 — `release.toml`.** Pin the `cargo-release` version in `decisions.md`. Check: `cargo release patch` (dry run) on a clean `main` exits 0, shows commit `Release v0.1.1`, tag `v0.1.1` and a push to `origin`, and leaves `git status` clean.
- [ ] **R5 — `ci.yml` changes.** `--locked`, `workflow_call`, push restricted to branches.
- [ ] **R6 — `release.yml` and the notes header.** Check: on a fork, run `cargo release patch --execute`. A release appears with the ZIP, the `.sha256` and the notes. Run the job again and it still passes. Then delete the fork's release and tag.
- [ ] **R7 — First real release.** Run the M9.2 clean-VM checks against the published ZIP. Write the dependency list, SmartScreen/Defender behaviour and timings into `decisions.md`. Tick the matching M9.2 boxes in `skrivli-implementation-plan.md`.

Spec updates that come with this work:

- `skrivli-product.md`: where to download releases, and the version shown in Settings.
- `skrivli-app-design.md`: a short Build and release section.
- `skrivli-implementation-plan.md`: M9.2 points to this plan.
- `decisions.md`: static CRT, `cargo-release` and `cargo about` versions, signing deferred, release built in CI.

---

## 11. Open questions

- **Q1 — Protected `main`.** If `main` needs pull requests, `cargo-release` cannot push the release commit. Options:
  - let the maintainer bypass the rule,
  - or do the bump by hand: `cargo release version patch --execute` and `cargo release commit --execute` on a `release/vX.Y.Z` branch, open a PR, and after the merge tag the merge commit with `cargo release tag --execute` and `cargo release push --execute`.

  The plan assumes a direct push.
- **Q2 — First version number.** Is the first public release `0.1.0` or `0.1.1`?
  - `0.1.1` is `cargo release patch --execute`.
  - `0.1.0` needs no bump. Tag the current commit by hand with `git tag -a v0.1.0 -m "Skrivli 0.1.0"` and `git push origin v0.1.0`. `release.yml` runs the same way.
