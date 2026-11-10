Skrivli - record and transcribe meetings on your PC
===================================================

Install
  1. Unzip the whole folder anywhere you like (spaces and non-ASCII
     characters in the path are fine).
  2. Run skrivli.exe. There is no installer. To remove Skrivli, delete the
     folder; see "Where your data lives" for what it leaves behind.

Windows SmartScreen
  skrivli.exe is not code-signed yet. Windows may say it protected your PC.
  Choose "More info", then "Run anyway". You can compare the SHA-256 of the
  ZIP with the .sha256 file published next to it.

Where your data lives
  Recordings and transcripts:  %USERPROFILE%\Documents\Skrivli
  Settings:                    %APPDATA%\Skrivli\settings.json
  Models:                      %LOCALAPPDATA%\Skrivli\models
  Logs:                        %LOCALAPPDATA%\Skrivli\logs
  Crash-recovery files:        %LOCALAPPDATA%\Skrivli\rescue
  Logs never contain audio or transcript text. You can delete any of these
  folders; recordings are only removed when you delete them.

Transcription models
  Models are not in the ZIP. Skrivli shows the size and asks before it
  downloads one, and only downloads when you choose to. Once a model is
  installed, transcription runs offline on your PC. Notices for the models
  (Nemotron 3.5 and Parakeet TDT v3) are in the licenses folder.

More
  PRIVACY.txt           what Skrivli does and does not do with your data
  LICENSE.txt           the license Skrivli is distributed under
  licenses\             notices for third-party components
