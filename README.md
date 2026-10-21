# Siphon

Download audio from your browser and drag it into your DAW. Siphon pairs a Helium extension with a local Windows host powered by yt-dlp and ffmpeg.

- Download buttons on YouTube, YouTube Music, SoundCloud, and Bandcamp.
- MP3, M4A, Opus, Ogg, FLAC, WAV, or the original audio stream.
- Local BPM/key analysis, metadata, cover art, playlists, and download history.
- Optional leading-silence removal and WAV tempo metadata for DAW workflows.
- A native drag tray for moving finished files into FL Studio or other apps.
- Light/dark glass interface with accents matching the source service.

## Install

**[Download Siphon for Windows](https://github.com/Senni360/siphon/releases/latest/download/Siphon-Windows.zip)**

Tested on Windows 11 with Helium. Install Python 3.13 and Node.js LTS first, and make sure they are available on your PATH. The installer sets up the Python dependencies and offers to install ffmpeg if it is missing. Installation needs an internet connection.

1. Extract the ZIP to a folder you intend to keep, such as `Documents\Siphon`.
2. Double-click **install.cmd** and follow the prompts. No administrator terminal is needed.
3. Open **helium://extensions** (or **chrome://extensions**) in Helium.
4. Enable **Developer mode**, choose **Load unpacked**, and select the extracted **extension** folder.
5. Pin Siphon to the toolbar and open a supported music or video page.

The browser extension needs the local host to download files. The host is installed in `%LOCALAPPDATA%\Siphon`; keep the unpacked extension folder in place. This is a manual installation, not a browser-store listing.

## Use

Click the Siphon button on a supported page, or open the toolbar popup, choose a format, and click **Download audio**. **Alt+Shift+D** downloads the current tab with your defaults; **Alt+Shift+S** opens the popup.

Files go to `%USERPROFILE%\Music\Siphon` by default. Change the folder, silence trimming, analysis, and filename options in Settings. Finished files appear in the drag tray and download history.

For a clean start in a DAW, enable **Trim silence at the start** and use WAV or FLAC. Tempo and key estimates can be imperfect; removing silence does not guarantee that the first sound is a musical downbeat. Converting a lossy source to WAV or FLAC does not restore lost detail.

If a download needs sign-in, use the cookie options while signed in to that site. Cookies go to the local host. Downloads and audio processing run on your computer.

## Update or remove

For updates, extract the new release into your existing Siphon folder, run **install.cmd** again, then click **Reload** on Siphon's extensions-page entry and refresh any open music tabs. The download engine can also be updated from **Settings → Advanced**.

To uninstall, remove the browser extension and run **uninstall.cmd**. Downloaded audio is kept.

If the host is missing, rerun the installer and reload the extension. If conversion fails, check ffmpeg in **Settings → Connection & setup**. For YouTube challenges, install Node.js LTS and rerun the installer. Local diagnostics are in `%LOCALAPPDATA%\Siphon\logs`.

## Edit and contribute

Fork this repository, make your changes, and open a pull request. The extension is plain HTML/CSS/JavaScript with no build step; the native host is Python.

```powershell
git clone https://github.com/Senni360/siphon.git
cd siphon
powershell -NoProfile -ExecutionPolicy Bypass -File .\installer\install.ps1 -Dev -Yes
npm --prefix extension test
host\.venv\Scripts\python.exe -m pip install pytest
host\.venv\Scripts\python.exe -m pytest host\tests
powershell -NoProfile -ExecutionPolicy Bypass -File .\installer\tests\Run-Tests.ps1
```

`-Dev` runs the host from your checkout. Reload the extension after browser-code changes and reconnect the host after Python changes. Optional browser checks use `npm --prefix tests ci` followed by `npm --prefix tests run sites`; they open YouTube in headless Helium without starting a download.

Download content you own or have permission to use. Siphon does not bypass DRM. Bundled Geist fonts retain their SIL Open Font License files.
