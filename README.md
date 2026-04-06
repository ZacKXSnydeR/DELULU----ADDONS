# EmbeGator

EmbeGator is an external stream-extractor add-on runtime for Delulu.

This repository is intended to stay separate from the core Delulu app repo.

## Build

```powershell
cd bypass
npm install
cd ..
cargo build --release
```

## CLI usage (legacy/manual)

```powershell
embegator movie -i 157336 --json
embegator tv -i 1402 -s 1 -e 1 --json
embegator anime -i 37205 -s 1 -e 1 --json
```

## Release prep (x64 + x86)

Use:

```powershell
.\scripts\prepare-release.ps1 -Version 1.0.0 -RepoOwner <owner> -RepoName <repo>
```

This generates:
- `release-artifacts/embegator-windows-x64.exe`
- `release-artifacts/embegator-windows-x86.exe`
- `release-artifacts/addon.manifest.json` (unsigned placeholder signature)

## Manifest/Catalog files

- `addon.manifest.template.json` -> base template for signed release manifest
- `catalog.sample.json` -> sample entry for Delulu official catalog

## Important

The core Delulu app verifies:
- manifest signature (Ed25519, pinned key id)
- asset SHA256
- HTTPS-only manifest and asset URLs

So before publishing, you must:
1. Update manifest fields/URLs/version.
2. Sign canonical manifest payload with your private key.
3. Put base64 signature in `signature`.
