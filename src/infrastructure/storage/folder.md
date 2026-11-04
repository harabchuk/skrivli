# infrastructure/storage

Settings and recording metadata on disk. `tests/` is not indexed.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `Settings.hpp`, `Settings.cpp` | Loads and saves `%APPDATA%\Skrivli\settings.json` (Glaze). | Settings store | Storage | Settings, Recording, Transcription, Autostart, Hotkey |
| `RecordingLibrary.hpp`, `RecordingLibrary.cpp` | Scans a folder, repairs leftover `.partial` files, and renames or recycles audio plus `.txt` plus `.json` (and leftover `.md`) as one unit. | Library | Storage | Recordings library, Crash recovery |
| `RecordingSidecar.hpp`, `RecordingSidecar.cpp` | Loads and saves the `.json` sidecar and builds one from a finished WAV. | JSON I/O | Storage | Recordings library, Transcription |
| `DirectoryWatcher.hpp`, `DirectoryWatcher.cpp` | `ReadDirectoryChangesW` thread that posts one coalesced message. | Folder watch | Storage | Recordings library |
