# Skrivli: remove rescue and crash recovery

**Status:** Not started.

**Goal in one sentence:** take out every path that saves, renames, lists or labels a recording after something went wrong (the rescue copy, startup recovery and the "Recovered" label), so a recording is either finalized as a normal `.wav` or not saved at all.

**Branch base:** `d2f2d62`. Line references are to that commit and will drift.

**No backward compatibility.** Files left behind by the old code get no special handling:

| Old artifact | What happens after this change |
| --- | --- |
| `*.wav.partial` in the recordings folder | Ignored. Not listed, not repaired, not renamed, not deleted. |
| `*.recovered.wav` in the recordings folder | Listed as an ordinary WAV. The name includes `.recovered` (for example `Skrivli_2026-11-04_13-01-02.recovered`), with no badge and no "Recovered" note. |
| `%LOCALAPPDATA%\Skrivli\rescue\` and its `*.rescued.wav` files | Ignored. The app no longer creates, reads or deletes this folder. |

No migration, cleanup pass or one-time notice is added for any of these.

---

## 1. What "rescue" means today

Three features are tied together. All three go.

1. **Rescue copy (during a session).** When the recordings folder fails mid-recording, `services/recording.rs::preserve_or_rescue` releases the `.wav.partial` and `infrastructure/files/rescue.rs::rescue_partial` copies it to `%LOCALAPPDATA%\Skrivli\rescue\<stem>.rescued.wav`. The user sees "A copy was saved to the rescue folder." or "A partial file was kept."
2. **Startup recovery (next launch).** `app/bootstrap.rs::recovery_notice` calls `infrastructure/files/wav.rs::recover_partials`, which repairs any `*.wav.partial` with a valid header and renames it to `*.recovered.wav`. The user sees "A previous recording was recovered." or "…could not be recovered."
3. **Recovered presentation (library).** `storage/library.rs` flags `*.recovered.wav` as `recovered`, `domain::recording::RecordingInfo` carries the flag, and `views/recordings.rs` shows a yellow **Recovered** badge and the note "Recovered after an interruption. The end of this recording may be missing."

### What stays

These look related but are not rescue, so they are kept unchanged:

- **`.wav.partial` staging.** The writer still writes into `Name.wav.partial` and renames to `Name.wav` on finalize. This is what keeps an in-progress capture out of the library and out of the transcription reader.
- **Periodic header repair** (`WavWriter::repair`, about every 2 s). It keeps the lengths right for finalize after a failure.
- **`WavWriter::abandon`.** Deleting the partial after a failed *start* is unrelated to rescue.
- **The transcription reader refusing `.wav.partial`** (`files/transcript_reader.rs`).
- **Device-loss handling, low-disk stop, worker panic and stall notices** (the rest of D-10).
- **The rule "Skrivli does not silently delete partial recordings."** It still holds: we stop touching them at all.

---

## 2. Behavior after the change

What happens when a recording ends because of a failure (`finish_session` with `failed` set, or the folder check failing at stop):

```text
failure during session
        │
        ▼
 note "save_error" in problems
        │
        ▼
 try wav.finalize()  ── ok ──▶ write sidecar (best effort) ──▶ show the failure notice
        │                                                     (recording appears in the list
        │                                                      with the "Problems occurred" note)
      error
        │
        ▼
 log the error, leave the .wav.partial where it is ──▶ show the failure notice
