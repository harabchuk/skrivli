# infrastructure/common

Small helpers any layer may use. `tests/` is not indexed.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `Log.hpp`, `Log.cpp` | Appends Info, Warn, and Error lines under `%LOCALAPPDATA%\Skrivli\logs\`. | Logger | Common | Logging |
| `HResult.hpp`, `HResult.cpp` | Logs a failed `HRESULT` and returns whether it succeeded. | Error helper | Common | Recording, Device selection |
| `Paths.hpp`, `Paths.cpp` | Settings, models, logs, rescue, and default recordings directories. Creates them. | Paths | Common | Settings, Model download, Logging, Crash recovery |
| `Text.hpp`, `Text.cpp` | UTF-8 conversion, path compare, and `hh:mm:ss` formatting. | Text | Common | Recording, Recordings library, Transcription |
| `FileIo.hpp`, `FileIo.cpp` | Read and write bytes, atomic replace, file size, and a writable-folder check. | File bytes | Common | Settings, Transcription, Recordings library |
| `Scoped.hpp` | RAII for COM init, MMCSS (`Audio` task), and Media Foundation startup. | RAII | Common | Recording, Transcription, M4A export |
| `PostOwned.hpp` | `PostMessage` of a heap payload. The receiver takes ownership with `TakeOwned`. | Message helper | Common | Recording, Transcription |
