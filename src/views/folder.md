# views

Windows and dialogs. They compose `ui/` controls and call `services/`. They do not own WASAPI or whisper handles.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `MainWindow.hpp`, `MainWindow.cpp` | Frame, tabs, status bar, tray menu, hotkey, and `WM_APP_*` routing. Hide-to-tray and end-session save. | Frame window | UI | Shell UI, Tray, Recording, Transcription, Hotkey, Device change, Low disk |
| `RecordView.hpp`, `RecordView.cpp` | Record tab: devices, save folder, gains, Record/Stop, elapsed timer, meters. | Tab page | UI | Recording, Level meters, Device selection |
| `RecordingsView.hpp`, `RecordingsView.cpp` | Recordings list and Play, Transcribe, View, Open folder, Rename, Delete, Refresh. | Tab page | UI | Recordings library, Transcription, M4A export |
| `TranscriptWindow.hpp`, `TranscriptWindow.cpp` | Read-only RichEdit of the Markdown source. Copy, Save as, Open in editor, Find. | Frame window | UI | Transcript view |
| `SettingsDialog.hpp`, `SettingsDialog.cpp` | Settings: folder, devices, gains, M4A, model, auto-transcribe, hotkey, start with Windows. | Dialog | UI | Settings, Autostart, Hotkey, M4A export, Auto-transcribe |
| `ModelPrompt.hpp`, `ModelPrompt.cpp` | Progress window and file picker that download or install a whisper model before enqueue. | Dialog helper | UI | Model download, Transcription |