```

Decisions behind this:

- **Always try finalize, even when the folder check failed.** The writability probe can fail while the rename still works. If the rename succeeds the user gets a normal, playable recording. Today the folder-failed path never tries.
- **The notice is the original failure** (`NOTICE_SAVE`, `NOTICE_FOLDER`, `NOTICE_LOW_DISK`, `NOTICE_WORKER`…). No new notice text is added.
- **A partial that cannot be finalized is left on disk and not mentioned.** The app has no way to show it any more, so a notice pointing at it would only confuse.
- **After a crash or kill**, the `.wav.partial` stays where it is and the app says nothing on the next launch.

Removed user-facing strings: `NOTICE_RECOVERED`, `NOTICE_RECOVER_FAILED`, `NOTICE_RESCUED`, `NOTICE_PARTIAL_KEPT`, `NOTE_RECOVERED`, and the **Recovered** badge label.

---

## 3. Phases

Each phase leaves the tree building with `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` clean. Follow red, green, refactor from `AGENTS.md`: where a phase changes behavior, write or change the test first and watch it fail.

### Phase 1: Session failure path stops rescuing

**Why first:** this is the only phase that changes what happens during a recording. Once it is done, nothing calls `rescue.rs`, and the later phases are pure deletion.

**Red**

- [x] In `tests/recording_flow.rs`, rename `folder_fault_rescues_readable_partial` to `folder_fault_finalizes_in_place_and_reports_folder`. Assert:
  - the notice contains "The recordings folder is not writable",
  - exactly one finalized `.wav` exists in the recordings folder,
  - no file name contains `partial` or `rescued`.
  
  Run it. It fails today because the code returns `NOTICE_RESCUED` and never finalizes.
- [x] In `mid_session_write_fault_preserves_usable_output`, replace the `has_artifact` check (which accepts partials and rescue copies) with: one finalized `.wav` exists, and no `partial` or `rescued` file exists. This should already pass, because the write-fault path finalizes when the folder is fine. Keep it as a guard.
- [x] In `mid_session_low_disk_stops_with_warning`, drop the "or a partial exists" fallback so the test needs a finalized `.wav`.

**Green**

- [x] `services/recording.rs`: replace `preserve_or_rescue` and `rescue_existing_partial` with one function, `keep_after_failure`:
  - note `save_error`,
  - call `wav.finalize()`. On success, write the sidecar (ignore its error, as today). On failure, log it with `tracing::error!`,
  - return `Err(failure)` either way.
  
  Remove the `folder_ok` and `_directory` parameters it no longer needs.
- [x] `finish_session`: call `keep_after_failure` in both failure branches. When `failed` is `None` and the folder check fails, pass `NOTICE_FOLDER`.
- [x] Remove `use crate::infrastructure::files::rescue::rescue_partial;`.
- [x] Remove the constants `NOTICE_RESCUED` and `NOTICE_PARTIAL_KEPT`.

**Refactor**

- [x] `infrastructure/files/wav.rs`: delete `WavWriter::release_partial`. It is now unused, because a failed `finalize` already leaves the partial on disk through `Drop`. Its only test caller is the recovery test, which goes in Phase 2.
- [x] Update the module doc on `wav.rs` (lines 8–11): keep "never auto-delete unknown `*.wav.partial` files" and the `abandon` sentence. Remove the "Crash recovery renames…" sentence.

**Done when:** a folder fault produces a normal `.wav` plus the folder notice, and `rescue_partial` has no callers.

---

### Phase 2: Remove the rescue folder and the rescue module

**Why:** with no callers left, the rescue copy, its directory and its plumbing are dead code.

- [x] Delete `src/infrastructure/files/rescue.rs`. Remove `pub(crate) mod rescue;` from `infrastructure/files/mod.rs`.
- [x] `services/recording.rs`: remove the `rescue_dir: PathBuf` field from `RecordingEngine` and its doc comment. Remove `rescue_dir: dir.join("rescue")` from the three test engines (about lines 1162, 1256 and 1475).
- [x] `services/recording_harness.rs`: remove the `rescue_dir` field, its `create_dir_all`, the engine field it fills, and the public `RecordingHarness::rescue_dir()` accessor. This is a public `lib` API, which is fine because only `tests/` uses it.
- [x] `tests/recording_flow.rs`: remove any remaining `harness.rescue_dir()` use. After Phase 1 there should be none.
- [x] `app/bootstrap.rs`:
  - remove the `rescue_dir` argument from `spawn_workers` and the `paths.rescue_dir()` argument at its call site,
  - change `prepare_local_dirs` to create only `paths.models_dir()`.
- [x] `common/paths.rs`: delete `AppPaths::rescue_dir()`, and the `rescue` line and its assert in `layouts_folders_from_injected_bases`.
- [x] `common/error.rs`: change `"save recording" | "recover recording" | "rescue recording"` to `"save recording"`.

**Check:** `rg -i rescue src tests` returns nothing.

---

### Phase 3: Remove startup recovery

**Why:** after this phase the app no longer reads or renames `*.wav.partial` on launch, and the launch notice disappears.

- [x] `infrastructure/files/wav.rs`: delete `RecoveryReport`, `recover_partials`, `recover_one` and `recovered_path` (about lines 293–420). Delete the test `recoverable_partial_is_renamed_and_a_bad_header_is_left` and remove `recover_partials` from the test `use` list. If `File`, `Read` or `Seek` imports become unused outside `#[cfg(test)] read_wav_lengths`, move them under `cfg(test)` or drop them as clippy reports.
- [x] `app/bootstrap.rs`:
  - delete `fn recovery_notice` and the `use …::wav::recover_partials` import,
  - remove `NOTICE_RECOVERED` and `NOTICE_RECOVER_FAILED` from the `services::recording` import,
  - remove `let recovery_notice = …`, the `recovery_notice` parameter of `open_shell`, and `presentation.set_notice(recovery_notice);`. `Presentation::set_notice` stays: other callers use it.
