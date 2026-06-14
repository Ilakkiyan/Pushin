# Run from Source

```bash
git clone https://github.com/Ilakkiyan/Pushin.git
cd Pushin
npm install
npm run tauri dev
```

`npm run tauri dev` starts Vite, compiles the Rust backend, and opens the desktop app.

## Local Data

Pushin stores its app data in your OS application data directory under `com.pushin.app`. That includes:

- `pushin.db` for tasks, events, settings, pages, labels, and sync metadata
- downloaded model files
- the managed `llama-server` binary and support files

## Common Development Commands

```bash
npm test
npm run build
npm run test:e2e
cargo test --manifest-path src-tauri/Cargo.toml --lib
```
