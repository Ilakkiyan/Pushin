# Rust and Tauri Backend

The backend lives in `src-tauri`.

## Important Modules

- `commands.rs` exposes the Tauri IPC surface
- `db.rs` owns SQLite migrations and persistence helpers
- `parser.rs` turns natural language into structured plans
- `scheduler.rs` places task blocks into free time
- `habits.rs` handles habit stats and habit scheduling
- `hermes.rs` handles vault recall and embeddings
- `calendar/google.rs` handles Google Calendar sync
- `model_manager.rs` downloads and starts local model servers

## Locking Rule

Do not hold the SQLite mutex across `.await`. Async commands should read what they need, drop the lock, await external work, then reacquire the lock to write.