- [x] `services/recording.rs`: delete `NOTICE_RECOVERED` and `NOTICE_RECOVER_FAILED`.
- [x] `views/recording.rs`:
  - **Red:** change `good_news_is_not_shown_as_a_warning` to check only `NOTICE_SAVED` (Info) against a failure (Warning). It fails to compile until the next step, which is acceptable here because the change is a pure removal.
  - **Green:** `notice_tone` becomes `if notice == NOTICE_SAVED { Info } else { Warning }`. Drop `NOTICE_RECOVERED` from the import.

**Check:** `rg "recover_partials|NOTICE_RECOVER" src` returns nothing. On launch with a leftover `*.wav.partial` in the folder, the file is untouched and no notice is shown.

---

### Phase 4: Remove "Recovered" from the library and the UI

**Why:** no new `*.recovered.wav` can appear, and old ones are treated as plain WAVs, so the flag, the badge and the note have nothing to show.

**Red**

- [x] `infrastructure/storage/library.rs`: rename `scan_skips_partials_keeps_recovered_and_unpaired` to `scan_skips_partials_and_lists_old_recovered_as_plain_wav`. Assert that `crash.recovered.wav` is listed with `name() == "crash.recovered"` (not `"crash"`), and that `busy.wav.partial` is still skipped. It fails today because `display_stem` strips `.recovered.wav`.
- [x] `views/recordings.rs`:
  - `badge_maps_status_and_recovered_flag` becomes `badge_maps_status` over `RowBadge::from_status(status)` for the five statuses. Drop the two `Recovered` cases.
  - `notes_cover_recovered_sidecar_and_problems` becomes `notes_cover_sidecar_and_problems`. Drop the recovered case and `NOTE_RECOVERED`.
  - Delete the row test that asserts `RowBadge::Recovered` (about line 1219), and the recovered half of the row-notes test (about lines 1420–1432).
  - Update the `info_at` test helper to drop its `recovered` argument.

**Green**

- [x] `infrastructure/storage/library.rs`:
  - remove the `let recovered = name.ends_with(".recovered.wav")` branch. Every file goes through `RecordingContainer::from_extension`,
  - remove the explicit `if name.ends_with(".wav.partial") { continue; }`. The extension `partial` already maps to `None` (see the `domain/recording.rs` test `from_extension("partial") == None`). Keep a one-line comment saying partials are skipped by extension,
  - remove the `recovered` parameter from `entry_from_sidecar` and `entry_without_sidecar`. If either drops to seven parameters or fewer, also remove its `#[allow(clippy::too_many_arguments)]`,
  - remove `.strip_suffix(".recovered.wav")` from `display_stem`.
