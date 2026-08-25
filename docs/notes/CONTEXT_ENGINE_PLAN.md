# Phase 1 — Context Engine + universal embeddings (implementation plan)

The keystone from [ROADMAP.md](ROADMAP.md): one retrieval/assembly layer every feature calls,
so the on-device LLM always has the whole knowledge base loaded. Grounded in the current
`hermes.rs` / `db.rs`.

## Problem
Recall today is **notes-only and type-locked**: [`hermes::rank_notes`](src-tauri/src/hermes.rs)
is hardcoded to `Note`, [`db::notes_for_recall`](src-tauri/src/db.rs) only reads `notes`, and
embeddings live in one column (`notes.embedding`, LE-f32 BLOB). Tasks/events/people/goals are
invisible to recall. To share context, recall must span every entity type through one path.

## Design — a polymorphic context index (mirrors `entity_labels`/`entity_links`)

1. **Migration `0012_context_index.sql`** — `entity_index(entity_kind, entity_id, text, text_hash,
   embedding BLOB NULL, embedding_model, updated_at, PRIMARY KEY(entity_kind, entity_id))` + a
   `kind` index. One table → one query for cross-entity recall. Absorbs the existing
   "re-index pages created before the embed server was up" TODO.
2. **Deterministic text projection** (`context::*_text`, pure + tested) — task → title+notes,
   event → title, page → title+plaintext, (later) person/goal.
3. **Generalized ranking** — `ContextItem { kind, id, text, score, embedding }`;
   `hermes::rank_items` carries the same semantic/keyword fallback; `rank_notes` becomes a thin
   adapter so its tests stay green.
4. **Reindex pipeline** (`context::reindex_stale`, best-effort, DB-lock gotcha #8) — scoped-read
   changed/missing rows → drop lock → `embed_batch` → re-lock upsert. `text_hash` skips unchanged
   rows. Triggered on entity create/update + a background sweep from `store.load()`.
5. **Assembler** — `assemble_context(conn, intent, query, budget) -> ContextBundle`: semantic recall
   (cross-kind) + graph neighbors (`entity_links`/`page_links`/`entity_labels`) + recent activity +
   user-memory facts, deduped and token-budgeted.
6. **Wire-in (careful, gotchas #1/#9)** — planner auto-recall → assembler (semantic-only,
   score-gated, capped); `vault_ask` + Cmd-K → cross-entity index.

## Build order (each step is independently testable, `cargo test --lib` green)
- [x] **Step 1 — schema + ranking core**: migration 0012, `EntityKind`/`ContextItem`,
  `rank_items` + `rank_notes` adapter, `project_text`, `entity_index` CRUD, `text_hash`.
- [x] **Step 2 — reindex pipeline**: `commands::reindex_all` (batched async embed + upsert + prune,
  gotcha-#8 lock dance), driven by `db::entities_for_index` + `db::entity_index_meta` +
  `context::needs_index_work`. Wired into `ensure_embeddings` (background sweep on engine-ready).
  *Deferred:* per-mutation single-row hooks — for now new tasks/events index on the next sweep
  (startup / "Start the AI"); pages too. Add inline upserts when live freshness matters.
- [x] **Step 3 — assembler**: `commands::recall_context` (embed query → `hermes::rank_items` over
  `entity_index` → 1-hop `db::entity_neighbors` expansion → `db::recent_entities` tail →
  `context::merge_and_trim` to a `Budget`). Memory facts need no special path — chat→memory saves
  them as pages, which are already recalled. Returns `context::ContextBundle`.
- [x] **Step 4 — wire-in**: planner auto-recall now uses `recall_context` + `gate_recalled_context`
  (semantic-only, ≥0.35, ≤2 — neighbors/recency carry no score so they're excluded, protecting the
  parser). `vault_ask` answers over tasks/events/pages but keeps **page-only citations** (non-page
  slots map to 0 and drop). *Scoped out:* Cmd-K (`hermes_recall`) stays notes/pages-only — broadening
  it surfaces tasks/events in the palette, a UI change for its own slice; so `rank_notes`/
  `notes_for_recall` are **kept** (still used by Cmd-K), not removed.

## Decisions & risks
- **No vector index yet** — full-scan cosine over 384-dim bge vectors is fine into low tens of
  thousands of entities. HNSW is a future lever.
- **Model change resizes vectors** — store `embedding_model`; treat dim-mismatch as unindexed
  (`cosine` already returns 0) and reindex.
- **Parser protection** — assembler stays semantic-only + score-gated in the planner path.
- **People/goals tables don't exist yet** — index ships keyed for them; initial kinds: task/event/page.
