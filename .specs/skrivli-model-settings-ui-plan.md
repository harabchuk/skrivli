# Skrivli model dialog, Settings and status bar plan

Status: **Phases 1-5 done** (steps 1.1-5.3, automated tests only; manual tests not run, so card layout is unseen). Phase 6: steps 6.1 and 6.3 done in code; 6.2 and 6.4 are manual checks, pending (see `decisions.md`). Decision 1 approved. Phase 7 not started. The [GPUI code](#9-gpui-code) is written and compiles, but it has not been applied to `src/` or seen running.

**Goal in one sentence:** make it obvious which transcription models are installed and which one is in use, let the user change model at any time, and give the dialogs a consistent, modern look.

---

## 1. Why we are doing this

### What the user sees today

| Where | Problem |
| --- | --- |
| Model dialog | Rows are plain text with technical IDs such as `base-q5_1 (56.9 MB)`. Nothing explains how the models differ. |
| Model dialog | Nothing shows which models are already downloaded or which one is in use. |
| Model dialog | Download, Cancel and Retry are always visible, and at least one of them is always greyed out. |
| Model dialog | Progress is a line of text below the list, not attached to the model being downloaded. |
| Settings | Shows only one checkbox and an About button. The model cannot be seen or changed here. |
| Status bar | Can show internal text such as `Shell ready (5)` or `Recording Idle`. It does not show the model. |
| All dialogs | Width, spacing and button layout are not consistent. |

### Behaviour problems found in the code

The visual work depends on fixing these, so they come first.

1. **Clicking a model switches to it immediately.** `select_prompt_model` saves the choice even if that model is not installed. The next transcription then asks for a download, even when another model is already installed.
2. **The dialog can trap the user.** It cannot be closed until a model is installed, and Cancel only works during a download. With no network there is no way out.
3. **The app does not know what is installed at startup.** The list is only loaded when the model dialog opens, so the status bar and Settings would have nothing to show.
4. **Transcription does not resume after a download.** The user has to click Transcribe a second time.
5. **Retry can download the wrong model.** The worker retries the last requested model, even if the user has since selected a different one.
6. **The app doesn't notice when the dialog closes.** `model_prompt_open` is only cleared when a download finishes. Once the dialog is closable, a stale flag could make a later download close the wrong dialog, such as Settings.

---

## 2. Target design

The wireframes show layout only. Final colours come from the Kit theme, so light and dark modes both work.

### Model dialog

```text
┌ Transcription model ───────────────────────────────┐
│ Runs on this PC. Your audio never leaves it.       │
│                                                    │
│ ┌────────────────────────────────────────────────┐ │
│ │ ○ Tiny                                 30.7 MB │ │
│ │   Quick drafts and short notes                 │ │
│ │   Speed ●●●   Accuracy ●○○                     │ │
│ └────────────────────────────────────────────────┘ │
│ ┌────────────────────────────────────────────────┐ │
│ │ ○ Base  [✓ In use]                  On this PC │ │
│ │   Good balance for most meetings               │ │
│ │   Speed ●●○   Accuracy ●●○                     │ │
│ └────────────────────────────────────────────────┘ │
│ ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓ │
│ ┃ ◉ Small                               181.3 MB ┃ │
│ ┃   Best accuracy, takes longer to run           ┃ │
│ ┃   Speed ●○○   Accuracy ●●●                     ┃ │
│ ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛ │
│────────────────────────────────────────────────────│
│ Download needs a network                           │
│                        [Close] [Download 181.3 MB] │
└────────────────────────────────────────────────────┘
```

While downloading, the selected card gains a progress bar and the footer shows only **Cancel download**:

```text
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ ◉ Small                               181.3 MB ┃
┃   Best accuracy, takes longer to run           ┃
┃   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░░░░░░░░░░░       ┃
┃   Downloading 42%    About 76.1 MB of 181.3 MB ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

### Which buttons appear

| Selected model | Dialog state | Buttons |
| --- | --- | --- |
| Not installed | Idle | Close, **Download 181.3 MB** |
| Installed, not in use | Idle | Close, **Use Small** |
| In use | Idle | **Done** |
| Any | Downloading | Cancel download |
| The model that failed or was cancelled | Failed or cancelled | Close, **Retry download** |
| A different model from the one that failed | Failed or cancelled | Same as the Idle rows above |

### Settings

```text
┌ Settings ──────────────────────────────────────────┐
│ TRANSCRIPTION                                      │
│ ┌────────────────────────────────────────────────┐ │
│ │ Base  [✓ In use]              [Change model…]  │ │
│ │ Good balance for most meetings · 56.9 MB       │ │
│ └────────────────────────────────────────────────┘ │
│ GENERAL                                            │
│ ┌────────────────────────────────────────────────┐ │
│ │ Close to the notification area           (on)  │ │
│ │ Skrivli keeps running when you close the window│ │
│ └────────────────────────────────────────────────┘ │
│────────────────────────────────────────────────────│
│ Skrivli 0.1.0                              [Done]  │
└────────────────────────────────────────────────────┘
```

### Status bar

```text
Ready                                        ● Model: Base
Recording 00:12:41                           ● Model: Base
Transcribing 42%                             ● Model: Base
Ready                     ● No model installed · Choose…
Ready                          ● Downloading Small 42%
```

The left side reports what the app is doing. The right side is one clickable item that opens the model dialog.

---

## 3. Decisions to confirm before starting

| # | Decision | Proposed answer | Why it needs confirming |
| --- | --- | --- | --- |
| 1 | **Approved.** Can the model dialog be closed before a model is installed? | **Yes**, at any time except during a download. Closing just abandons that Transcribe action, and the status bar keeps showing "No model installed". | This reverses a rule in `skrivli-product.md` section 6, which says the dialog cannot be closed until the model is installed. |

Phases 1–6 assume the proposed answer. If it is rejected, only step 1.5 changes: the dialog stays locked when it was opened by Transcribe, and it needs another way out when there is no network.

### Choices already made in this plan

These are easy to change if you disagree.

- **Selecting and using are separate.** Clicking a card only highlights it. The saved model changes when the user clicks **Use** or a download succeeds.
- **A successful download becomes the model in use.** The user just chose it, so asking again adds a step for no benefit.
- **When Transcribe opened the dialog, it closes after install and the transcription starts.** When the user opened the dialog themselves, it stays open and shows the new "In use" tag.
- **Descriptions and ratings live in the view, not `manifest.json`.** The manifest is an integrity file for downloads. Its entries stay as they are.
- **Ratings are general comparisons between models.** They are not measured on the user's PC, and no timings are shown.
- **The cards build on Kit's `ListItem`, with a Kit `Radio` inside.** `ListItem` provides click, disabled and accessibility handling but has no focus handle, so the `Radio` is the keyboard stop.
- **`AppCommand::About` stays.** Only the Settings button goes. The reserved `ShowAbout` action in `app/menus.rs` still dispatches the command.

### Not in this plan

- **Downloads that continue after the dialog closes.** The dialog stays open during a download. The status bar item shows progress, but only behind the open dialog for now.
- **Deleting a downloaded model from the app.**
- **The other M8.1 settings** such as devices, gain, language and threads. They can reuse the Settings layout from Phase 5 when they are built.
- **Installing a model from a local file.** The existing `accept_file` code stays unused.

---

## 4. How to work through this plan

- Do the phases in order. Each phase leaves the app working and can be its own commit.
- Follow `AGENTS.md`: write the failing test first, and mark Arrange, Act and Assert. Default `cargo test` must not need hardware or a network.
- Keep decisions in small pure functions that can be tested without GPUI. Rendering code only displays their results.
- Each step ends with `cargo fmt` and `cargo clippy --all-targets -- -D warnings` clean.
- Update `.specs/` in the same commit as the behaviour. Each phase lists what to change.
- Check off steps here as they finish, and add any measurements or surprises to `.specs/decisions.md`.

---

## 5. The steps

### Phase 1: Fix how model selection works

**Code:** [9.5 state](#95-viewsstaters), [9.6 actions](#96-viewsactionsrs) and [9.7 main window](#97-viewsmain_windowrs).

**What this phase achieves:** the app knows what is installed, clicking a model no longer changes anything by accident, and the user can always leave the dialog. It looks almost the same afterwards, but every later phase relies on this.

#### Step 1.1: Load the installed-model list at startup

- [x] **What it does:** asks the model worker for the installed list as soon as the main window is created.
- **Why:** the status bar and Settings need this before the model dialog has ever been opened.
- **Changes:** call `model_host.refresh()` during `MainWindow` setup. Add a `model_files_known` flag to `Presentation` (`views/state.rs`), set by the first snapshot, so the app can tell "not checked yet" from "nothing installed".
- **Tests:** `Presentation` reports not known before any snapshot, and known after an Idle snapshot with an empty list.
- **Done when:** the flag is set shortly after launch, with no network use and no blocking on the GPUI thread.

#### Step 1.2: Work out each model's state in one place

- [x] **What it does:** adds one function that decides whether a model is `InUse`, `Installed` or `NotInstalled`.
- **Why:** the dialog, Settings and status bar must never disagree. A model only counts as in use if it is selected **and** its file is installed.
- **Changes:** add a `ModelInstallState` enum and `model_install_state(id, selected_id, files)` to `views/state.rs`, reading `ModelFile::variant_id`. It lives there because both `Presentation` and the dialog need it.
- **Tests:** selected and installed gives `InUse`. Selected but missing gives `NotInstalled`. Installed but not selected gives `Installed`. A file with no `variant_id` matches nothing.
- **Done when:** no other code compares a model ID with the installed list directly.

#### Step 1.3: Make clicking a card only highlight it

- [x] **What it does:** clicking a model no longer changes the saved setting.
- **Why:** this fixes problem 1 above. Browsing the list should have no side effects.
- **Changes:** `select_prompt_model` only sets `prompt_model_id`. Add `AppCommand::UseModel` and a matching `Effect`, which call `set_selected_model` and `save_settings`.
- **Tests:** in `views/actions.rs`, `UseModel` maps to its effect. A pure test confirms that selecting a row leaves the saved model unchanged.
- **Done when:** clicking through the list leaves `settings.json` unchanged.

#### Step 1.4: Remember which model is downloading, and save it on success

- [x] **What it does:** records the model being downloaded, and makes it the model in use when the download finishes.
- **Why:** step 1.3 removed the old save, so this replaces it at the correct moment. It also fixes the Retry problem and supplies the name shown in progress displays.
- **Changes:**
  - Add `download_target: Option<String>` and `begin_download(id)` to `Presentation`. `begin_download` sets the phase to `Downloading` straight away, which stops a double-click from queuing a second download before the first snapshot arrives.
  - Add `take_installed_target()`, which returns the target once when the phase is `Ready` and the file is in the installed list. `apply_model` in `MainWindow` saves that ID as the selected model.
  - Keep the target on `Failed` and `Cancelled`.
  - Keep calling `ModelHost::retry`. The footer only offers Retry when the highlighted model is the target (step 2.2), so the worker's last request is always the right one. This fixes the wrong-model problem with no service change.
- **Tests:** Ready hands over the target once. Failed keeps the target and hands nothing over. `begin_download` counts as busy before the first snapshot.
- **Done when:** a failed or cancelled download leaves the model in use unchanged.

#### Step 1.5: Let the user close the dialog

- [x] **What it does:** the dialog can be closed with Close, the X, Escape or the backdrop, except during a download.
- **Why:** this fixes the no-network trap, and depends on decision 1.
- **Changes:**
  - In `open_model_prompt`, drive `close_button`, `overlay_closable` and `keyboard` from `!busy`.
  - Add `on_close` to clear `model_prompt_open`, `download_target` and any pending transcription from step 1.6. Kit runs it for Close, Done, the X, Escape and the backdrop, but not for a programmatic `window.close_dialog`, so the code that closes the dialog clears its own flag.
  - Add `.on_ok(|_, _, _| false)`. Enabling the keyboard also enables Enter, which would otherwise confirm the plain dialog and close it.
  - Each time the dialog opens, reset `prompt_model_id` to the saved model, or to the manifest default if the saved ID isn't in the manifest. Today `ensure_prompt_selection` keeps a stale highlight from the last opening.
  - Remove `prompt_can_dismiss` and its test. The rule is now `can_close = !view.busy`.
- **Tests:** none of its own, because the rule is one expression in the dialog builder. It is covered by manual tests 2 and 3.
- **Why these options can change while it's open:** `WindowState` calls the dialog builder again on every render (`root.rs` in gpui-component 0.9.0), so `close_button`, `overlay_closable` and `keyboard` follow the current phase.
- **Specs:** `skrivli-product.md` section 6 and step 2 of its first-run flow; the model section of `skrivli-app-design.md`.
- **Done when:** a user with no network can open the dialog and leave it.

#### Step 1.6: Start the transcription after install

- [x] **What it does:** if Transcribe opened the dialog, the job starts as soon as the model is ready.
- **Why:** this fixes problem 4, so the user doesn't have to repeat the action.
- **Changes:**
  - Add `pending_transcription: Option<RecordingId>`, set in `enqueue_transcription` when the model file is missing.
  - After a successful download or **Use**, take the pending ID, close the dialog and call `begin_transcription` with that ID. That function already handles a recording that has since been deleted.
  - With no pending job, leave the dialog open. Opening it from the Recordings tab button, Settings or the status bar never sets a pending job.
  - Update `apply_model` so a Ready snapshot only auto-closes the dialog when a job is pending.
- **Tests:** keep the decision in a pure function, for example `after_model_ready(pending) -> CloseAndTranscribe(id) | StayOpen`. Test both results.
- **Done when:** Transcribe → Download → finish produces a transcript with no second click, and closing the dialog starts nothing.

**Phase 1 specs:** tick decision 1 in this plan and record it in `.specs/decisions.md`. Update `skrivli-product.md` section 6.

---

### Phase 2: Show installed models and one clear main button

**Code:** [9.4 model dialog](#94-viewsmodel_promptrs) and `model_tag` in [9.2](#92-uicomponentsmodel_cardrs).

**What this phase achieves:** your main complaint is fixed. The user can see what is downloaded, and the footer always offers one obvious next step.

#### Step 2.1: Add tags to the rows

- [x] **What it does:** shows a green **✓ In use** tag or an **Installed** tag on each row, and shows "On this PC" in place of the size for installed models.
- **Why:** this is the direct answer to "not clear which models are already downloaded".
- **Changes:** add `install_state` to `CatalogRow`, and fill it in `model_prompt_view` using step 1.2. Use Kit's `Tag::success()`. Include the state in each row's `accessibility_label`, for example "Base, in use".
- **Tests:** `CatalogRow` gets the right state for each case. The accessibility label includes it.
- **Done when:** both tags are readable in light and dark themes.

#### Step 2.2: Show only the buttons that apply

- [x] **What it does:** replaces the three fixed buttons with the set in the [button table](#which-buttons-appear).
- **Why:** greyed-out buttons make the user work out what is possible. One primary button tells them.
- **Changes:** add a `ModelFooter` enum and `model_footer(selected_state, phase, selected_id, download_target)` in `views/model_prompt.rs`. The primary button uses Kit's `.primary()`. The Download label includes the size. **Use** dispatches `AppCommand::UseModel`.
- **Tests:** cover every row of the table. Remove `download_button_is_the_word_download` and `download_button_label`.
- **Done when:** selecting different cards updates the footer straight away.

#### Step 2.3: Show download errors inside the dialog

- [x] **What it does:** displays a failure next to the model that failed, using Kit's `Alert::error` or danger-coloured text.
- **Why:** today some failures open a second popup over the dialog, and that popup can be the one `close_dialog` closes.
- **Changes:** in `apply_model`, only open `open_model_alert` when the model dialog is not open. Keep the distinct messages for missing, unsupported and unverifiable files, which the product spec requires.
- **Tests:** a pure function decides inline or alert, depending on whether the dialog is open.
- **Done when:** a failed download shows one message and no second popup.

**Phase 2 specs:** `skrivli-product.md` section 6, replacing "the chosen model is shown apart from the list". Update `skrivli-app-design.md`, which has the same wording.

---

### Phase 3: Turn the list into proper model cards

**Code:** [9.2 model card](#92-uicomponentsmodel_cardrs) and [9.4 model dialog](#94-viewsmodel_promptrs).

**What this phase achieves:** the models look distinct, and the user can choose between them without knowing what "q5_1" means.

#### Step 3.1: Add plain names and descriptions

- [x] **What it does:** adds the friendly text for each model.
- **Why:** technical IDs mean nothing to most users.
- **Changes:** add `model_copy(id) -> ModelCopy` to `views/model_prompt.rs`, returning the name, description and ratings. An unknown ID falls back to showing the ID itself.

  | ID | Name | Description | Speed | Accuracy |
  | --- | --- | --- | --- | --- |
  | `tiny-q5_1` | Tiny | Quick drafts and short notes | 3 of 3 | 1 of 3 |
  | `base-q5_1` | Base | Good balance for most meetings | 2 of 3 | 2 of 3 |
  | `small-q5_1` | Small | Best accuracy, takes longer to run | 1 of 3 | 3 of 3 |

- **Tests:** every bundled manifest entry has real copy, so a newly added model fails the test until it is described. An unknown ID falls back to itself.
- **Done when:** raw IDs no longer appear anywhere in the UI.

#### Step 3.2: Build the card

- [x] **What it does:** gives each row a border, radio dot, name, tags, description, size and rating dots. The selected card gets a stronger border and a tinted background.
- **Why:** this makes the list clearly separated, which was the first thing you asked for.
- **Changes:**
  - Add `ui/components/model_card.rs`, wrapping `ListItem` so clicks and disabled state still work. Use a Kit `Radio` for the dot, which also gives the card a keyboard stop.
  - Add a small `rating_dots(filled, total)` element.
  - Remove the "Selected: …" line, `selected_model_line`, `catalog_row_label` and their tests.
  - During a download, dim the other cards and disable them.
- **Tests:** rendering is checked by hand. Keep any logic, such as how many dots are filled, in tested functions.
- **Done when:** the list matches the wireframe in light and dark themes, and at 100% and 150% scaling.

#### Step 3.3: Move progress into the card

- [x] **What it does:** shows a Kit `Progress` bar, the percentage and "About 76.1 MB of 181.3 MB" inside the card being downloaded.
- **Why:** it is clear which model the progress belongs to, and the layout doesn't jump.
- **Changes:** add `download_progress_text(percent, total_bytes)`, reusing `format_size`. The total comes from the manifest, so the worker needs no change. Show the card's failure message in the same spot.
- **Tests:** text at 0%, 42% and 100%. The result is marked "About" because it is calculated from a whole-number percentage.
- **Done when:** progress moves smoothly and no text overflows the card at the smallest window size.

#### Step 3.4: Add the header and footer note

- [x] **What it does:** adds the subtitle "Runs on this PC. Your audio never leaves it." and moves the network notice to the footer, shown only when a download is on offer.
- **Why:** it answers the privacy question up front, and removes a network warning that doesn't apply to installed models.
- **Changes:** use `.title()` for the heading and add a muted subtitle. Reuse the helper from step 6.1 if that has landed.
- **Done when:** the network notice only appears when the selected model isn't installed.

**Phase 3 specs:** `.specs/skrivli-product.md` section 6, describing how models are presented.

---

### Phase 4: Status bar

**Code:** `status_line` and `ModelStatus` in [9.5](#95-viewsstaters), `model_status_item` in [9.4](#94-viewsmodel_promptrs), and the `render` changes in [9.7](#97-viewsmain_windowrs).

**What this phase achieves:** the current model is always visible and one click from being changed, and the status text reads naturally.

#### Step 4.1: Clean up the status text

- [x] **What it does:** shows `Ready`, `Starting`, `Recording 00:12:41`, `Saving` or `Transcribing 42%`, with any notice taking priority.
- **Why:** users shouldn't see `Shell ready (5)` or `Recording Idle`.
- **Changes:** rewrite `Presentation::status_line` to take the elapsed time as an argument, because `MainWindow::render` is what holds the start time. Stop showing `shell_pulses`. The bridge test still counts them, so the accessor stays with `#[cfg_attr(not(test), allow(dead_code))]`, as this codebase already does elsewhere.
- **Tests:** update the `status_line` assertions in `views/state.rs` and `views/actions.rs`, including `"Shell ready (5)"`. Add one case per recording phase, one for transcription, and one for a notice.
- **Done when:** none of the internal text can appear.

#### Step 4.2: Work out what the model item should say

- [x] **What it does:** adds one function that decides the text and colour of the status bar's model item.
- **Why:** this keeps the logic testable and matches the rest of the app through step 1.2.
- **Changes:** add `ModelStatus` with `Checking`, `InUse { id }`, `Missing` and `Downloading { id, percent }`, and `model_status(...)` in `views/state.rs`.
- **Tests:** not-known gives `Checking`, so "No model installed" doesn't flash at launch. Selected but not installed gives `Missing`. A download in progress wins over everything else.
- **Done when:** every state has a test.

#### Step 4.3: Add the clickable item

- [x] **What it does:** adds the model item on the right of the status bar, with a coloured dot and hover highlight. Clicking it opens the model dialog.
- **Why:** this is the status bar improvement you asked for.
- **Changes:**
  - Replace the hand-built strip in `MainWindow::render` with Kit's `StatusBar`, keeping the 28px height. Put the status text in `.child()` and the model item in `.right()`. With only a right region, Kit left-aligns the center and gives it the free space.
  - Build the item from a ghost Kit `Button`, which provides focus, hover and an accessible name.
  - Add `AppCommand::OpenModelDialog`, used by this item and Settings.
  - Show nothing for `Checking`. Add a tooltip and accessibility label: "Transcription model: Base. Click to change."
- **Check by hand:** long notices must not push the model item out of a 560px window. The left text should truncate first.
- **Specs:** `skrivli-app-design.md` section 8 (status strip) and `skrivli-product.md`, which should mention that the current model is shown.
- **Done when:** the item is correct across launch, download and recording, and works from the keyboard.

---

### Phase 5: Settings

**Code:** [9.3 settings rows](#93-uicomponentssettings_rowsrs) and [9.8 Settings dialog](#98-viewssettingsrs).

**What this phase achieves:** the current model is visible in Settings and can be changed from there, in a layout the later settings can use.

#### Step 5.1: Add reusable section and row helpers

- [x] **What it does:** adds the grouped section layout: an uppercase label, a bordered group, and rows with text on the left and a control on the right.
- **Why:** every M8.1 setting will need this, so it should only be written once.
- **Changes:** add `settings_section(title, rows, cx)` and `settings_row(title, description, control, cx)` in `ui/components/settings_rows.rs`. Don't use Kit's `setting` module for now: its `Settings` component is built around a sidebar with pages, which is too much for two settings. Revisit this during M8.1, when the list is longer. The outcome is recorded as D-17 in `decisions.md`.
- **Done when:** the helpers render correctly in both themes.

#### Step 5.2: Rebuild the Settings dialog

- [x] **What it does:** applies the new layout, replaces the checkbox with a Kit `Switch`, adds a line explaining it, and adds the footer.
- **Why:** this is the first look most users get at Settings, and today it looks unfinished.
- **Changes:**
  - Add a `SettingsView` struct, read through a closure the same way `ModelPromptView` is, replacing the single `checked` closure.
  - Update `views/settings.rs` and `Effect::OpenSettings`.
  - Replace the About button with "Skrivli 0.1.0" in the footer, using `CARGO_PKG_VERSION`, next to **Done**.
  - Keep `open_about`, `AppCommand::About` and `Effect::OpenAbout`, because the reserved `ShowAbout` action in `app/menus.rs` still dispatches them. Only the button goes.
- **Tests:** a pure function builds the version text.
- **Specs:** `skrivli-product.md` section on navigation (line 52 today: "About is available from that dialog"), and the same wording in `skrivli-app-design.md`.
- **Done when:** the close-to-tray switch still saves, and the tray's Settings command still opens the dialog.

#### Step 5.3: Add the Transcription section

- [x] **What it does:** shows the model in use, with its tag, description and size, plus **Change model…**. If nothing is installed, it shows "No model installed" and **Choose model…**.
- **Why:** this is the Settings improvement you asked for.
- **Changes:** add a row built from `model_status` and `model_copy`. The button dispatches `AppCommand::OpenModelDialog`, which opens the model dialog on top of Settings.
- **Tests:** a pure function builds the row text for each model state.
- **Check by hand:** closing the model dialog returns to Settings, Settings shows the new model right away, and a finished download doesn't close Settings.
- **Specs:** tick the transcription-model part of M8.1 in `skrivli-implementation-plan.md`. Update the sentence in `skrivli-app-design.md` that says choosing another variant in Settings is M8.
- **Done when:** a model can be changed entirely from Settings.

---

### Phase 6: Make all dialogs consistent

**Code:** [9.1 dialog style](#91-uicomponentsdialog_stylers).

**What this phase achieves:** every dialog feels like part of one app.

#### Step 6.1: Define the shared style

- [x] **What it does:** puts width, padding and subtitle styling in one place.
- **Why:** consistency is easier to keep when there is one place to change it.
- **Changes:** add `ui/components/dialog_style.rs` with `DIALOG_WIDTH` at 520px, plus `dialog_subtitle`, `dialog_footer` and `dialog_footer_note`. Change the model dialog from 560px to this width.
- **Rules for all dialogs:**
  - A divider above the footer, drawn by `dialog_footer` with `border_t_1`. It stops at the dialog's padding, because Kit wraps the footer in a padded container.
  - The main action on the far right. Destructive actions use the danger style.
  - Theme colours only, with no fixed colours.
  - Sentence-case titles and button labels that name the action, such as "Stop and exit" instead of "OK".
- **Done when:** the model dialog and Settings use only these shared values.

#### Step 6.2: Make dialogs fit small windows

- [ ] **What it does:** confirms that long dialogs stay usable when the window is at its 560×420 minimum.
- **Why:** three cards plus a header and footer come close to the available height.
- **Changes:** probably none. Kit 0.9.0 already limits a dialog's height to the window, scrolls the body with its own scrollbar and keeps the footer outside the scrolling area. Don't add a second scroll container. Use `.centered()` or `.margin_top()` only if the default position wastes space.
- **Check by hand:** at 560×420, with scaling at 100% and 150%, the footer buttons stay visible and reachable.
- **Done when:** results are recorded under the D-02 layout checks in `decisions.md`.

#### Step 6.3: Bring the remaining dialogs into line

- [x] **What it does:** applies the shared style to the rename, delete, tray explanation and exit-while-recording dialogs. (Code done: shared width on all, rename uses `dialog_footer`. The side-by-side theme check is pending manual verification.)
- **Why:** these are short and mostly fine, so this is a light pass and not a redesign.
- **Changes:** use the shared width and check button order. Rename already uses a primary button and delete already uses the danger style, so expect small changes. Keep their current wording unless it breaks the rules above.
- **Done when:** a side-by-side check of each dialog in both themes shows no inconsistencies.

#### Step 6.4: Check the keyboard path

- [ ] **What it does:** walks the model dialog, Settings and status bar item with only the keyboard.
- **Why:** custom layouts can break focus order and focus rings. This also covers part of the M8.2 keyboard pass.
- **Check by hand:** Tab reaches every card's radio and every button in a sensible order, Space selects a radio, Enter on the dialog does nothing unexpected, Escape closes the dialog unless busy, and focus rings stay visible.
- **Done when:** problems are fixed, or written down in `decisions.md`.

---

### Phase 7: Final check

**What this phase achieves:** confirmation that the whole experience works, recorded so nobody has to repeat it blindly.

- [ ] **Run the automated checks:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.
- [ ] **Run the manual pass** in the real app, in light and dark themes, using the list in [section 6](#6-manual-test-list).
- [ ] **Re-read the specs:** search `.specs/` for "cannot be closed", "Selected:", "About" and "Choosing another variant" to catch old wording.
- [ ] **Record the results** in `.specs/decisions.md`, and mark this plan's status as done.

---

## 6. Manual test list

| # | Do this | Expect |
| --- | --- | --- |
| 1 | Launch with no models installed | Status bar shows "No model installed · Choose…" without first flashing anything else |
| 2 | Click that item, then close the dialog | Dialog closes, nothing changes |
| 3 | Disconnect the network, click Transcribe, try Download | Failure shows in the card, Retry is offered, and the dialog can be closed |
| 4 | Reconnect, click Transcribe, then Download | Progress shows in the card. On finish the dialog closes and the transcript starts by itself |
| 5 | Open the dialog from Settings, download another model | Dialog stays open with the new "In use" tag. Settings and the status bar update |
| 6 | Click between installed models without pressing Use, then Close | The model in use doesn't change |
| 7 | Click **Use** on another installed model | Tag, Settings and status bar all change. The dialog stays open with **Done** |
| 8 | Cancel a download partway | Previous model stays in use, Retry is offered, and no partial file is accepted |
| 9 | Fail a download, select a different model | Footer offers Download for that model, and downloads the right one |
| 10 | Delete the in-use model file by hand, reopen the dialog | The model shows as not installed, and the status bar warns after the next refresh |
| 11 | Change model while a transcription is running | The running job finishes on the old model, and the next job uses the new one |
| 12 | Make the window 560×420 at 100% and 150% scaling | All dialogs fit, or scroll, with the footers reachable |
| 13 | Repeat 1, 4 and 5 in dark mode | All tags, dots and progress are readable |
| 14 | Start a download from Transcribe, open Settings from the tray, wait for it to finish | No crash and no lost setting. Close whatever stays open and the app still works |

---

## 7. Files that change

| File | What changes |
| --- | --- |
| `src/views/model_prompt.rs` | Install state, model copy, footer logic, new dialog body |
| `src/views/main_window.rs` | Startup refresh, Use, pending transcription, status bar, Settings wiring |
| `src/views/state.rs` | Download target, known flag, `ModelStatus`, new `status_line` |
| `src/views/actions.rs` | `UseModel` and `OpenModelDialog` added |
| `src/views/settings.rs` | New layout, Transcription section, `SettingsView` |
| `src/views/recordings.rs` | Rename and delete dialogs use the shared style |
| `src/ui/components/mod.rs` | Declares the three new modules |
| `src/ui/components/model_card.rs` | New: model card and rating dots |
| `src/ui/components/settings_rows.rs` | New: section and row helpers |
| `src/ui/components/dialog_style.rs` | New: shared dialog values |
| `.specs/skrivli-product.md` | Model section, first-run flow, navigation |
| `.specs/skrivli-app-design.md` | Model dialog, status strip, Settings |
| `.specs/skrivli-implementation-plan.md` | M8.1 and M8.2 progress |
| `.specs/decisions.md` | Decision 1, Kit `setting` outcome, manual results |

The services and infrastructure layers should not need changes. `ModelHost`'s worker already supplies the installed list, and the download, verification and cancel code stays as it is.

---

## 8. Risks to watch

| Risk | What could go wrong | How the plan handles it |
| --- | --- | --- |
| Enter key | Enabling the keyboard on a plain dialog lets Enter confirm and close it | Step 1.5 adds `.on_ok(|_, _, _| false)` |
| Stacked dialogs | `close_dialog` closes the top dialog. If the tray opens Settings over the model dialog during a download, a finished Transcribe job could close Settings instead | Rare, and nothing is lost. Steps 1.5, 1.6 and 2.3 remove the other cases, and manual test 14 covers this one |
| Refresh and download racing | The Idle snapshot from opening the dialog can arrive just after `begin_download`, briefly clearing the busy state | Lasts milliseconds, and the next snapshot restores it |
| Refresh timing | The worker handles one command at a time, so a refresh sent during a download only runs afterwards, and its Idle snapshot follows Ready | Step 1.4 acts on Ready once by clearing the target, so the later Idle does nothing |
| Rich content in `ListItem` | The code compiles, but the layout has not been seen running. Padding and hover may need adjusting | Step 3.2 is checked by hand, and a custom element is the fallback |
| Small windows | Three cards may not fit at 560×420 | Kit scrolls the dialog body, and step 6.2 checks the result |
| Stale installed list | A file removed by hand isn't noticed until the next refresh | Accepted for now: the dialog refreshes when it opens, and Transcribe still checks the file itself |
| Dot colours | Colour alone may not be clear enough to all users | The status bar item always includes text |

---

## 9. GPUI code

This is the end state of Phases 1–6, written against gpui-kit 0.9.0. New files are shown whole. Existing files are shown as diffs against the current `main` (`db6fb36`).

**What was checked:** this code was built in a scratch copy of the repo, where `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` all passed.

**What was not checked:** it has never been run, so nobody has seen these dialogs on screen. Spacing, colours, hover and keyboard behaviour still need the manual pass in section 6.

**How to use it:** copy the parts a step needs, phase by phase, and write the failing test first as `AGENTS.md` requires. The tests are included below each module. In Phase 2, the existing plain rows can use `model_tag` until the cards arrive in Phase 3.

**If decision 1 is rejected:** only `can_close` in `open_model_prompt` (9.4) and the Close buttons in `footer_buttons` need to change.

### How each visual maps to Kit

| What you see | Built with |
| --- | --- |
| Model card | `ListItem` with `border_1`, `rounded(radius_lg)` and `.selected()` |
| Radio dot | `Radio`, which is also the keyboard stop |
| In use and Installed tags | `Tag::success()`, with `.outline()` for Installed |
| Speed and accuracy dots | Plain 6px `div`s using `foreground` and `border` |
| Progress bar | `Progress` |
| Download error | Text in the `danger` colour inside the card |
| Dialog footer and divider | `DialogFooter` with `border_t_1`, plus `DialogClose` for Close and Done |
| Close to tray control | `Switch` |
| Settings groups | `v_flex` with a border and `rounded(radius_lg)` |
| Status bar | `StatusBar` |
| Status bar model item | Ghost `Button` with `xsmall()`, a dot and text |

### 9.1 `ui/components/dialog_style.rs`

Add this file. Also declare the three new modules in `ui/components/mod.rs`:

```rust
pub(crate) mod dialog_style;
pub(crate) mod level_meter;
pub(crate) mod model_card;
pub(crate) mod settings_rows;
```

```rust
//! Values and small pieces every Skrivli dialog shares.

use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{div, px, App, IntoElement, ParentElement, Pixels, SharedString, Styled};

pub(crate) const DIALOG_WIDTH: Pixels = px(520.);

/// Muted line under a dialog title. The dialog body scrolls, so this scrolls with it.
pub(crate) fn dialog_subtitle(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// Footer with a divider above it. Kit keeps the footer outside the scrolling body.
pub(crate) fn dialog_footer(cx: &App) -> DialogFooter {
    DialogFooter::new()
        .w_full()
        .items_center()
        .pt_3()
        .border_t_1()
        .border_color(cx.theme().border)
}

/// Muted footer text that takes the free space and pushes the buttons right.
pub(crate) fn dialog_footer_note(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
```

### 9.2 `ui/components/model_card.rs`

Add this file.

```rust
//! One selectable transcription model. Takes plain data, so it knows nothing about services.

use gpui_kit::component::list::ListItem;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, px, App, IntoElement, ParentElement, SharedString, Styled, Window};

const RATING_MAX: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelCardTag {
    InUse,
    Installed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelCardProgress {
    pub(crate) percent: u8,
    pub(crate) left: SharedString,
    pub(crate) right: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelCardData {
    pub(crate) id: SharedString,
    pub(crate) name: SharedString,
    pub(crate) description: SharedString,
    pub(crate) meta: SharedString,
    pub(crate) tag: Option<ModelCardTag>,
    pub(crate) ratings: Option<(u8, u8)>,
    pub(crate) selected: bool,
    pub(crate) disabled: bool,
    pub(crate) progress: Option<ModelCardProgress>,
    pub(crate) failure: Option<SharedString>,
    pub(crate) accessibility_label: SharedString,
}

pub(crate) fn model_tag(tag: ModelCardTag) -> impl IntoElement {
    match tag {
        ModelCardTag::InUse => Tag::success().small().rounded_full().child(
            h_flex()
                .gap_1()
                .items_center()
                .child(Icon::new(IconName::Check).xsmall())
                .child("In use"),
        ),
        ModelCardTag::Installed => Tag::success()
            .outline()
            .small()
            .rounded_full()
            .child("Installed"),
    }
}

fn rating(label: &'static str, filled: u8, cx: &App) -> impl IntoElement {
    let on = cx.theme().foreground;
    let off = cx.theme().border;
    h_flex()
        .gap_1()
        .items_center()
        .child(label)
        .children((0..RATING_MAX).map(|index| {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(if index < filled { on } else { off })
        }))
}

/// `on_select` runs from both the card and its radio. The radio is the keyboard
/// stop, because `ListItem` has no focus handle. Selecting twice is harmless.
pub(crate) fn model_card(
    data: ModelCardData,
    on_select: impl Fn(&mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let border = if data.selected {
        theme.primary
    } else {
        theme.border
    };
    let muted = theme.muted_foreground;
    let danger = theme.danger;
    let on_radio = on_select.clone();
    ListItem::new(SharedString::from(format!("model-card-{}", data.id)))
        .w_full()
        .p_3()
        .border_1()
        .border_color(border)
        .rounded(theme.radius_lg)
        .selected(data.selected)
        .disabled(data.disabled)
        .when(data.disabled && !data.selected, |this| this.opacity(0.55))
        .accessibility_label(data.accessibility_label.clone())
        .on_click(move |_event, window, cx| on_select(window, cx))
        .child(
            v_flex()
                .w_full()
                .gap_2()
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(
                            Radio::new(SharedString::from(format!("model-radio-{}", data.id)))
                                .checked(data.selected)
                                .disabled(data.disabled)
                                .accessibility_label(data.accessibility_label)
                                .on_click(move |_checked, window, cx| on_radio(window, cx)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().font_semibold().child(data.name))
                                        .when_some(data.tag, |this, tag| {
                                            this.child(model_tag(tag))
                                        }),
                                )
                                .child(div().text_xs().text_color(muted).child(data.description))
                                .when_some(data.ratings, |this, (speed, accuracy)| {
                                    this.child(
                                        h_flex()
                                            .gap_3()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(rating("Speed", speed, cx))
                                            .child(rating("Accuracy", accuracy, cx)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(muted)
                                .child(data.meta),
                        ),
                )
                .when_some(data.progress, |this, progress| {
                    this.child(
                        Progress::new(SharedString::from(format!("model-progress-{}", data.id)))
                            .w_full()
                            .value(f32::from(progress.percent))
                            .accessibility_label(progress.left.clone()),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .text_xs()
                            .text_color(muted)
                            .child(progress.left)
                            .child(progress.right),
                    )
                })
                .when_some(data.failure, |this, failure| {
                    this.child(div().text_xs().text_color(danger).child(failure))
                }),
        )
}
```

### 9.3 `ui/components/settings_rows.rs`

Add this file.

```rust
//! Grouped rows for Settings: a section label, a bordered group, text left and control right.

use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, AnyElement, App, IntoElement, ParentElement, SharedString, Styled};

pub(crate) fn settings_section(
    title: &'static str,
    rows: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let border = cx.theme().border;
    v_flex()
        .w_full()
        .gap_1p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(
            v_flex()
                .w_full()
                .border_1()
                .border_color(border)
                .rounded(cx.theme().radius_lg)
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    div()
                        .w_full()
                        .when(index > 0, |this| this.border_t_1().border_color(border))
                        .child(row)
                })),
        )
}

pub(crate) fn settings_row(
    title: impl IntoElement,
    description: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .px_3()
        .py_2p5()
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.into()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}
```

### 9.4 `views/model_prompt.rs`

Replace the whole file.

```rust
//! Transcription-model dialog. Selection, download, cancel, and retry stay off `render`.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogClose;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, AnyElement, App, ClickEvent, IntoElement, ParentElement, SharedString, Styled, Window,
};

use crate::domain::recording::format_size;
use crate::services::models::ModelPhase;
use crate::ui::components::dialog_style::{
    dialog_footer, dialog_footer_note, dialog_subtitle, DIALOG_WIDTH,
};
use crate::ui::components::model_card::{
    model_card, ModelCardData, ModelCardProgress, ModelCardTag,
};

use super::state::{ModelInstallState, ModelStatus};

const SUBTITLE: &str = "Runs on this PC. Your audio never leaves it.";
const NETWORK_NOTE: &str = "Download needs a network";

/// Friendly text for one manifest entry. Kept here because the manifest is an integrity file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelCopy<'a> {
    pub(crate) name: &'a str,
    pub(crate) description: &'a str,
    /// (speed, accuracy), each out of 3. General comparisons, not measured on this PC.
    pub(crate) ratings: Option<(u8, u8)>,
}

pub(crate) fn model_copy(id: &str) -> ModelCopy<'_> {
    match id {
        "tiny-q5_1" => ModelCopy {
            name: "Tiny",
            description: "Quick drafts and short notes",
            ratings: Some((3, 1)),
        },
        "base-q5_1" => ModelCopy {
            name: "Base",
            description: "Good balance for most meetings",
            ratings: Some((2, 2)),
        },
        "small-q5_1" => ModelCopy {
            name: "Small",
            description: "Best accuracy, takes longer to run",
            ratings: Some((1, 3)),
        },
        unknown => ModelCopy {
            name: unknown,
            description: "",
            ratings: None,
        },
    }
}

/// What the footer offers. `Retry` only appears for the model that failed, because
/// `ModelHost::retry` repeats the last requested model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelFooter {
    CancelDownload,
    Retry,
    Download,
    Use,
    Done,
    CloseOnly,
}

pub(crate) fn model_footer(
    selected: Option<(&str, ModelInstallState)>,
    phase: ModelPhase,
    download_target: Option<&str>,
) -> ModelFooter {
    if phase == ModelPhase::Downloading {
        return ModelFooter::CancelDownload;
    }
    let Some((id, state)) = selected else {
        return ModelFooter::CloseOnly;
    };
    let failed = matches!(phase, ModelPhase::Failed | ModelPhase::Cancelled);
    if failed && download_target == Some(id) {
        return ModelFooter::Retry;
    }
    match state {
        ModelInstallState::NotInstalled => ModelFooter::Download,
        ModelInstallState::Installed => ModelFooter::Use,
        ModelInstallState::InUse => ModelFooter::Done,
    }
}

pub(crate) fn download_label(bytes: u64) -> String {
    format!("Download {}", format_size(bytes))
}

pub(crate) fn use_label(name: &str) -> String {
    format!("Use {name}")
}

/// The total comes from the manifest and the percent is whole, so the amount is approximate.
pub(crate) fn download_progress_text(percent: u8, total_bytes: u64) -> (String, String) {
    let done = total_bytes.saturating_mul(u64::from(percent.min(100))) / 100;
    (
        format!("Downloading {percent}%"),
        format!(
            "About {} of {}",
            format_size(done),
            format_size(total_bytes)
        ),
    )
}

pub(crate) fn row_meta(state: ModelInstallState, bytes: u64) -> String {
    match state {
        ModelInstallState::NotInstalled => format_size(bytes),
        ModelInstallState::Installed | ModelInstallState::InUse => "On this PC".to_string(),
    }
}

pub(crate) fn row_tag(state: ModelInstallState) -> Option<ModelCardTag> {
    match state {
        ModelInstallState::InUse => Some(ModelCardTag::InUse),
        ModelInstallState::Installed => Some(ModelCardTag::Installed),
        ModelInstallState::NotInstalled => None,
    }
}

pub(crate) fn row_accessibility_label(copy: &ModelCopy<'_>, state: ModelInstallState) -> String {
    let status = match state {
        ModelInstallState::InUse => "in use",
        ModelInstallState::Installed => "installed",
        ModelInstallState::NotInstalled => "not installed",
    };
    match copy.ratings {
        Some((speed, accuracy)) => format!(
            "{}, {status}, speed {speed} of 3, accuracy {accuracy} of 3",
            copy.name
        ),
        None => format!("{}, {status}", copy.name),
    }
}

/// Text for the status bar item. `None` while the installed list is still loading.
pub(crate) fn model_status_text(status: &ModelStatus) -> Option<String> {
    match status {
        ModelStatus::Checking => None,
        ModelStatus::InUse { id } => Some(format!("Model: {}", model_copy(id).name)),
        ModelStatus::Missing => Some("No model installed · Choose…".to_string()),
        ModelStatus::Downloading { id, percent } => {
            Some(format!("Downloading {} {percent}%", model_copy(id).name))
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelPromptView {
    pub(crate) cards: Vec<ModelCardData>,
    pub(crate) footer: ModelFooter,
    /// Size of the selected model, for the Download label.
    pub(crate) selected_bytes: Option<u64>,
    /// Display name of the selected model, for the Use label.
    pub(crate) selected_name: SharedString,
    pub(crate) busy: bool,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type SelectHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Everything the dialog can ask for. A struct because the list outgrew plain arguments.
#[derive(Clone)]
pub(crate) struct ModelPromptHandlers {
    pub(crate) on_select: SelectHandler,
    pub(crate) on_download: ClickHandler,
    pub(crate) on_use: ClickHandler,
    pub(crate) on_cancel: ClickHandler,
    pub(crate) on_retry: ClickHandler,
    /// Runs for Close, Done, the X, Escape, and the backdrop. A programmatic
    /// `window.close_dialog` does not run it.
    pub(crate) on_closed: ClickHandler,
}

pub(crate) fn open_model_prompt(
    window: &mut Window,
    cx: &mut App,
    read: impl Fn(&App) -> ModelPromptView + 'static,
    handlers: ModelPromptHandlers,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = read(cx);
        let can_close = !view.busy;
        let footer = footer_buttons(&view, &handlers);
        let on_closed = handlers.on_closed.clone();
        let mut body = v_flex()
            .w_full()
            .gap_2()
            .child(dialog_subtitle(SUBTITLE, cx));
        for card in view.cards.clone() {
            let on_select = handlers.on_select.clone();
            let id = card.id.clone();
            body = body.child(model_card(
                card,
                move |window, cx| on_select(id.as_ref(), window, cx),
                cx,
            ));
        }
        let note = if view.footer == ModelFooter::Download {
            dialog_footer_note(NETWORK_NOTE, cx).into_any_element()
        } else {
            div().flex_1().into_any_element()
        };
        dialog
            .title("Transcription model")
            .w(DIALOG_WIDTH)
            .close_button(can_close)
            .overlay_closable(can_close)
            .keyboard(can_close)
            // Enter would otherwise confirm the plain dialog and close it.
            .on_ok(|_, _, _| false)
            .on_close(move |event, window, cx| on_closed(event, window, cx))
            .child(body)
            .footer(dialog_footer(cx).child(note).children(footer))
    });
}

fn footer_buttons(view: &ModelPromptView, handlers: &ModelPromptHandlers) -> Vec<AnyElement> {
    let close = |label: &'static str, primary: bool| {
        let button = Button::new("model-close").label(label);
        let button = if primary {
            button.primary()
        } else {
            button.outline()
        };
        DialogClose::new().child(button).into_any_element()
    };
    match view.footer {
        ModelFooter::CancelDownload => {
            let on_cancel = handlers.on_cancel.clone();
            vec![Button::new("model-cancel")
                .outline()
                .label("Cancel download")
                .on_click(move |event, window, cx| on_cancel(event, window, cx))
                .into_any_element()]
        }
        ModelFooter::Retry => {
            let on_retry = handlers.on_retry.clone();
            vec![
                close("Close", false),
                Button::new("model-retry")
                    .primary()
                    .label("Retry download")
                    .on_click(move |event, window, cx| on_retry(event, window, cx))
                    .into_any_element(),
            ]
        }
        ModelFooter::Download => {
            let on_download = handlers.on_download.clone();
            let label = download_label(view.selected_bytes.unwrap_or(0));
            vec![
                close("Close", false),
                Button::new("model-download")
                    .primary()
                    .label(label)
                    .on_click(move |event, window, cx| on_download(event, window, cx))
                    .into_any_element(),
            ]
        }
        ModelFooter::Use => {
            let on_use = handlers.on_use.clone();
            vec![
                close("Close", false),
                Button::new("model-use")
                    .primary()
                    .label(use_label(&view.selected_name))
                    .on_click(move |event, window, cx| on_use(event, window, cx))
                    .into_any_element(),
            ]
        }
        ModelFooter::Done => vec![close("Done", true)],
        ModelFooter::CloseOnly => vec![close("Close", false)],
    }
}

/// Status bar item. A ghost Button gives focus, hover and an accessible name for free.
pub(crate) fn model_status_item(
    status: &ModelStatus,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<AnyElement> {
    let text = model_status_text(status)?;
    let dot = match status {
        ModelStatus::InUse { .. } => cx.theme().success,
        ModelStatus::Missing | ModelStatus::Downloading { .. } | ModelStatus::Checking => {
            cx.theme().warning
        }
    };
    let tooltip = match status {
        ModelStatus::InUse { id } => format!(
            "Transcription model: {}. Click to change.",
            model_copy(id).name
        ),
        _ => "Choose a transcription model.".to_string(),
    };
    Some(
        Button::new("model-status")
            .ghost()
            .xsmall()
            .tooltip(tooltip.clone())
            .accessibility_label(tooltip)
            .on_click(on_click)
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(div().size_1p5().rounded_full().bg(dot))
                    .child(SharedString::from(text)),
            )
            .into_any_element(),
    )
}

pub(crate) fn open_model_alert(window: &mut Window, cx: &mut App, message: &'static str) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert.title("Transcription model").description(message)
    });
}

/// What to do once a model is ready. A pending job exists only when Transcribe opened the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterModelReady<T> {
    CloseAndTranscribe(T),
    StayOpen,
}

pub(crate) fn after_model_ready<T>(pending: Option<T>) -> AfterModelReady<T> {
    match pending {
        Some(id) => AfterModelReady::CloseAndTranscribe(id),
        None => AfterModelReady::StayOpen,
    }
}

/// Builds a card's progress block from the worker's percent and the manifest size.
pub(crate) fn card_progress(percent: u8, total_bytes: u64) -> ModelCardProgress {
    let (left, right) = download_progress_text(percent, total_bytes);
    ModelCardProgress {
        percent,
        left: left.into(),
        right: right.into(),
    }
}
```

<details>
<summary>Tests for this file</summary>

```rust
#[cfg(test)]
mod tests {
    use super::{
        after_model_ready, download_label, download_progress_text, model_copy, model_footer,
        model_status_text, row_accessibility_label, row_meta, use_label, AfterModelReady,
        ModelFooter,
    };
    use crate::infrastructure::ai::models::ModelManifest;
    use crate::services::models::ModelPhase;
    use crate::views::state::{ModelInstallState, ModelStatus};

    #[test]
    fn every_manifest_entry_has_real_copy() {
        // Arrange
        let manifest = ModelManifest::load_bundled().expect("manifest");

        // Act
        let missing: Vec<&str> = manifest
            .entries()
            .iter()
            .map(|entry| entry.id())
            .filter(|id| model_copy(id).ratings.is_none())
            .collect();

        // Assert
        assert!(missing.is_empty(), "no copy for {missing:?}");
    }

    #[test]
    fn unknown_model_falls_back_to_its_id() {
        // Arrange
        let id = "large-v9";

        // Act
        let copy = model_copy(id);

        // Assert
        assert_eq!(copy.name, "large-v9");
        assert_eq!(copy.ratings, None);
    }

    #[test]
    fn footer_follows_the_selected_model_state() {
        // Arrange
        let idle = ModelPhase::Idle;

        // Act
        let missing = model_footer(Some(("a", ModelInstallState::NotInstalled)), idle, None);
        let installed = model_footer(Some(("a", ModelInstallState::Installed)), idle, None);
        let in_use = model_footer(Some(("a", ModelInstallState::InUse)), idle, None);
        let none = model_footer(None, idle, None);

        // Assert
        assert_eq!(missing, ModelFooter::Download);
        assert_eq!(installed, ModelFooter::Use);
        assert_eq!(in_use, ModelFooter::Done);
        assert_eq!(none, ModelFooter::CloseOnly);
    }

    #[test]
    fn downloading_offers_only_cancel() {
        // Arrange
        let selected = Some(("a", ModelInstallState::NotInstalled));

        // Act
        let footer = model_footer(selected, ModelPhase::Downloading, Some("a"));

        // Assert
        assert_eq!(footer, ModelFooter::CancelDownload);
    }

    #[test]
    fn retry_is_only_offered_for_the_model_that_failed() {
        // Arrange
        let same = Some(("a", ModelInstallState::NotInstalled));
        let other = Some(("b", ModelInstallState::NotInstalled));

        // Act
        let failed_same = model_footer(same, ModelPhase::Failed, Some("a"));
        let cancelled_same = model_footer(same, ModelPhase::Cancelled, Some("a"));
        let failed_other = model_footer(other, ModelPhase::Failed, Some("a"));

        // Assert
        assert_eq!(failed_same, ModelFooter::Retry);
        assert_eq!(cancelled_same, ModelFooter::Retry);
        assert_eq!(failed_other, ModelFooter::Download);
    }

    #[test]
    fn labels_name_the_action() {
        // Arrange
        let bytes = 190_085_487;

        // Act
        let download = download_label(bytes);
        let use_model = use_label("Small");

        // Assert
        assert_eq!(download, "Download 181.3 MB");
        assert_eq!(use_model, "Use Small");
    }

    #[test]
    fn progress_text_is_marked_approximate() {
        // Arrange
        let total = 190_085_487;

        // Act
        let (left, right) = download_progress_text(42, total);

        // Assert
        assert_eq!(left, "Downloading 42%");
        assert_eq!(right, "About 76.1 MB of 181.3 MB");
    }

    #[test]
    fn installed_rows_replace_the_size() {
        // Arrange
        let bytes = 32_152_673;

        // Act
        let missing = row_meta(ModelInstallState::NotInstalled, bytes);
        let installed = row_meta(ModelInstallState::Installed, bytes);

        // Assert
        assert_eq!(missing, "30.7 MB");
        assert_eq!(installed, "On this PC");
    }

    #[test]
    fn accessibility_label_includes_the_state() {
        // Arrange
        let copy = model_copy("base-q5_1");

        // Act
        let label = row_accessibility_label(&copy, ModelInstallState::InUse);

        // Assert
        assert_eq!(label, "Base, in use, speed 2 of 3, accuracy 2 of 3");
    }

    #[test]
    fn status_text_covers_every_state() {
        // Arrange
        let in_use = ModelStatus::InUse {
            id: "base-q5_1".to_string(),
        };
        let downloading = ModelStatus::Downloading {
            id: "small-q5_1".to_string(),
            percent: 42,
        };

        // Act
        let checking = model_status_text(&ModelStatus::Checking);
        let missing = model_status_text(&ModelStatus::Missing);
        let in_use = model_status_text(&in_use);
        let downloading = model_status_text(&downloading);

        // Assert
        assert_eq!(checking, None);
        assert_eq!(missing.as_deref(), Some("No model installed · Choose…"));
        assert_eq!(in_use.as_deref(), Some("Model: Base"));
        assert_eq!(downloading.as_deref(), Some("Downloading Small 42%"));
    }

    #[test]
    fn pending_job_closes_the_dialog_and_transcribes() {
        // Arrange
        let pending = Some(7);

        // Act
        let with_job = after_model_ready(pending);
        let without_job = after_model_ready::<i32>(None);

        // Assert
        assert_eq!(with_job, AfterModelReady::CloseAndTranscribe(7));
        assert_eq!(without_job, AfterModelReady::StayOpen);
    }
}
```

</details>

### 9.5 `views/state.rs`

Apply this diff to the production code.

```diff
@@ -35,4 +35,6 @@
     model_failure: Option<&'static str>,
     model_files: Vec<ModelFile>,
+    model_files_known: bool,
+    download_target: Option<String>,
 }
 
@@ -59,4 +61,6 @@
             model_failure: None,
             model_files: Vec::new(),
+            model_files_known: false,
+            download_target: None,
         }
     }
@@ -159,4 +163,5 @@
 
     pub(crate) fn apply_model(&mut self, snapshot: ModelSnapshot) {
+        self.model_files_known = true;
         self.model_phase = snapshot.phase;
         self.model_failure = snapshot.failure;
@@ -168,4 +173,52 @@
         {
             self.model_files = snapshot.models;
+        }
+    }
+
+    pub(crate) fn download_target(&self) -> Option<&str> {
+        self.download_target.as_deref()
+    }
+
+    /// Marks the download busy right away, so a double click cannot queue a second one.
+    pub(crate) fn begin_download(&mut self, id: impl Into<String>) {
+        self.download_target = Some(id.into());
+        self.model_phase = ModelPhase::Downloading;
+        self.model_failure = None;
+        self.model_progress = Some(0);
+    }
+
+    pub(crate) fn clear_download_target(&mut self) {
+        self.download_target = None;
+    }
+
+    /// The finished model, once. A later Idle snapshot from a refresh finds nothing here.
+    pub(crate) fn take_installed_target(&mut self) -> Option<String> {
+        if self.model_phase != ModelPhase::Ready {
+            return None;
+        }
+        let target = self.download_target.take()?;
+        self.model_files
+            .iter()
+            .any(|file| file.variant_id.as_deref() == Some(target.as_str()))
+            .then_some(target)
+    }
+
+    pub(crate) fn model_status(&self, selected_id: &str) -> ModelStatus {
+        if self.model_phase == ModelPhase::Downloading {
+            if let Some(id) = &self.download_target {
+                return ModelStatus::Downloading {
+                    id: id.clone(),
+                    percent: self.model_progress.unwrap_or(0),
+                };
+            }
+        }
+        if !self.model_files_known {
+            return ModelStatus::Checking;
+        }
+        match model_install_state(selected_id, selected_id, &self.model_files) {
+            ModelInstallState::InUse => ModelStatus::InUse {
+                id: selected_id.to_string(),
+            },
+            ModelInstallState::Installed | ModelInstallState::NotInstalled => ModelStatus::Missing,
         }
     }
@@ -210,4 +263,5 @@
     }
 
+    #[cfg_attr(not(test), allow(dead_code))] // No longer shown; the bridge test still counts it.
     pub(crate) fn shell_pulses(&self) -> u64 {
         self.shell_pulses
@@ -249,26 +303,58 @@
     }
 
-    pub(crate) fn status_line(&self) -> String {
-        let shell = if self.shell_pulses() == 0 {
-            "Idle".to_string()
+    /// `elapsed` is passed in because the window owns the recording start time.
+    pub(crate) fn status_line(&self, elapsed: &str) -> String {
+        let activity = match self.record_phase {
+            RecordPhase::Starting => Some("Starting".to_string()),
+            RecordPhase::Recording => Some(format!("Recording {elapsed}")),
+            RecordPhase::Stopping => Some("Saving".to_string()),
+            RecordPhase::Idle | RecordPhase::Error => None,
+        };
+        let transcription = self
+            .transcript_status
+            .filter(|_| self.transcription_active())
+            .map(|status| status.display().to_string());
+        let parts: Vec<String> = activity
+            .into_iter()
+            .chain(self.notice.map(str::to_string))
+            .chain(transcription)
+            .collect();
+        if parts.is_empty() {
+            "Ready".to_string()
         } else {
-            format!("Shell ready ({})", self.shell_pulses)
-        };
-        let body = match self.notice {
-            Some(notice) => format!("{notice} {shell}"),
-            None => shell,
-        };
-        let base = match self.record_phase {
-            RecordPhase::Starting => format!("Starting {body}"),
-            RecordPhase::Recording => format!("Recording {body}"),
-            RecordPhase::Stopping => format!("Saving {body}"),
-            RecordPhase::Idle | RecordPhase::Error => body,
-        };
-        if self.transcription_active() {
-            if let Some(status) = self.transcript_status {
-                return format!("{base} · {}", status.display());
-            }
-        }
-        base
-    }
-}
+            parts.join(" · ")
+        }
+    }
+}
+
+/// Whether one model is usable. Selected but not installed counts as `NotInstalled`.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub(crate) enum ModelInstallState {
+    InUse,
+    Installed,
+    NotInstalled,
+}
+
+pub(crate) fn model_install_state(
+    id: &str,
+    selected_id: &str,
+    files: &[ModelFile],
+) -> ModelInstallState {
+    let installed = files
+        .iter()
+        .any(|file| file.variant_id.as_deref() == Some(id));
+    match (installed, id == selected_id) {
+        (true, true) => ModelInstallState::InUse,
+        (true, false) => ModelInstallState::Installed,
+        (false, _) => ModelInstallState::NotInstalled,
+    }
+}
+
+/// What the status bar and Settings report about the model.
+#[derive(Clone, Debug, PartialEq, Eq)]
+pub(crate) enum ModelStatus {
+    Checking,
+    InUse { id: String },
+    Missing,
+    Downloading { id: String, percent: u8 },
+}
```

The existing tests call `status_line()`. Change those calls to `status_line("00:00:05")`.

<details>
<summary>New tests for this file</summary>

```rust
// Add to the existing tests module in views/state.rs, and import
// model_install_state, ModelInstallState, ModelStatus and ModelFile.

    fn model_snapshot(phase: ModelPhase, installed: &[&str]) -> ModelSnapshot {
        ModelSnapshot {
            phase,
            progress_percent: 0,
            failure: None,
            notice: None,
            total_bytes: 0,
            models: installed
                .iter()
                .map(|id| ModelFile {
                    filename: format!("ggml-{id}.bin"),
                    variant_id: Some((*id).to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn install_state_needs_the_file_and_the_selection() {
        // Arrange
        let files = model_snapshot(ModelPhase::Idle, &["a"]).models;
        let unmapped = [ModelFile {
            filename: "ggml-x.bin".to_string(),
            variant_id: None,
        }];

        // Act
        let in_use = model_install_state("a", "a", &files);
        let installed = model_install_state("a", "b", &files);
        let missing = model_install_state("b", "b", &files);
        let no_variant = model_install_state("x", "x", &unmapped);

        // Assert
        assert_eq!(in_use, ModelInstallState::InUse);
        assert_eq!(installed, ModelInstallState::Installed);
        assert_eq!(missing, ModelInstallState::NotInstalled);
        assert_eq!(no_variant, ModelInstallState::NotInstalled);
    }

    #[test]
    fn model_list_is_unknown_until_the_first_snapshot() {
        // Arrange
        let mut state = presentation();

        // Act
        let before = state.model_status("a");
        state.apply_model(model_snapshot(ModelPhase::Idle, &[]));
        let after = state.model_status("a");

        // Assert
        assert_eq!(before, ModelStatus::Checking);
        assert_eq!(after, ModelStatus::Missing);
    }

    #[test]
    fn begin_download_is_busy_before_the_first_snapshot() {
        // Arrange
        let mut state = presentation();

        // Act
        state.begin_download("a");

        // Assert
        assert_eq!(state.model_phase(), ModelPhase::Downloading);
        assert_eq!(
            state.model_status("b"),
            ModelStatus::Downloading {
                id: "a".to_string(),
                percent: 0
            }
        );
    }

    #[test]
    fn ready_hands_over_the_target_once() {
        // Arrange
        let mut state = presentation();
        state.begin_download("a");
        state.apply_model(model_snapshot(ModelPhase::Ready, &["a"]));

        // Act
        let first = state.take_installed_target();
        let second = state.take_installed_target();

        // Assert
        assert_eq!(first.as_deref(), Some("a"));
        assert_eq!(second, None);
    }

    #[test]
    fn failure_keeps_the_target_and_hands_nothing_over() {
        // Arrange
        let mut state = presentation();
        state.begin_download("a");
        state.apply_model(model_snapshot(ModelPhase::Failed, &[]));

        // Act
        let taken = state.take_installed_target();

        // Assert
        assert_eq!(taken, None);
        assert_eq!(state.download_target(), Some("a"));
    }

    #[test]
    fn status_line_reads_naturally() {
        // Arrange
        let mut state = presentation();
        let idle = state.status_line("00:00:05");
        state.set_record_phase(RecordPhase::Recording);

        // Act
        let recording = state.status_line("00:12:41");

        // Assert
        assert_eq!(idle, "Ready");
        assert_eq!(recording, "Recording 00:12:41");
    }
```

</details>

### 9.6 `views/actions.rs`

Apply this diff. It includes the updated test expectation.

```diff
@@ -34,4 +34,7 @@
     CancelModelDownload,
     RetryModelDownload,
+    /// Make the model highlighted in the dialog the one in use.
+    UseModel,
+    OpenModelDialog,
     Show,
     Hide,
@@ -76,4 +79,6 @@
     CancelModelDownload,
     RetryModelDownload,
+    UseModel,
+    OpenModelDialog,
     /// Show the window. `recording_id` is set only when that row is still in the library.
     ShowSelect {
@@ -124,4 +129,6 @@
         AppCommand::CancelModelDownload => Effect::CancelModelDownload,
         AppCommand::RetryModelDownload => Effect::RetryModelDownload,
+        AppCommand::UseModel => Effect::UseModel,
+        AppCommand::OpenModelDialog => Effect::OpenModelDialog,
         AppCommand::Show => {
             state.set_visible(true);
@@ -405,5 +412,5 @@
         assert!(again.effects.is_empty());
         assert_eq!(state.shell_pulses(), 5);
-        assert_eq!(state.status_line(), "Shell ready (5)");
+        assert_eq!(state.status_line("00:00:05"), "Ready");
     }
```

### 9.7 `views/main_window.rs`

Apply this diff. It covers the startup refresh, the Use and download effects, the dialog handlers and the status bar.

```diff
@@ -2,4 +2,5 @@
 
 use std::path::PathBuf;
+use std::rc::Rc;
 use std::sync::mpsc::Receiver;
 use std::sync::Arc;
@@ -10,4 +11,5 @@
 use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
 use gpui_kit::component::slider::{SliderEvent, SliderState};
+use gpui_kit::component::status_bar::StatusBar;
 use gpui_kit::component::tab::{Tab, TabBar};
 use gpui_kit::component::table::{TableEvent, TableSelection, TableState};
@@ -15,5 +17,5 @@
 use gpui_kit::prelude::*;
 use gpui_kit::{
-    div, px, App, Context, Entity, FocusHandle, IntoElement, ParentElement, Render, SharedString,
+    div, px, Context, Entity, FocusHandle, IntoElement, ParentElement, Render, SharedString,
     Styled, Subscription, Task, Window,
 };
@@ -33,5 +35,5 @@
 use crate::services::level_monitor::MonitorTargets;
 use crate::services::library::{LibraryHost, LibrarySnapshot};
-use crate::services::models::{ModelHost, ModelSnapshot};
+use crate::services::models::{ModelHost, ModelPhase, ModelSnapshot};
 use crate::services::playback::{PlayerHost, PlayerPhase, PlayerSnapshot};
 use crate::services::ports::DesktopShell;
@@ -41,12 +43,14 @@
 };
 use crate::services::transcription::{JobPhase, TranscriptionHost, TranscriptionSnapshot};
+use crate::ui::components::model_card::ModelCardData;
 use crate::ui::theme::{apply_theme, theme_icon};
 use crate::views::actions::{drain_bridge, AppCommand, BridgeMessage, Effect};
 use crate::views::hooks::ShellHooks;
-use crate::views::state::{AppTab, Presentation};
+use crate::views::state::{model_install_state, AppTab, Presentation};
 
 use super::model_prompt::{
-    catalog_row_label, open_model_alert, open_model_prompt, prompt_can_dismiss,
-    selected_model_line, CatalogRow, ModelPromptView,
+    after_model_ready, card_progress, model_copy, model_footer, model_status_item,
+    open_model_alert, open_model_prompt, row_accessibility_label, row_meta, row_tag,
+    AfterModelReady, ModelFooter, ModelPromptHandlers, ModelPromptView,
 };
 use super::recording::record_tab;
@@ -57,4 +61,5 @@
 use super::settings::{
     open_about, open_exit_while_recording, open_settings, open_tray_explanation,
+    settings_model_row, SettingsView,
 };
 
@@ -84,4 +89,6 @@
     prompt_model_id: String,
     model_prompt_open: bool,
+    /// Set only when Transcribe found no model. Starts once a model is ready.
+    pending_transcription: Option<RecordingId>,
     recordings_table: Entity<TableState<RecordingsDelegate>>,
     _table_sub: Subscription,
@@ -235,4 +242,6 @@
         let transcript_editor = cx.new(|cx| EditorState::new(window, cx).line_number(false));
         let prompt_model_id = settings.selected_model().to_string();
+        // The status bar and Settings need the installed list before the dialog ever opens.
+        model_host.refresh();
         let appearance_entity = cx.weak_entity();
         let appearance = window.observe_window_appearance(move |window, cx| {
@@ -274,4 +283,5 @@
             prompt_model_id,
             model_prompt_open: false,
+            pending_transcription: None,
             recordings_table,
             _table_sub: table_sub,
@@ -453,13 +463,6 @@
                 }
             }
-            Effect::OpenSettings => {
-                let session = cx.entity();
-                let watched = session.clone();
-                let on_change = move |checked: &bool, _: &mut Window, cx: &mut App| {
-                    session.update(cx, |view, cx| view.set_close_to_tray(*checked, cx));
-                };
-                let checked = move |cx: &App| watched.read_with(cx, |view, _| view.close_to_tray());
-                open_settings(window, cx, checked, on_change);
-            }
+            Effect::OpenSettings => self.open_settings_dialog(window, cx),
+            Effect::OpenModelDialog => self.open_model_prompt(window, cx),
             Effect::OpenAbout => open_about(window, cx),
             Effect::StartRecording => self.begin_recording(),
@@ -483,7 +486,22 @@
             }
             Effect::Quit => self.quit_now(window),
-            Effect::DownloadModel => self.model_host.download(self.prompt_model_id.clone()),
+            Effect::DownloadModel => {
+                self.presentation
+                    .begin_download(self.prompt_model_id.clone());
+                self.model_host.download(self.prompt_model_id.clone());
+            }
             Effect::CancelModelDownload => self.model_host.cancel(),
-            Effect::RetryModelDownload => self.model_host.retry(),
+            Effect::RetryModelDownload => {
+                // The footer only offers Retry for the model the worker last tried.
+                self.presentation
+                    .begin_download(self.prompt_model_id.clone());
+                self.model_host.retry();
+            }
+            Effect::UseModel => {
+                self.settings
+                    .set_selected_model(self.prompt_model_id.clone());
+                self.hooks.save_settings(self.settings.clone());
+                self.finish_model_choice(window, cx);
+            }
         }
         cx.notify();
@@ -661,16 +679,38 @@
         let previous = self.presentation.model_failure();
         self.presentation.apply_model(snapshot);
-        if self.model_prompt_open
-            && prompt_can_dismiss(self.presentation.model_phase())
-            && window.has_active_dialog(cx)
-        {
-            self.model_prompt_open = false;
-            window.close_dialog(cx);
-        }
-        if let Some(message) = self.presentation.model_failure() {
-            if previous != Some(message) && is_distinct_model_message(message) {
-                open_model_alert(window, cx, message);
-            }
-        }
+        if let Some(id) = self.presentation.take_installed_target() {
+            self.settings.set_selected_model(id.clone());
+            self.hooks.save_settings(self.settings.clone());
+            self.prompt_model_id = id;
+            self.finish_model_choice(window, cx);
+        }
+        // An open dialog shows the failure in the card, so no second popup.
+        if !self.model_prompt_open {
+            if let Some(message) = self.presentation.model_failure() {
+                if previous != Some(message) && is_distinct_model_message(message) {
+                    open_model_alert(window, cx, message);
+                }
+            }
+        }
+    }
+
+    fn finish_model_choice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
+        match after_model_ready(self.pending_transcription.take()) {
+            AfterModelReady::CloseAndTranscribe(recording_id) => {
+                if self.model_prompt_open {
+                    self.model_prompt_open = false;
+                    window.close_dialog(cx);
+                }
+                self.begin_transcription(recording_id, window, cx);
+            }
+            AfterModelReady::StayOpen => {}
+        }
+    }
+
+    /// Runs when the user dismisses the dialog. Abandons any Transcribe that was waiting.
+    fn model_prompt_closed(&mut self) {
+        self.model_prompt_open = false;
+        self.pending_transcription = None;
+        self.presentation.clear_download_target();
     }
 
@@ -703,4 +743,7 @@
             Err(error) => {
                 tracing::error!(error = %error, "transcription model was not resolved");
+                if error.user_message() == MODEL_MISSING_MESSAGE {
+                    self.pending_transcription = Some(recording_id);
+                }
                 self.report_model_error(error.user_message(), window, cx);
             }
@@ -714,5 +757,5 @@
         cx: &mut Context<Self>,
     ) {
-        if message == "The transcription model file is missing." {
+        if message == MODEL_MISSING_MESSAGE {
             self.open_model_prompt(window, cx);
             return;
@@ -798,95 +841,161 @@
     }
 
+    fn open_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
+        let read_entity = cx.entity();
+        let tray_entity = read_entity.clone();
+        let model_entity = read_entity.clone();
+        open_settings(
+            window,
+            cx,
+            move |cx| read_entity.read(cx).settings_view(),
+            move |checked, _window, cx| {
+                tray_entity.update(cx, |view, cx| view.set_close_to_tray(*checked, cx));
+            },
+            move |_event, window, cx| {
+                model_entity.update(cx, |view, cx| {
+                    view.handle(AppCommand::OpenModelDialog, window, cx);
+                });
+            },
+        );
+    }
+
+    fn settings_view(&self) -> SettingsView {
+        let selected = self.settings.selected_model();
+        let bytes = ModelManifest::load_bundled()
+            .ok()
+            .and_then(|manifest| manifest.by_id(selected).map(|entry| entry.bytes()));
+        SettingsView {
+            close_to_tray: self.close_to_tray(),
+            model: settings_model_row(&self.presentation.model_status(selected), bytes),
+        }
+    }
+
     fn open_model_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
+        // The tray can open Settings over this dialog, so its button could ask twice.
+        if self.model_prompt_open {
+            return;
+        }
         self.model_host.refresh();
         self.model_prompt_open = true;
-        self.ensure_prompt_selection();
+        self.reset_prompt_selection();
         let read_entity = cx.entity();
         let select_entity = read_entity.clone();
         let download_entity = read_entity.clone();
+        let use_entity = read_entity.clone();
         let cancel_entity = read_entity.clone();
         let retry_entity = read_entity.clone();
+        let closed_entity = read_entity.clone();
         open_model_prompt(
             window,
             cx,
             move |cx| read_entity.read(cx).model_prompt_view(),
-            move |id, _window, cx| {
-                select_entity.update(cx, |view, cx| {
-                    view.select_prompt_model(id);
-                    cx.notify();
-                });
-            },
-            move |_event, window, cx| {
-                download_entity.update(cx, |view, cx| {
-                    view.handle(AppCommand::DownloadModel, window, cx);
-                });
-            },
-            move |_event, window, cx| {
-                cancel_entity.update(cx, |view, cx| {
-                    view.handle(AppCommand::CancelModelDownload, window, cx);
-                });
-            },
-            move |_event, window, cx| {
-                retry_entity.update(cx, |view, cx| {
-                    view.handle(AppCommand::RetryModelDownload, window, cx);
-                });
+            ModelPromptHandlers {
+                on_select: Rc::new(move |id, _window, cx| {
+                    select_entity.update(cx, |view, cx| {
+                        view.select_prompt_model(id);
+                        cx.notify();
+                    });
+                }),
+                on_download: Rc::new(move |_event, window, cx| {
+                    download_entity.update(cx, |view, cx| {
+                        view.handle(AppCommand::DownloadModel, window, cx);
+                    });
+                }),
+                on_use: Rc::new(move |_event, window, cx| {
+                    use_entity.update(cx, |view, cx| {
+                        view.handle(AppCommand::UseModel, window, cx);
+                    });
+                }),
+                on_cancel: Rc::new(move |_event, window, cx| {
+                    cancel_entity.update(cx, |view, cx| {
+                        view.handle(AppCommand::CancelModelDownload, window, cx);
+                    });
+                }),
+                on_retry: Rc::new(move |_event, window, cx| {
+                    retry_entity.update(cx, |view, cx| {
+                        view.handle(AppCommand::RetryModelDownload, window, cx);
+                    });
+                }),
+                on_closed: Rc::new(move |_event, _window, cx| {
+                    closed_entity.update(cx, |view, cx| {
+                        view.model_prompt_closed();
+                        cx.notify();
+                    });
+                }),
             },
         );
     }
 
-    fn ensure_prompt_selection(&mut self) {
+    /// Each opening starts on the saved model, never on the last highlight.
+    fn reset_prompt_selection(&mut self) {
         let Ok(manifest) = ModelManifest::load_bundled() else {
             return;
         };
-        if manifest.by_id(&self.prompt_model_id).is_some() {
-            return;
-        }
-        if manifest.by_id(self.settings.selected_model()).is_some() {
-            self.prompt_model_id = self.settings.selected_model().to_string();
-            return;
-        }
-        if let Some(entry) = manifest.default_entry() {
+        let entry = manifest
+            .by_id(self.settings.selected_model())
+            .or_else(|| manifest.default_entry());
+        if let Some(entry) = entry {
             self.prompt_model_id = entry.id().to_string();
         }
     }
 
+    /// Highlights only. The saved model changes on Use or after a finished download.
     fn select_prompt_model(&mut self, id: &str) {
         self.prompt_model_id = id.to_string();
-        self.settings.set_selected_model(id);
-        self.hooks.save_settings(self.settings.clone());
     }
 
     fn model_prompt_view(&self) -> ModelPromptView {
-        let progress = self.presentation.model_progress();
+        let phase = self.presentation.model_phase();
+        let busy = phase == ModelPhase::Downloading;
+        let target = self.presentation.download_target();
         let failure = self.presentation.model_failure();
-        let phase = self.presentation.model_phase();
+        let percent = self.presentation.model_progress().unwrap_or(0);
+        let saved = self.settings.selected_model();
+        let files = self.presentation.model_files();
         let Ok(manifest) = ModelManifest::load_bundled() else {
             return ModelPromptView {
-                catalog: Vec::new(),
-                selected_id: None,
-                selected_line: SharedString::from(selected_model_line(None, None)),
-                progress,
-                failure,
-                phase,
+                cards: Vec::new(),
+                footer: ModelFooter::CloseOnly,
+                selected_bytes: None,
+                selected_name: SharedString::default(),
+                busy,
             };
         };
+        let cards = manifest
+            .entries()
+            .iter()
+            .map(|entry| {
+                let id = entry.id();
+                let copy = model_copy(id);
+                let state = model_install_state(id, saved, files);
+                let is_target = target == Some(id);
+                ModelCardData {
+                    id: SharedString::from(id.to_string()),
+                    name: SharedString::from(copy.name.to_string()),
+                    description: SharedString::from(copy.description.to_string()),
+                    meta: row_meta(state, entry.bytes()).into(),
+                    tag: row_tag(state),
+                    ratings: copy.ratings,
+                    selected: id == self.prompt_model_id,
+                    disabled: busy,
+                    progress: (busy && is_target).then(|| card_progress(percent, entry.bytes())),
+                    failure: failure.filter(|_| is_target).map(SharedString::from),
+                    accessibility_label: row_accessibility_label(&copy, state).into(),
+                }
+            })
+            .collect();
         let selected = manifest.by_id(&self.prompt_model_id);
         ModelPromptView {
-            catalog: manifest
-                .entries()
-                .iter()
-                .map(|entry| CatalogRow {
-                    id: SharedString::from(entry.id()),
-                    label: SharedString::from(catalog_row_label(entry.id(), entry.bytes())),
-                })
-                .collect(),
-            selected_id: selected.map(|entry| SharedString::from(entry.id())),
-            selected_line: SharedString::from(selected_model_line(
-                selected.map(|entry| entry.id()),
-                selected.map(|entry| entry.bytes()),
-            )),
-            progress,
-            failure,
-            phase,
+            cards,
+            footer: model_footer(
+                selected.map(|entry| (entry.id(), model_install_state(entry.id(), saved, files))),
+                phase,
+                target,
+            ),
+            selected_bytes: selected.map(|entry| entry.bytes()),
+            selected_name: selected
+                .map(|entry| SharedString::from(model_copy(entry.id()).name.to_string()))
+                .unwrap_or_default(),
+            busy,
         }
     }
@@ -1187,4 +1296,6 @@
 }
 
+const MODEL_MISSING_MESSAGE: &str = "The transcription model file is missing.";
+
 fn is_distinct_model_message(message: &str) -> bool {
     matches!(
@@ -1276,4 +1387,5 @@
                 .unwrap_or(Duration::ZERO),
         );
+        let status_elapsed = elapsed.clone();
         let phase_label = match phase {
             RecordPhase::Idle => "Ready",
@@ -1436,5 +1548,18 @@
             }
         };
-        let status = SharedString::from(self.presentation.status_line());
+        let status = SharedString::from(self.presentation.status_line(&status_elapsed));
+        let model_status = self
+            .presentation
+            .model_status(self.settings.selected_model());
+        let status_entity = entity.clone();
+        let model_item = model_status_item(
+            &model_status,
+            move |_event, window, cx| {
+                status_entity.update(cx, |view, cx| {
+                    view.handle(AppCommand::OpenModelDialog, window, cx);
+                });
+            },
+            cx,
+        );
         let settings_entity = entity.clone();
         let theme_entity = entity.clone();
@@ -1501,13 +1626,11 @@
             )
             .child(
-                h_flex()
+                // With only a right region, the center is start-aligned and takes the free
+                // space, so long status text truncates before the model item moves.
+                StatusBar::new()
                     .h(px(28.))
                     .px_3()
-                    .items_center()
-                    .border_t_1()
-                    .border_color(cx.theme().border)
-                    .text_sm()
-                    .text_color(cx.theme().muted_foreground)
-                    .child(status),
+                    .child(div().min_w_0().truncate().child(status))
+                    .when_some(model_item, |bar, item| bar.right(item)),
             )
     }
```

### 9.8 `views/settings.rs`

Replace the whole file.

```rust
//! Settings: the transcription model and app behaviour. Rendering never touches a worker.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogClose;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, ClickEvent, IntoElement, ParentElement, SharedString, Styled, Window};

use crate::domain::recording::format_size;
use crate::ui::components::dialog_style::{dialog_footer, dialog_footer_note, DIALOG_WIDTH};
use crate::ui::components::model_card::{model_tag, ModelCardTag};
use crate::ui::components::settings_rows::{settings_row, settings_section};

use super::model_prompt::model_copy;
use super::state::ModelStatus;

/// Text for the Transcription row. Pure, so each model state can be tested without GPUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsModelRow {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) button: &'static str,
    pub(crate) in_use: bool,
}

pub(crate) fn settings_model_row(
    status: &ModelStatus,
    model_bytes: Option<u64>,
) -> SettingsModelRow {
    match status {
        ModelStatus::InUse { id } => {
            let copy = model_copy(id);
            let description = match model_bytes {
                Some(bytes) => format!("{} · {}", copy.description, format_size(bytes)),
                None => copy.description.to_string(),
            };
            SettingsModelRow {
                title: copy.name.to_string(),
                description,
                button: "Change model…",
                in_use: true,
            }
        }
        ModelStatus::Downloading { id, percent } => SettingsModelRow {
            title: model_copy(id).name.to_string(),
            description: format!("Downloading {percent}%"),
            button: "Change model…",
            in_use: false,
        },
        ModelStatus::Missing | ModelStatus::Checking => SettingsModelRow {
            title: "No model installed".to_string(),
            description: "Choose a model to create transcripts.".to_string(),
            button: "Choose model…",
            in_use: false,
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsView {
    pub(crate) close_to_tray: bool,
    pub(crate) model: SettingsModelRow,
}

pub(crate) fn version_text() -> String {
    format!("Skrivli {}", env!("CARGO_PKG_VERSION"))
}

pub(crate) fn open_settings(
    window: &mut Window,
    cx: &mut App,
    read: impl Fn(&App) -> SettingsView + 'static,
    on_close_to_tray: impl Fn(&bool, &mut Window, &mut App) + Clone + 'static,
    on_change_model: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = read(cx);
        let model = view.model;
        let model_title = h_flex()
            .gap_2()
            .items_center()
            .child(SharedString::from(model.title))
            .when(model.in_use, |this| {
                this.child(model_tag(ModelCardTag::InUse))
            });
        dialog
            .title("Settings")
            .w(DIALOG_WIDTH)
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .child(settings_section(
                        "Transcription",
                        [settings_row(
                            model_title,
                            model.description,
                            Button::new("settings-change-model")
                                .outline()
                                .small()
                                .label(model.button)
                                .on_click(on_change_model.clone()),
                            cx,
                        )],
                        cx,
                    ))
                    .child(settings_section(
                        "General",
                        [settings_row(
                            "Close to the notification area",
                            "Skrivli keeps running when you close the window.",
                            close_to_tray_switch(view.close_to_tray, on_close_to_tray.clone()),
                            cx,
                        )],
                        cx,
                    )),
            )
            .footer(
                dialog_footer(cx)
                    .child(dialog_footer_note(version_text(), cx))
                    .child(
                        DialogClose::new()
                            .child(Button::new("settings-done").label("Done").primary()),
                    ),
            )
    });
}

fn close_to_tray_switch(
    checked: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    Switch::new("close-to-tray")
        .checked(checked)
        .accessibility_label("Close to the notification area")
        .on_change(on_change)
}

/// Reached only through the reserved `ShowAbout` action. Settings shows the version itself.
pub(crate) fn open_about(window: &mut Window, cx: &mut App) {
    let version = SharedString::from(format!("Version {}", env!("CARGO_PKG_VERSION")));
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert.title("Skrivli").description(version.clone())
    });
}

pub(crate) fn open_tray_explanation(window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, |alert, _, _| {
        alert
            .title("Skrivli is still running")
            .description("Skrivli stays in the notification area. Use Exit to quit.")
    });
}

pub(crate) fn open_exit_while_recording(
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(&mut Window, &mut App) + Clone + 'static,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_confirm = on_confirm.clone();
        alert
            .title("Stop and save?")
            .description("A recording is in progress. Exit will stop and save it first.")
            .confirm()
            .ok_text("Stop and exit")
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            })
    });
}
```

<details>
<summary>Tests for this file</summary>

```rust
#[cfg(test)]
mod tests {
    use super::{settings_model_row, version_text, SettingsModelRow};
    use crate::views::state::ModelStatus;

    #[test]
    fn row_shows_the_model_in_use_with_its_size() {
        // Arrange
        let status = ModelStatus::InUse {
            id: "base-q5_1".to_string(),
        };

        // Act
        let row = settings_model_row(&status, Some(59_707_625));

        // Assert
        assert_eq!(
            row,
            SettingsModelRow {
                title: "Base".to_string(),
                description: "Good balance for most meetings · 56.9 MB".to_string(),
                button: "Change model…",
                in_use: true,
            }
        );
    }

    #[test]
    fn row_offers_a_choice_when_nothing_is_installed() {
        // Arrange
        let status = ModelStatus::Missing;

        // Act
        let row = settings_model_row(&status, None);

        // Assert
        assert_eq!(row.title, "No model installed");
        assert_eq!(row.button, "Choose model…");
        assert!(!row.in_use);
    }

    #[test]
    fn version_text_names_the_app() {
        // Arrange

        // Act
        let text = version_text();

        // Assert
        assert!(text.starts_with("Skrivli "));
    }
}
```

</details>
