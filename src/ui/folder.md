# ui

Reusable Win32 controls. They do not name a recording or a transcription job.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `TabCtrl.hpp`, `TabCtrl.cpp` | Owner-drawn tab strip. The selected tab is open along the bottom. | Control | UI | Shell UI |
| `DeviceCombo.hpp`, `DeviceCombo.cpp` | Combo whose rows keep a device id next to the label. | Control | UI | Device selection, Settings |
| `LevelMeter.hpp`, `LevelMeter.cpp` | dB meter with segmented fill and peak hold. Input is linear 0..1. | Control | UI | Level meters |
| `RecordButton.hpp`, `RecordButton.cpp` | Owner-drawn Record/Stop pill. | Control | UI | Recording |
| `TrayIcon.hpp`, `TrayIcon.cpp` | `Shell_NotifyIconW` v4: idle and recording icons, tip, balloon, re-add. | Shell wrapper | UI | Tray |
| `ShellDialogs.hpp`, `ShellDialogs.cpp` | Folder, open-file, and save-file pickers (`IFileOpenDialog` / `IFileSaveDialog`). | Shell wrapper | UI | Recording, Transcript view, Model download |
| `Theme.hpp` | DPI scale, message box titled with the app name, and shared colors. | Palette | UI | Shell UI, Settings |
