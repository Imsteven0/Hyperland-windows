# Releasing

1. Bump `version` in `Cargo.toml` (and run `cargo build` so `Cargo.lock` follows).
2. Commit, then tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. The **Release** workflow tests, builds and publishes:
   - `caelestia-win-<v>-setup.exe`: per-user installer (Inno Setup, `installer.iss`),
   - `caelestia-win-<v>-x64.zip`: portable build,
   - `caelestia.json`: Scoop manifest with the zip's hash (`scoop install <its URL>`),
   - `SHA256SUMS.txt`.

The running shells see the new version within a day (Settings › About).

Build the installer locally with [Inno Setup 6](https://jrsoftware.org/isinfo.php):

```powershell
cargo build --release
iscc /DAppVersion=0.2.0 packaging\installer.iss   # -> dist\caelestia-win-0.2.0-setup.exe
```

## Code signing (SmartScreen)

Unsigned downloads trigger SmartScreen's "Windows protected your PC" until they
build reputation; the release notes explain "More info → Run anyway" while the
files are unsigned. Options to sign:

- **[SignPath Foundation](https://signpath.org/)**: free certificates for open
  source projects (apply with the repository; signing runs in their service).
- **[Azure Trusted Signing](https://learn.microsoft.com/azure/trusted-signing/)**:
  a few dollars a month, no hardware token; replace the `signtool` steps with
  the `azure/trusted-signing-action`.
- **Your own certificate** (OV/EV): export it as `.pfx` and add two repository
  secrets: `SIGN_CERT_PFX` (`[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`)
  and `SIGN_CERT_PASSWORD`. The workflow then signs `caelestia.exe` and the
  installer with SHA-256 and a timestamp, and drops the SmartScreen note.

## winget

The first submission is manual, later ones are automatic:

1. After the first release: `wingetcreate new https://github.com/Imsteven0/Hyperland-windows/releases/download/v<v>/caelestia-win-<v>-setup.exe`
   (package id e.g. `Imsteven0.Caelestia`; installer type `inno`, scope `user`) and submit the PR.
2. Once merged, add the repository **variable** `WINGET_ID` (the package id)
   and the **secret** `WINGET_TOKEN` (a classic GitHub token with `public_repo`).
   Every release then opens the update PR to `microsoft/winget-pkgs` by itself.

## Scoop

Each release's `caelestia.json` installs directly from its URL. To list the app
in a bucket, copy `scoop/caelestia.json` there: `checkver` and `autoupdate` let
the bucket's bots follow new releases.
