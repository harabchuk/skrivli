# infrastructure

OS and library adapters. No WTL windows. Each subfolder has its own `folder.md`. Shared helpers live in `common/`.

Update this file when a subfolder is added or its domain changes.

| Folder | Domain |
|---|---|
| `audio/` | WASAPI capture, loopback keep-alive, mix, resample |
| `ai/` | whisper.cpp and model files |
| `storage/` | Settings, recording library, sidecar JSON, folder watch |
| `files/` | WAV and M4A encode, audio decode |
| `system/` | Autostart, hotkey, clipboard |
| `common/` | Log, paths, text, file bytes, RAII helpers |
