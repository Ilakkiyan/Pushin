# Frontend

The frontend is React, TypeScript, Vite, Tailwind, and Zustand.

## Main Areas

- `App.tsx` composes the sidebar shell and active pane
- `state/store.ts` owns app state and refresh/mutation flows
- `lib/ipc.ts` defines typed wrappers over Tauri commands
- `panes/` contains user-facing views
- `components/` contains reusable UI controls

## Editor and Graph

The vault editor uses BlockNote. The graph view uses `react-force-graph-2d`.

## Testing

Frontend tests use Vitest, Testing Library, jsdom, IPC contract tests, and Playwright mocked-IPC E2E.
