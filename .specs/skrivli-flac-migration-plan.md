# Skrivli — WAV → FLAC migration plan

**Status: complete (2026-11-08).** FLAC becomes the only format Skrivli saves, plays and transcribes. No backwards
compatibility: existing `.wav` files are no longer listed, played or transcribed.

## Decisions

- Library: `flac-codec` (crates.io, 1.3.x) for encode, decode and seek.
- Output: 16-bit stereo 48 kHz FLAC, compression level 5 (default).
- Files: `Skrivli_YYYY-MM-DD_HH-MM-SS[-N].flac`, written to `.flac.partial`, renamed on finalize. Unknown partials are never auto-deleted.
- Size limit: the 4 GB RIFF safe stop is replaced by a **12 hour duration cap**. Reaching it finalizes the file and shows "Recording stopped because it reached the 12 hour limit."
- Playback: flac-codec decoder wrapped as a rodio `Source` (with seek). `hound` and rodio's `hound` feature are removed.
- `RecordingContainer::M4a` and `Wav` are removed; FLAC is the sole container.

## Phase 1 — Core FLAC I/O [done]

- Add `flac-codec`; replace `infrastructure/files/wav.rs` with `flac.rs` (`FlacWriter`): create/partial/collision naming, periodic flush, finalize (STREAMINFO written), abandon, Drop, 12 h cap (`FlacWrite::SafeStop`).
- `domain/recording.rs`: container is FLAC only; domain constant for the 12 h cap.
- `transcript_reader.rs`: decode finalized `.flac` in bounded chunks via flac-codec; refuse `.partial` and non-FLAC.
- `player.rs`: flac-codec–backed rodio `Source` with seeking.
- `storage/library.rs`, `storage/sidecar.rs`: `.flac` suffixes only.
- `services/recording.rs` and pipeline wiring use the new writer.
- Remove `hound` and rodio `hound` feature. Tests first; fmt/clippy/test green.

**Evidence:** commit `7b97179`. `flac-codec` 1.3.3 locked; `files/flac.rs` (`FlacWriter`) and `FlacSource` in `player.rs` replace `wav.rs`/hound; unit and integration tests run without hardware.

## Phase 2 — UI, errors, integration tests [done]

- Views: detail line shows `FLAC`; remove the "Only WAV playback" disabled state.
- `common/error.rs`: replace "wav limit" message with the duration cap message.
- `tests/` integration flows and remaining `.wav` fixtures/strings switch to FLAC.

**Evidence:** commit `81c1c1d`. Detail line shows `FLAC`, player is always enabled, cap message reads "Recording stopped because it reached the 12 hour limit.", integration tests use `.flac`.

## Phase 3 — Specs [done]

- Update `skrivli-product.md`, `skrivli-app-design.md`, `decisions.md`, `skrivli-implementation-plan.md` (and other plans mentioning WAV) to describe FLAC-only behavior and the 12 h cap.

**Evidence:** product spec (format, detail line, 12 h cap, old WAV files not shown), app design (6.5 audio files, stack table, layout), `decisions.md` D-29 (flac-codec 1.3.3, 80,000-byte padding, 12 h cap), implementation and other plans annotated, README updated. `grep -rniE "wav|m4a|hound|riff"` on `.specs` now finds only historical text, marked as superseded.
