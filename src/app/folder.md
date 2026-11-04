# app

Process lifetime only. No windows, recording, or transcription.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `Application.hpp`, `Application.cpp` | Single-instance mutex, activate an existing window, and run `MainWindow`. Honors a `--tray` start. | Process host | Process | Single instance, Autostart, Shell UI |