- [x] `domain/recording.rs`: remove the `recovered` field, its constructor argument and `RecordingInfo::recovered()`. Fix every `RecordingInfo::new(…)` caller (library and test helpers). Re-check whether `#[allow(clippy::too_many_arguments)]` on `new` is still needed (it will still be, at ten arguments).
- [x] `views/recordings.rs`:
  - remove the private `recovered` field from `RecordingRow`,
  - `RowBadge::from_status(status)` takes one argument. Delete the `Recovered` variant, its doc comment and its arm in `row_badge`,
  - `set_row_status` calls `RowBadge::from_status(status)`,
  - `recording_notes(sidecar, has_problems)` takes two arguments. Delete `NOTE_RECOVERED`.

**Check:** `rg -i "recovered" src tests` returns only unrelated hits: `ChunkFlags::RECOVERED` in `flex_capture.rs` (a flexaudio flag) and the `recovered` variable in `services/playback.rs` tests.

---

### Phase 5: Specs

**Why:** `AGENTS.md` requires the specs to describe the app the code ships. Make these edits in the same change as the code.

`.specs/skrivli-product.md`

- [ ] "Durability and recovery (user-visible)": rename it to "Durability (user-visible)". Delete the bullet "Interrupted recordings are recovered on next launch…". Add: "If a recording fails part way, Skrivli saves what it can as a normal recording and says what went wrong. A recording interrupted by a crash or power loss is not recovered."
- [ ] Keep "Skrivli does not silently delete usable partial recordings." and add: "Leftover unfinished files are not shown in the app."
- [ ] Library: change "In-progress captures are not treated as finished library entries until saved (except when presenting recovered files)." to drop the parenthesis.
- [ ] Detail: remove "when a recording was recovered after an interruption (…)" from the warning-note sentence.
- [ ] Status table: delete the **Recovered** row.
- [ ] v1 scope: change "Durable capture with recovery for common failures" to "Durable capture that keeps usable audio after common failures (device loss, low disk, write errors)".

`.specs/skrivli-app-design.md`

