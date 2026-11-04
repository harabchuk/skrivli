# infrastructure/ai

whisper.cpp and model files on disk. No `HWND`.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `WhisperEngine.hpp`, `WhisperEngine.cpp` | Loads a ggml model and runs one greedy transcription with progress and cancel. | whisper.cpp adapter | Transcription | Transcription |
| `ModelManager.hpp`, `ModelManager.cpp` | Model catalog, paths, SHA-256 check, and the WinHTTP download. | Model files | Transcription | Model download |
