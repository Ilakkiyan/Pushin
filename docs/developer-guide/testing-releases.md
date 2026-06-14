# Testing and Releases

## Local Checks

```bash
npm test
npm run build
npm run test:e2e
cargo test --manifest-path src-tauri/Cargo.toml --lib
npm run docs:build
```

## CI

`.github/workflows/test.yml` runs:

- Rust unit and integration tests
- frontend unit/component tests with coverage
- mocked-IPC Playwright E2E

`.github/workflows/pages.yml` builds and deploys this documentation site.

`.github/workflows/release.yml` builds installers when a version tag is pushed.

## Release Flow

1. Bump versions in the app manifests.
2. Commit and push to `main`.
3. Tag the release, for example `v0.3.1`.
4. Push the tag.
5. Review and publish the draft GitHub Release.
