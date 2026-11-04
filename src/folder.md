# src

Process-wide files every layer includes. Do not add new types here. Subfolders have their own `folder.md`. `tests/` folders are not indexed.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `main.cpp` | Process entry: COM STA, common controls, WTL module, then `Application::Run`. | Startup | Process | Single instance, Shell UI |
| `pch.hpp` | Precompiled ATL/WTL and shell headers. Defines `UNICODE` and `_Module`. | Build header | Process | Shell UI |
| `Ids.hpp` | `WM_APP_*` messages, status-bar pane IDs, and timer IDs. | Message IDs | Process | Recording, Tray, Transcription, Device change, Low disk |
| `Version.hpp` | `kAppName` and `kAppVersion`. | Constants | Process | Shell UI, Settings |