- [ ] Line 58 table row: "`.wav.partial` while writing; finalize and recover on startup" becomes "`.wav.partial` while writing; renamed to `.wav` on finalize".
- [ ] Line 183 (single instance): keep it. It is still true.
- [ ] Line 206: delete the `%LOCALAPPDATA%\Skrivli\rescue\` path and the `AppPaths::rescue_dir()` sentence.
- [ ] §6.5: delete the "On launch, detect unfinished `.partial`…" and "…attempt recovery to `%LOCALAPPDATA%\Skrivli\rescue\`…" bullets. Replace them with the §2 flow of this plan: always try finalize; on failure leave the partial and report; no startup scan.
- [ ] §9 failures table: "Destination deleted/unmounted/full" becomes "Stop and finalize as safely as possible; if finalize fails, leave the partial and report". "Worker panic/shutdown": drop "preserve recovery artifact".
- [ ] Test list (about lines 591–605): drop "crash-interrupted recovery" and "partial file recovery". Change "recovery behavior" in the long-recording check to "finalize behavior".
- [ ] Line 626 (M4 row) and line 650 (risk row): remove "partial-file recovery", "metadata recovery" and "rescue path".

`.specs/skrivli-implementation-plan.md`

- [ ] Line 24: "Never delete usable partials without explicit recovery/cleanup logic." becomes "Never delete partials except `abandon` after a failed start."
- [ ] Line 87: drop "/rescue" from the paths item.
- [ ] M4.2 lines 236–237: change them to `- [~]` (or strike through) with a note "Removed by the remove-rescue plan, see D-23". Do not delete the history.
- [ ] Lines 244, 246 and 250: drop "crash-interrupted header", "restart recovery" and "recover partial on next boot". Reword the acceptance to "yields a playable recording or an explicit actionable error".
- [ ] Line 262: drop "except recovery presentation".

`.specs/skrivli-recordings-ui-plan.md`

- [ ] Lines 116, 126, 205 and 236: add a short "Superseded: Recovered removed (D-23)" note after each item. Keep the checkboxes as history.

`.specs/decisions.md`

- [ ] D-10: add one line at the end: "Rescue copy and startup recovery were removed later, see D-23."
- [ ] D-11 "Catalog": "`*.recovered.wav` listed" becomes "`*.recovered.wav` (from older builds) is listed as a plain WAV".
- [ ] Add **D-23 — Rescue and crash recovery removed**:
  - What was removed: the rescue copy to `%LOCALAPPDATA%\Skrivli\rescue\`, startup `*.wav.partial` → `*.recovered.wav` repair, and the Recovered flag, badge and note.
  - New failure rule: always attempt finalize, otherwise leave the partial and report the original failure.
  - No backward compatibility: old `.wav.partial` files are ignored, old `.recovered.wav` files are plain WAVs, and the old rescue folder is ignored and never deleted.
  - Why: *(fill in the product reason, for example less surface for a rarely hit path and no hidden copies outside the chosen folder)*.

`.specs/skrivli-remove-rescue-plan.md` (this file)

- [ ] Set **Status** to Done and add an evidence note (the commands and results from Phase 6).

---

### Phase 6: Verify

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] `cargo build`
- [ ] Leftover sweep. Each must return only the expected hits:
  - `rg -i rescue src tests` → none
  - `rg "recover_partials|RecoveryReport|release_partial|NOTICE_RECOVER|NOTICE_RESCUED|NOTICE_PARTIAL_KEPT|NOTE_RECOVERED|RowBadge::Recovered" src tests` → none
  - `rg -i "recovered" src` → only `ChunkFlags::RECOVERED` and the playback test variable
  - `rg -i "rescue|\.recovered\.wav" .specs` → only D-10's pointer, D-23, the superseded notes and this plan
- [ ] Manual check on Windows (record the result in the evidence note; write "Not verified" if it was not run):
  1. Put a copy of an old `Skrivli_….wav.partial` and an old `Skrivli_….recovered.wav` in the recordings folder, then launch. Expect no launch notice; the partial is untouched and not listed; the recovered file is listed as `Skrivli_….recovered` with no badge.
  2. Start recording, then make the folder read-only (or remove the USB drive it is on), then stop. Expect the folder notice. If the rename was possible, the recording is in the list with the "Problems occurred" note. No `rescue` folder is created under `%LOCALAPPDATA%\Skrivli\`.
  3. Kill the process during a recording, then relaunch. Expect no notice. The `.wav.partial` remains on disk and is not listed.

---

## 4. File checklist

| File | Change |
| --- | --- |
| `src/infrastructure/files/rescue.rs` | Delete |
| `src/infrastructure/files/mod.rs` | Drop `mod rescue` |
| `src/infrastructure/files/wav.rs` | Delete recovery functions, `release_partial` and the recovery test; fix the module doc |
| `src/services/recording.rs` | `keep_after_failure` replaces the rescue path; drop four notices and `rescue_dir` |
| `src/services/recording_harness.rs` | Drop `rescue_dir` |
| `src/app/bootstrap.rs` | Drop the startup recovery notice and the `rescue_dir` plumbing; create only the models dir |
| `src/common/paths.rs` | Drop `rescue_dir()` and its test line |
| `src/common/error.rs` | Drop the `recover`/`rescue` contexts |
| `src/domain/recording.rs` | Drop `RecordingInfo.recovered` |
| `src/infrastructure/storage/library.rs` | Drop recovered detection and the stem strip; skip partials by extension |
| `src/views/recordings.rs` | Drop the Recovered badge, note and row field |
| `src/views/recording.rs` | `notice_tone` only treats Saved as good news |
| `tests/recording_flow.rs` | Tighten the fault tests to require a finalized `.wav` |
| `.specs/*.md` | As listed in Phase 5 |

## 5. Risks

| Risk | Mitigation |
| --- | --- |
| A crash now loses the recording from the user's point of view (the partial is on disk but invisible). | Accepted product trade-off. Write it plainly in the product spec (Phase 5) and in D-23. |
| Finalizing after a folder-check failure may hang on a dead network share. | `finalize` already runs on the recording host thread, not the GPUI thread, as it did before for write faults. No new blocking on the UI. |
| Users with existing `.recovered.wav` files lose the "end may be missing" hint. | Accepted (no backward compatibility). The files still play and transcribe. |
| Hidden callers of the removed public harness method `RecordingHarness::rescue_dir`. | Only `tests/` uses the harness. The compiler catches any other use. |
