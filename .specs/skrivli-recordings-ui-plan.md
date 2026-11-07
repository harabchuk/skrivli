# Skrivli Recordings page redesign plan

**Status:** Phases 0 and 1 done. Phases 2 to 7 are not started. The code snippets have not been compiled.

**Goal in one sentence:** turn the Recordings tab from a data table with a row of buttons into a calm list and detail page, where a person can find a recording, listen to it, seek through it, and transcribe it again.

**Branch base:** `5a07e92`. Line references are to that commit and will drift.

---

## 1. The target design

```text
┌───────────────────────────────────────────────────────────────────────────┐
│  Record   [Recordings]                                                 ⚙ │
├──────────────────────────────┬────────────────────────────────────────────┤
│  Recordings 6             📁 │  Weekly sync ✎                       🗑  ⋯ │
│                              │  Today, 14:32 · 12:41 · 38 MB · WAV        │
│  Today                       │                                            │
│ ▌Weekly sync     ✓ Transcript│  ┌──────────────────────────────────────┐  │
│  14:32 · 12:41 · 38 MB       │  │ ▶  00:00 ━━━━━●───────────────  12:41 │  │
│  Design review               │  └──────────────────────────────────────┘  │
│  09:05 · 47:10 · 141 MB      │                                            │
│                              │  Transcript                             ⧉ │
│  Yesterday                   │  00:00:00  Okay, let's get started. First  │
│  Customer call      ⓘ Failed │            item is the release schedule.   │
│  16:20 · 23:58 · 72 MB       │  00:00:14  We're on track for Thursday...  │
├──────────────────────────────┴────────────────────────────────────────────┤
│  Ready                                         Model: Base (Multilingual) │
└───────────────────────────────────────────────────────────────────────────┘
```

The area under the player shows exactly one of these states:

| State | What the user sees |
| --- | --- |
| No transcript | "No transcript yet", "Runs on this PC. Your audio never leaves it.", and a primary **Transcribe** button |
| Queued | "Queued", "Starts when the current transcription finishes.", and **Cancel** |
| Running | "Transcribing", the percentage, a progress bar, and **Cancel** |
| Failed | "Transcription failed", the reason, "The recording itself is intact.", and a primary **Retry** button |
| Ready | "Transcript" heading, a Copy icon button, and a scrolling list with one multiline item per paragraph |
| Transcript unreadable | "Could not open the transcript" and the reason |

### Decisions already made

| Decision | Outcome |
| --- | --- |
| Search | Not included |
| Sort control and sortable columns | Removed. The list is always newest first |
| Export | Not included |
| Transcript display | A list with multiline items, replacing the read-only `Editor` |
| Selectable transcript text | **No longer required.** The Copy button copies the whole transcript, and the `.txt` file stays available for anything finer |
| Copy and Delete | Icon buttons with tooltips |
| Delete location | Title row, next to `⋯`, visible in every state |
| Transcribe again | In the `⋯` menu, after a confirmation |
| Status during Transcribe again | **The same Queued and Running states as a first transcription.** The old transcript is hidden while the job runs |
| No model installed | The same Transcribe button opens the existing model dialog, and transcription starts when the model is ready |

> The clickable mockup shows a thin progress strip above the old transcript during Transcribe again. That is out of date. The decision above replaces it.

---

## 2. Problems in today's code this plan must fix

These were found while reading the code. Each one is assigned to a phase.

| # | Problem | Where | Phase |
| --- | --- | --- | --- |
| 1 | **Playback never reports that it finished.** The worker blocks on `rx.recv()` and only checks `finished` after a command arrives, so the button stays on "Playing…" after the audio ends. | `services/playback.rs` `player_loop` | 5 |
| 2 | **Cancel is shown as Failed.** `JobPhase::Cancelled` and the stored `"cancelled"` status both map to `TranscriptStatus::Failed`. | `services/transcription.rs:113`, `infrastructure/storage/sidecar.rs:196` | 4 |
| 3 | **A failed or cancelled re-run hides a good transcript.** The old `.txt` is safe, because it is replaced only when the new one is ready. The status becomes Failed, though, and the view shows text only when the status is Ready. | `sidecar.rs`, `storage/library.rs:298`, `views/main_window.rs:1345` | 4 |
| 4 | **Transcribing again has no confirmation.** Transcribe is enabled on Ready rows and overwrites without asking. | `views/recordings.rs:476` | 4 |
| 5 | **Dates are shown in UTC.** `format_unix_ms` ignores the local time zone, so "Today" would be wrong near midnight. | `views/recordings.rs:489` | 0 |
| 6 | **Duration and size sort as text.** "10:00" sorts before "9:00". This goes away when sorting is removed. | `views/recordings.rs:157` | 1 |
| 7 | **Rename does not stop playback.** The open WAV can make the rename fail on Windows. Pause would make this more likely, because the file stays open for longer. | `views/main_window.rs:1036` | 5 |

