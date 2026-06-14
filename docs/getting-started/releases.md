# Download Releases

Installers are published through GitHub Releases:

[github.com/Ilakkiyan/Pushin/releases](https://github.com/Ilakkiyan/Pushin/releases)

## Builds

| OS | Artifacts |
| --- | --- |
| Windows | `.msi` and NSIS `.exe` |
| macOS | `.dmg` for Apple Silicon and Intel |
| Linux | `.AppImage` and `.deb` |

Builds are currently unsigned, so macOS Gatekeeper or Windows SmartScreen may ask you to confirm the first launch.

## Local Build

```bash
npm run tauri build
```

This builds for your current OS only.
