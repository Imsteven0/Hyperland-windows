## Install

- **Installer** (recommended): download `caelestia-win-{VERSION}-setup.exe` and run it. No admin rights needed; it installs per user and starts with Windows (you can turn that off in Settings › Panels › Startup).
- **Portable**: unzip `caelestia-win-{VERSION}-x64.zip` anywhere and run `caelestia.exe`.
- **Scoop**: `scoop install https://github.com/{REPO}/releases/download/v{VERSION}/caelestia.json`

Already installed? Just run the new installer: it closes the running shell and keeps your dots (`~\.config\caelestia`).

<!-- unsigned -->
### "Windows protected your PC"

These files are not code-signed yet, so Microsoft Defender SmartScreen may warn the first time you run them. Click **More info → Run anyway**. You can check that a download is genuine with `SHA256SUMS.txt`:

```powershell
Get-FileHash .\caelestia-win-{VERSION}-setup.exe -Algorithm SHA256
```
<!-- /unsigned -->

See the README for shortcuts, komorebi tiling and configuration.
