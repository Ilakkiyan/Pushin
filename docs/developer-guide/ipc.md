# IPC Contract

The frontend talks to Rust through Tauri `invoke` calls wrapped in `src/lib/ipc.ts`.

When adding, renaming, or removing a command, update these together:

- Rust command implementation
- Tauri `generate_handler![]` registration
- TypeScript wrapper in `lib/ipc.ts`
- E2E mock bridge if the command is used in a user flow

The IPC contract test compares registered Rust commands with frontend invocations and fails on drift.