---

## 3. Phases at a glance

| Phase | Goal | Depends on |
| --- | --- | --- |
| 0 | Correct, tested display logic | None |
| 1 | Replace the table with a grouped list | 0 |
| 2 | Clean title row with icon actions | 1 |
| 3 | Transcript area with one clear state | 2 |
| 4 | Transcribe again, without losing the old transcript | 3 |
| 5 | Player engine: Pause, Seek, position, finish | None |
| 6 | Player card and transcript line highlight | 3, 5 |
| 7 | Final polish and release checks | All |

Each phase should be one reviewable commit or PR, and should leave the app working. Phase 5 can run alongside phases 1 to 4.

### Rules for every phase

- Follow `AGENTS.md`: write the failing test first, and mark Arrange, Act and Assert.
- Keep `cargo fmt` and `cargo clippy --all-targets -- -D warnings` clean. `cargo test` must not need audio hardware.
- Do not block the GPUI thread on disk or audio work. Never log transcript text.
- Update `.specs/skrivli-product.md` in the same change when user-visible behavior changes.
- Check off each item here, and add a short evidence note when a phase finishes. A passing test does not prove the look, so run the app for each UI phase.

---

## Phase 0: Display logic

**Goal:** have correct, tested dates and row text before any layout changes.

**Add**
- [x] Add the [`jiff`](https://docs.rs/jiff) crate to `Cargo.toml`, and commit `Cargo.lock`. Confirm that its default features resolve the Windows system time zone, including daylight saving changes, and record the version in `decisions.md`.
- [x] Add a function that converts Unix milliseconds to civil time with `jiff`, taking the `TimeZone` as a parameter so tests can pass a fixed zone. Use `jiff::Timestamp::from_millisecond`, then `to_zoned(zone).datetime()`, and convert the result to `CivilTime`. Keep `jiff` out of `domain/`, and call `TimeZone::system()` only where the views get their row data.
- [x] Add `RecordingDay { Today, Yesterday, Earlier }` and a pure `recording_day(recorded: CivilTime, today: CivilTime)` in `domain/recording.rs`. Handle month and year boundaries.
- [x] Add row text helpers in `views/recordings.rs`:
  - Today and Yesterday rows: `14:32 · 12:41 · 38 MB`
  - Earlier rows: `Nov 3 · 31:04 · 93 MB`
  - Detail line: `Today, 14:32 · 12:41 · 38 MB · WAV`
- [x] Add `RowBadge { None, Queued, Progress(u8), Ready, Failed, Recovered }`, with a pure mapping from status and the recovered flag. An active or failed status wins over Recovered.

**Remove**
- [x] Remove `format_unix_ms` and `civil_from_days` once nothing uses them.

**Tests to write first**
- [x] Conversion gives the right civil time in a fixed zone, on both sides of a daylight saving change, and for an out-of-range timestamp, which returns an error rather than panicking.
- [x] A recording from 23:59 yesterday and one from 00:01 today land in the right groups.
- [x] 1 January groups 31 December as Yesterday.
- [x] A recording with no start time goes to Earlier and shows `—`.
- [x] Each status maps to the right badge, including Recovered with Failed.

**Done when:** every row shows local time. A manual check near a time zone offset confirms it.

**Evidence (Phase 0):** `jiff` 0.2.39 with default features, recorded as D-19 in `decisions.md`. Unit tests cover a fixed zone, both sides of the New York spring-forward change, out-of-range timestamps, local-versus-UTC day grouping, month, year and leap-day boundaries, a missing start time, and every badge mapping. `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass. A test that used the system zone returned UTC+1 on the dev PC, so the Windows zone resolves. **Not verified:** a visual check of the running app near a zone offset (the rows are not shown in the new format until Phase 1). Notes: `RecordingRow` gained `day`, `meta`, `detail_line` and `badge`, marked `allow(dead_code)` until Phase 1 and 2 read them. Sizes still use `format_size`, so the text reads `38.0 MB`, not `38 MB`. The existing Date column now shows local time.

---

## Phase 1: Grouped recordings list

**Goal:** replace the table with rows that are easy to scan.

**Add**
- [x] Add `LibraryItem { Header(RecordingDay), Recording(RecordingRow) }` and a pure function that builds the items from the catalog, in catalog order.
- [x] Add a `recording_list_row` element in `views/recordings.rs`: bold name, muted metadata line, and the badge on the right.
- [x] Add a selected style with a tinted background and a 3 px accent bar on the left, plus a hover style.
- [x] Add a list header: "Recordings", the count, and an Open folder icon button.
- [x] Render the list with Kit's `v_virtual_list`, because headers and rows have different heights and the library can hold hundreds of files. Check its API in `gpui-component-0.9.0/src/virtual_list.rs` first.
- [x] Give the list a fixed 330 px width in the normal layout.

**Change**
- [x] Change `select_library_row` in `main_window.rs` so it is called from a row click instead of `TableEvent::SelectRow`.
- [x] Change `apply_library`, `apply_transcription` and the `selected_row` lookup to use the new row data. Keep the behavior where a progress tick updates one row without reordering, and keep its test.

**Remove**
- [x] Remove `RecordingsDelegate`, `DataTable` and `TableState`, including the `recordings_table` field and `table_sub`.
- [x] Remove the Size and Transcript columns and all column sorting. This also fixes problem 6.
- [x] **Kept** the Refresh button (now an icon button). Original wording: remove the Refresh button, but first confirm that the list updates when a file is added, renamed and deleted in Explorer. `skrivli-implementation-plan.md` names `storage/watcher.rs`, but that file is not in `infrastructure/storage/`. **If the list does not update, keep a Refresh icon button next to Open folder.**
- [x] **Kept** `AppCommand::RefreshLibrary`, because the Refresh button still sends it. Original wording: remove it only if nothing else sends it. Internal calls to `library.refresh()` stay.

**Sketch (not compiled)**

```rust
fn recording_list_row(row: &RecordingRow, selected: bool, cx: &App) -> Stateful<Div> {
    h_flex()
        .id(row.id.element_id())
        .relative()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(px(8.))
        .hover(|style| style.bg(cx.theme().accent))
        .when(selected, |el| el.bg(cx.theme().accent).child(selected_bar(cx)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().truncate().font_medium().child(row.name.clone()))
                .child(div().truncate().text_xs().text_color(cx.theme().muted_foreground)
                    .child(row.meta.clone())),
        )
        .child(row_badge(row.badge, cx))
}
```

**Tests to write first**
- [x] Items contain a header only for days that have recordings.
- [x] Items keep newest-first order.
- [x] Updating progress changes one row and keeps the order.

**Spec:** update section 5 "Browsing" in `skrivli-product.md`, and replace the status table's `—` and `Ready` wording to match the badges.

**Done when:** the list is grouped, scrolls smoothly with at least 200 files, and clicking a row selects it.

---

**Evidence (Phase 1):** `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass. New unit tests cover headers only for days with recordings, catalog order, empty input, item heights, and a progress tick that changes one row and keeps the order. `RecordingsDelegate`, the table, the Size and Transcript columns, sorting and the `allow(dead_code)` on `day`, `meta` and `badge` are gone (`detail_line` keeps its allow until Phase 2). The list uses `gpui_kit::base::v_virtual_list` with a `VirtualListScrollHandle` and an explicit `Rc<Vec<Size>>` of item heights, cached on `MainWindow` and rebuilt only when the catalog changes. **Refresh decision:** a watcher exists and is wired (`platform/windows/watcher.rs`, with a 30 s fallback rescan), but add, rename and delete in Explorer were not checked in the running app, so the icon button stays (see D-20). **Not verified:** the look of the list (selected bar, hover, badge colors, truncation), scrolling with 200+ files, and clicking a row, since the app was not run. The old Date text line in the detail pane still reads from `RecordingRow::date`; Phase 2 removes it.

---

## Phase 2: Title row and actions

**Goal:** remove the wall of same-weight buttons.

**Add**
- [ ] Add a title row with the name in large text, plus the detail line below it.
- [ ] Add a pencil icon button that opens the existing rename dialog. It appears when the title row is hovered or focused.
- [ ] Add a Delete ghost icon button with the tooltip "Delete recording". It opens the existing confirmation dialog.
- [ ] Add a `⋯` ghost icon button with a dropdown menu. For now it holds one item, **Show in folder**, which sends `AppCommand::OpenRecordingsFolder`.
- [ ] Add a warning note below the title for recovered recordings: "Recovered after an interruption. The end of this recording may be missing."
- [ ] Show sidecar and recording problems the same way, instead of in the `detail` string.

**Remove**
- [ ] Remove the `Date …`, `Duration …` and `Transcript …` text lines.
- [ ] Remove the Rename, Delete and Model buttons. The status bar and Settings still open the model dialog.
- [ ] Remove the `refresh`, `rename` and similar loose parameters from `recordings_tab`. Replace them with a single `RecordingsActions` struct, which also lets the `#[allow(clippy::too_many_arguments)]` go.

**Sketch (not compiled)**

```rust
Button::new("delete-recording")
    .ghost()
    .icon(IconName::Trash)
    .tooltip("Delete recording")
    .accessibility_label("Delete recording")
    .on_click(actions.delete);

Button::new("recording-more")
    .ghost()
    .icon(IconName::Ellipsis)
    .tooltip("More")
    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
        menu.item(PopupMenuItem::new("Show in folder").icon(IconName::FolderOpen)
            .on_click(show_in_folder.clone()))
    });
```

**Check:** `views/recording.rs:37` says Kit's default assets lack some Lucide icons. Confirm that each icon in this plan actually draws, and embed any that do not, the same way that file does. The full catalog is `gpui_kit_assets::IconName`.

**Tests to write first**
- [ ] A recovered recording, a missing sidecar and a recording with problems each produce the right note, and a clean recording produces none.

**Spec:** update "Detail and playback" in section 5.

**Done when:** the title row looks like the design, every icon has a tooltip and an accessibility label, and Rename and Delete work as before.

---

## Phase 3: Transcript area states

**Goal:** show one clear state and one clear action below the player.

**Add**
- [ ] Add this enum and a pure mapping to `views/recordings.rs`:

  ```rust
  pub(crate) enum TranscriptPane {
      NoTranscript,
      Queued,
      Running { percent: u8 },
      Failed { reason: Option<SharedString> },
      Ready,
      Unreadable { reason: SharedString },
  }
  ```

- [ ] Add a render function for each state, following the table in section 1. Use Kit's `Progress` for the bar and `Spinner` for the row badge.
- [ ] Add a "Transcript" heading with a Copy icon button. Use Kit's `Clipboard` with `value_fn`, reading `presentation.loaded_transcript()`. It copies the file's text, including timestamps.
- [ ] Add a pure parser in `domain/transcript.rs`, `parse_transcript(text) -> Vec<TranscriptParagraph>`, where each paragraph has `start_ms: Option<u64>` and `text`:
  - A new item starts at a blank line or a line beginning with `[HH:MM:SS]`. This matches what `format_transcript` writes, which is one timestamp per paragraph.
  - Text with no timestamp is kept as an item with `start_ms: None`, since users can edit the file. No text is ever dropped.
- [ ] Add a `transcript_list` that fills the remaining height. Each item has a muted timestamp column and text that wraps across as many lines as it needs.
- [ ] Use **GPUI's `list` element with its `ListState`** for the list, not Kit's `List`. Kit's `ListState` measures one item and gives every row that size (`gpui-component-0.9.0/src/list/list.rs`, `prepare_items_if_needed`, and `list/cache.rs`), so paragraphs of different lengths would not fit. GPUI's `list` measures each item and only draws those in view.
- [ ] Store the paragraphs and the list state on `MainWindow`. Import GPUI's `ListState` under another name, because Kit has a type with the same name.
- [ ] Reset the list state only when the loaded text changes. A progress update must not move the scroll position, which is the rule already recorded in `decisions.md` for the editor.
- [ ] Add PageUp, PageDown, Home and End scrolling while the list has focus, using `ListState::scroll_to`.

**Sketch (not compiled)**

```rust
let paragraphs = self.transcript_paragraphs.clone(); // Rc<Vec<TranscriptParagraph>>
list(self.transcript_list.clone(), move |index, _window, cx| {
    let paragraph = &paragraphs[index];
    h_flex()
        .items_start()
        .gap_3()
        .py_1()
        .child(
            div()
                .flex_none()
                .w(px(64.))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .children(paragraph.timestamp_label()),
        )
        .child(div().flex_1().min_w_0().child(paragraph.text.clone()))
        .into_any_element()
})
.flex_1()
.min_h_0();
```

**Change**
- [ ] Change `load_selected_transcript` so it parses the text on the background task and stores the paragraphs, keeping `set_loaded_transcript` for Copy.
- [ ] Change Cancel to use the job ID as it does today. Do not fall back to `JobId::from_raw(0)` silently: render Cancel only when the selected recording has a job.

**Remove**
- [ ] Remove `TranscriptControls`, `transcript_controls` and the `play_button_label` helper once nothing uses them. Move their test cases to `TranscriptPane`.
- [ ] Remove the always-visible Transcribe, Cancel and Retry buttons and the `failure` text line.
- [ ] Remove the transcript `Editor`: the `transcript_editor` field, `EditorState`, its creation at `main_window.rs:203`, its render at `main_window.rs:1346`, and the `set_value` calls. Replace `clear_transcript_editor` with a function that clears the paragraphs.

**Tests to write first**
- [ ] Every `TranscriptStatus` maps to the right pane, with and without an active job.
- [ ] A transcript read error on a Ready recording gives `Unreadable`.
- [ ] `Failed` carries the reason only for the recording that failed.
- [ ] `parse_transcript`: two paragraphs from `format_transcript` output, a timestamp past one hour, Unicode, an empty file, text with no timestamp, and several lines under one timestamp.
- [ ] Round trip: every word in `format_transcript` output appears in the parsed paragraphs.

**Spec**
- [ ] `skrivli-product.md`: add the states to section 6. Remove the selection promises at line 23 ("selectable, copyable"), line 39 ("Select and copy") and line 170 ("supports selection, select-all, copy"), and describe a scrolling list with a Copy button.
- [ ] `skrivli-app-design.md`: update lines 22, 512 and 628, which name the read-only `Editor` and text selection. Line 512 says a plain `div` is not the transcript view, so replace that rule.
- [ ] `skrivli-implementation-plan.md`: add a note to M7.1 (line 326) saying this plan replaces the editor, and change the manual check at line 381 and the checkpoint at line 435 so they no longer require selection.
- [ ] `decisions.md`: record that selectable text was dropped, and why GPUI's `list` was used rather than Kit's `List`.

**Done when:** clicking Transcribe moves through Queued, Running and Ready on screen, and the row badge and status bar show the same percentage. With no model installed, Transcribe opens the model dialog and then starts. A long transcript scrolls smoothly, paragraphs of different lengths wrap without overlapping or being cut off when the window is resized, and Copy pastes the full text with Unicode intact.

---

## Phase 4: Transcribe again

**Goal:** let the user replace a transcript from the `⋯` menu, using the same status display as a first run, and never lose the existing transcript unless a new one succeeds.

**Behavior**

| Step | Result |
| --- | --- |
| Open `⋯` on a Ready recording | **Transcribe again…** is enabled |
| Open `⋯` while a job is queued or running, or with no transcript | The item is disabled |
| Click the item | A dialog asks "Replace this transcript?", names the model in use, and offers **Replace** and **Cancel** |
| Replace | The area shows the same Queued and Running states as phase 3 |
| No model installed | The existing model dialog opens, and the job starts when a model is ready |
| New transcript succeeds | The new text is shown |
| Cancel the job | The old transcript returns, with no error |
| Job fails | The old transcript returns, and the status bar says "Transcription failed. Your previous transcript is unchanged." |
| App closes during the job | After restart the recording shows the old transcript |

**Add**
- [ ] Add a pure function in `domain/recording.rs`:

  ```rust
  /// Status after a job ends without a new transcript, or after a restart.
  pub(crate) fn settled_transcript_status(
      stored: TranscriptStatus,
      cancelled: bool,
      has_transcript_file: bool,
  ) -> TranscriptStatus {
      match (stored, cancelled, has_transcript_file) {
          (TranscriptStatus::Failed, _, true) => TranscriptStatus::Ready,
          (TranscriptStatus::Failed, true, false) => TranscriptStatus::None,
          (status, _, _) => status,
      }
  }
  ```

- [ ] Add **Transcribe again…** above **Show in folder** in the `⋯` menu.
- [ ] Add `open_transcribe_again_dialog` next to `open_delete_dialog`, using the same dialog style. It sends the existing `AppCommand::Transcribe`. Do not add a new command.
- [ ] Add the "previous transcript is unchanged" notice.

**Change**
- [ ] Change `update_transcription`'s read side so `"cancelled"` can be told apart from `"failed"` when the library loads a sidecar. Today both become `Failed` at `sidecar.rs:196`.
- [ ] Change `storage/library.rs:298` to use `settled_transcript_status`. This also covers a crash, because `interrupt_in_progress_transcription` writes Failed at startup.
- [ ] Change `MainWindow::apply_transcription` to settle Failed and Cancelled against the catalog entry's `transcript_path`, so the presentation overlay in `status_for` shows the same answer. `TranscriptionSnapshot::transcript_status` alone cannot do this, because it does not know about the file.
- [ ] Change the reload at `main_window.rs:622` so the transcript also loads after Failed and Cancelled, not only Done.
- [ ] Change first-run Cancel so the recording returns to "No transcript yet" instead of Failed (problem 2).

**Remove**
- [ ] Remove the Transcribe path for Ready recordings from the pane. Transcribe again is the only way to overwrite (problem 4).

**Do not change**
- The commit path in `commit_transcript` and `write_transcript_text`. It already writes to a temp file and replaces the old one only at the end. Add a test to protect that, not new code.

**Tests to write first**
- [ ] `settled_transcript_status`: every combination of stored status, cancelled and file present.
- [ ] Service test with fake inference: a failing re-run leaves the old `.txt` byte-identical.
- [ ] Service test: a cancelled re-run leaves the old `.txt` byte-identical.
- [ ] Library scan: `"failed"` with a `.txt` loads as Ready, `"failed"` without one loads as Failed, and `"cancelled"` without one loads as None.
- [ ] Menu rule: the item is enabled only for Ready with no active job.

**Spec**
- [ ] `skrivli-product.md`: add Transcribe again to section 6, including that the old transcript is hidden during the job and kept unless the new one succeeds. Correct the status table's Failed row. Add Cancel returning to the previous state.
- [ ] `skrivli-app-design.md`: record the settling rule.

**Done when:** all rows in the behavior table are confirmed in the running app, including killing the process during a job.

---

## Phase 5: Player engine

**Goal:** move playback to Rodio, so the player can pause, seek, report its position, and report when it finishes, with less of our own audio code. This phase has no visible layout change.

**Design rules**
- **Extend `PlayerHost`.** Do not add a separate playback service. It already owns the worker thread, shutdown handling and bootstrap wiring, and it has no GPUI dependency.
- **Keep Rodio in one file.** Only `infrastructure/files/player.rs` uses Rodio, behind the `PlaybackOutput` port.
- **Commands go one way.** They are `play`, `pause`, `resume`, `seek` and `stop`. Position is not a command, because asking for it would block the GPUI thread on a reply. The worker publishes it.
- **Publish state, not events.** The bridge keeps only the latest `PlayerSnapshot` in a batch (`views/actions.rs:236`), so a one-off event could be lost. The latest snapshot always gives the correct state:

  | Situation | In `PlayerSnapshot` |
  | --- | --- |
  | Position changed | `position_ms` changes |
  | Playback finished | `phase` is `Idle` and `position_ms` is 0 |
  | Playback error | `phase` is `Error`, with a `notice` |

- **Poll for the end.** Rodio's end of playback is detected by polling `empty()` on the worker. Do not wait on a blocking call such as `sleep_until_end`, because the worker must keep handling commands.

**Spike first**

Answer these on real hardware before the rest of the phase, and record the results and pinned version in `decisions.md`. If Rodio fails one that cannot be worked around, stop and revisit this phase.
- [ ] Pick a Rodio release that uses cpal 0.16, so the build does not include two copies of cpal. `infrastructure/audio/devices.rs` and `domain/audio.rs` still use cpal directly.
- [ ] Confirm the API names in that release. The player type may be `Sink` or `Player`, and this plan assumes `pause`, `play`, `try_seek`, `get_pos` and `empty`.
- [ ] Turn off default features and enable only WAV.
- [ ] Confirm the output stream can be created and kept on the worker thread.
- [ ] Confirm seeking is accurate and quick on a WAV longer than one hour.
- [ ] Confirm a recording plays when the device's sample rate differs from the file's.

**Add**
- [ ] Add the `rodio` dependency, and commit `Cargo.lock`.
- [ ] Add `PlayerPhase::Paused`.
- [ ] Add `position_ms: u64` and `duration_ms: Option<u64>` to `PlayerSnapshot`.
- [ ] Add `PlayerCommand::Pause`, `Resume` and `Seek { position_ms }`, and a `start_ms` field on `Play`.
- [ ] Add `PlayerHost::pause`, `resume` and `seek`.
- [ ] Add a port in `services/ports.rs` so tests need no audio device. The factory is `Send`, and the session stays on the worker thread:

  ```rust
  pub(crate) trait PlaybackOutput: Send {
      fn open(&self, path: &Path, start_ms: u64) -> Result<Box<dyn PlaybackSession>, AppError>;
  }

  pub(crate) trait PlaybackSession {
      fn position_ms(&self) -> u64;
      fn duration_ms(&self) -> Option<u64>;
      fn finished(&self) -> bool;
      fn set_paused(&self, paused: bool);
      fn seek(&self, position_ms: u64) -> Result<(), AppError>;
  }
  ```

**Change**
- [ ] Change `infrastructure/files/player.rs` to a thin Rodio implementation of `PlaybackOutput`. `finished` wraps `empty()`, and `position_ms` wraps the position call.
- [ ] Change `player_loop` from `rx.recv()` to `rx.recv_timeout(Duration::from_millis(100))`. On each timeout, check `finished` and publish a snapshot if the position changed (problem 1).
- [ ] Change `PlayerHost::spawn` to take the output port. `app/bootstrap.rs` passes the Rodio output.
- [ ] Change `rename_selected` to stop playback before renaming, as `delete_selected` already does (problem 7).
- [ ] Change the doc comment at `transcript_reader.rs:3`, which mentions the playback `WavReader`.

**Remove**
- [ ] Remove `infrastructure/files/reader.rs` and its `mod` line. Only the player uses it.
- [ ] Remove the cpal stream code, the audio callback and the `FromSample` trait from `player.rs`.
- [ ] Do not remove the `cpal` dependency, because device code still uses it.

**Rules**
- Reaching the end gives `Idle` with position 0.
- Seeking while paused stays paused.
- `Stop` stays as an internal command for selection change, delete and rename.
- The bridge already keeps only the latest player snapshot, so position updates cannot build up.

**Tests to write first, with a fake output**
- [ ] A finished session publishes `Idle` with no further commands. This is the regression test for problem 1.
- [ ] Pause then Resume gives Paused then Playing.
- [ ] Seek forwards the position and clamps beyond the end.
- [ ] Position updates publish while playing, and not while paused.
- [ ] An open failure publishes `Error` with a notice, and the next Play works.
- [ ] Commands are still handled while a session is playing.

**Spec:** update `skrivli-app-design.md` with Rodio, the new snapshot and the port, and replace the playback note at `decisions.md:104`. Keep the product spec change for phase 6, when the user can see it.

**Done when:** tests pass, and on real hardware a recording plays to the end and returns to `Idle` with no click or hang on pause and seek.

---

## Phase 6: Player card and paragraph highlight

**Goal:** give the user a real player, and connect it to the transcript.

**Add**
- [ ] Add a `player_card`: round Play/Pause button, current time, seek bar, total time. Times use `format_duration_ms`.
- [ ] Add a seek `Slider` with its own `SliderState` in `MainWindow`, following the existing `gain_slider`.
- [ ] Add a `seek_drag: Option<u64>` field. While the user drags, show the drag position and ignore position updates. Send the seek only on `SliderEvent::Release`.
- [ ] Add a pure `current_paragraph(paragraphs, position_ms) -> Option<usize>`. Paragraphs with no timestamp are skipped.
- [ ] Add a tinted background to the current paragraph's list item while a recording is playing or paused. A paragraph can span up to 60 seconds (`PARAGRAPH_MAX_SPAN`), so the highlight moves by paragraph, not by line.
- [ ] Add auto-scroll with `ListState::scroll_to_reveal_item`. Call it only when the current paragraph changes, and not within 5 seconds of the user scrolling the list.

**Change**
- [ ] Change the Play action: Idle plays from the slider position, Playing pauses, and Paused resumes.
- [ ] Change the player to show position 0 when `player_snapshot.recording_id` is not the selected recording.
- [ ] Change non-WAV recordings so the player is disabled with the tooltip "Only WAV playback is available", replacing the notice at `main_window.rs:1030`.

**Remove**
- [ ] Remove the Stop button, the "Playing…" label and the separate "Playing" text line.

**Sketch (not compiled)**

```rust
h_flex()
    .h(px(52.))
    .gap_3()
    .px_3()
    .border_1()
    .border_color(cx.theme().border)
    .rounded(px(10.))
    .child(
        Button::new("play-pause")
            .primary()
            .rounded(ButtonRounded::Full)
            .icon(if playing { IconName::Pause } else { IconName::Play })
            .tooltip(if playing { "Pause" } else { "Play" })
            .on_click(actions.play_pause),
    )
    .child(time_label(position))
    .child(Slider::new(&seek_state).flex_1().accessibility_label("Seek"))
    .child(time_label(duration));
```

**Tests to write first**
- [ ] `current_paragraph`: before the first paragraph, between paragraphs, after the last, and with paragraphs that have no timestamp.
- [ ] Play action: each phase produces the right command.
- [ ] Position shows 0 when the snapshot is for another recording.

**Spec:** in section 5 of `skrivli-product.md`, change "play / stop" to play, pause and seek, and add the paragraph highlight.

**Done when:** on a recording longer than one hour, dragging, pausing, resuming and changing selection all work, and the bar never jumps during a drag. The highlight follows playback, and scrolling by hand is not pulled back straight away.

---

## Phase 7: Final polish

**Goal:** finish the empty states, keyboard use and narrow windows, and confirm the whole page.

**Add**
- [ ] Add an empty library state: icon, "No recordings yet", and a primary **Start recording** button that switches to the Record tab.
- [ ] Add auto-selection of the newest recording when the tab opens with nothing selected. Keep "Select a recording" as a fallback.
- [ ] Add keyboard support:
  - Up and Down move through the list.
  - Space plays or pauses, unless a dialog is open.
  - F2 renames.
  - Delete opens the delete confirmation.
  - Escape closes menus and dialogs.
- [ ] Add a narrow layout: under 680 px wide, stack the list above the detail.

**Change**
- [ ] Change the spacing to one scale: 4, 8, 12, 16 and 20 px, with 8 to 12 px corner radii.
- [ ] Change all colors to theme tokens, and check light and dark themes.

**Spec**
- [ ] Reread section 5 and section 6 of `skrivli-product.md` against the running app.
- [ ] Update the M5.2 and M7 notes in `skrivli-implementation-plan.md`.
- [ ] Set this plan's status to done, with evidence notes.

**Final manual checks**
- [ ] First run: no recordings, then record one, and it appears selected under Today.
- [ ] Record → transcribe → copy works with no network, using an installed model.
- [ ] No model: Transcribe downloads a model and then starts.
- [ ] Transcribe again: success, cancel, failure and killing the app.
- [ ] Queue two recordings, then switch between them during the run.
- [ ] Rename and delete during playback, and during a job.
- [ ] Add, rename and delete a file in Explorer while the tab is open.
- [ ] 200 or more recordings: scrolling stays smooth.
- [ ] Screen reader: icon buttons announce their labels.

---

## Open questions

This has a default, so it does not block the work.

1. **Refresh button.** Default: remove it if the Explorer check in phase 1 passes, and otherwise keep it as an icon button.
