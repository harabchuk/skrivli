# infrastructure/system

Windows shell features that are not audio or files.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `Autostart.hpp`, `Autostart.cpp` | Creates or removes the HKCU Run entry. `--tray` when start-minimized is on. | Registry | System | Autostart |
| `Hotkey.hpp`, `Hotkey.cpp` | Parses a chord such as `Ctrl+Alt+R` and registers it with `RegisterHotKey`. | Hotkey | System | Hotkey |
| `Clipboard.hpp`, `Clipboard.cpp` | Puts plain Unicode text on the clipboard. | Clipboard | System | Transcript view |
