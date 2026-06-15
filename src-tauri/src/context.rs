//! The Context Engine — the shared retrieval/assembly layer every feature calls so the on-device
//! LLM always has the relevant slice of the user's whole knowledge base loaded (see
//! CONTEXT_ENGINE_PLAN.md). Step 1 ships the deterministic foundations: how each entity projects to
//! embeddable text, and a stable content hash so reindexing skips unchanged rows. Ranking lives in
//! `hermes` (`rank_items`); the reindex pipeline and assembler land in later steps.

use crate::model::{ContextItem, EntityKind, Event, Task};
use std::collections::HashSet;

/// Embeddable text for a task: its title, plus notes when present. Trimmed and deterministic so the
/// hash is stable across runs.
pub fn task_text(t: &Task) -> String {
    let title = t.title.trim();
    let notes = t.notes.trim();
    if notes.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n{notes}")
    }
}

/// Embeddable text for an event: its title (the only free-text field worth recalling on).
pub fn event_text(e: &Event) -> String {
    e.title.trim().to_string()
}

/// Embeddable text for a person: name, plus notes when present.
pub fn person_text(name: &str, notes: &str) -> String {
    let name = name.trim();
    let notes = notes.trim();
    if notes.is_empty() {
        name.to_string()
    } else {
        format!("{name}\n{notes}")
    }
}

/// Embeddable text for a vault page: title + plaintext body. Avoids duplicating the title when the
/// body already leads with it (legacy notes derive their title from the first line).
pub fn page_text(title: &str, content: &str) -> String {
    let title = title.trim();
    let content = content.trim();
    if title.is_empty() {
        content.to_string()
    } else if content.is_empty() || content.starts_with(title) {
        if content.is_empty() { title.to_string() } else { content.to_string() }
    } else {
        format!("{title}\n{content}")
    }
}

/// The stored state of an entity's index row, used to decide whether the reindex pipeline needs to
/// touch it. (`model` is the embedding model that produced the current vector, if any.)
#[derive(Debug, Clone)]
pub struct IndexState {
    pub text_hash: String,
    pub has_embedding: bool,
    pub model: Option<String>,
}

/// Whether the reindex pipeline should (re)write an entity's index row, given its prior state, the
/// hash of its current text, and the currently configured embedding model. True when the row is new,
/// its text changed, or — with an embed backend available — it lacks a vector or was embedded by a
/// different model. False (skip) for unchanged, already-current rows. Pure, so it's unit-tested.
pub fn needs_index_work(existing: Option<&IndexState>, new_hash: &str, current_model: &str) -> bool {
    let model_present = !current_model.trim().is_empty();
    match existing {
        None => true,
        Some(s) => {
            if s.text_hash != new_hash {
                return true;
            }
            model_present && (!s.has_embedding || s.model.as_deref() != Some(current_model))
        }
    }
}

/// The assembled, ready-to-prompt context for an intent: the recall `mode` that ran plus the chosen,
/// deduped, budget-trimmed items. The output of the Context Engine that every feature consumes.
pub struct ContextBundle {
    pub mode: String,
    pub items: Vec<ContextItem>,
}

/// A budget for assembled context: caps on item count and total characters (≈4 chars/token).
pub struct Budget {
    pub max_items: usize,
    pub max_chars: usize,
}

/// Merge prioritized groups of candidates into one list: dedupe by `(kind, id)` keeping the first
/// (highest-priority) occurrence, and stop at the budget. Earlier groups win — pass strongest recall
/// first, then graph neighbors, then recency. An item that would overflow the char budget is skipped
/// (but scanning continues, so a smaller later item can still fit). Pure.
pub fn merge_and_trim(groups: Vec<Vec<ContextItem>>, budget: &Budget) -> Vec<ContextItem> {
    let mut seen: HashSet<(EntityKind, i64)> = HashSet::new();
    let mut out: Vec<ContextItem> = Vec::new();
    let mut chars = 0usize;
    for group in groups {
        for it in group {
            if out.len() >= budget.max_items {
                return out;
            }
            if !seen.insert((it.kind, it.id)) {
                continue;
            }
            let len = it.text.chars().count();
            if !out.is_empty() && chars + len > budget.max_chars {
                continue;
            }
            chars += len;
            out.push(it);
        }
    }
    out
}

/// Stable FNV-1a (64-bit) hash of the projected text, hex-encoded. Used as `entity_index.text_hash`
/// so the reindex pipeline can skip rows whose text has not changed. Deterministic across Rust
/// versions (unlike `DefaultHasher`), which matters because the value is persisted.
pub fn text_hash(s: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(title: &str, notes: &str) -> Task {
        Task {
            id: 1,
            project_id: None,
            title: title.into(),
            notes: notes.into(),
            estimated_minutes: 60,
            deadline: None,
            earliest_start: None,
            priority: 2,
            min_chunk_minutes: 30,
            max_chunk_minutes: 120,
            status: "todo".into(),
            created_at: "2026-01-01T00:00:00".into(),
            depends_on: vec![],
        }
    }

    #[test]
    fn task_text_includes_notes_only_when_present() {
        assert_eq!(task_text(&task("Write report", "")), "Write report");
        assert_eq!(task_text(&task("  Write report  ", "  for Q3  ")), "Write report\nfor Q3");
    }

    #[test]
    fn page_text_avoids_duplicating_a_leading_title() {
        assert_eq!(page_text("Trip plan", "Trip plan\npack bags"), "Trip plan\npack bags");
        assert_eq!(page_text("Trip plan", "book flights"), "Trip plan\nbook flights");
        assert_eq!(page_text("", "just a note"), "just a note");
        assert_eq!(page_text("Title only", ""), "Title only");
    }

    #[test]
    fn needs_index_work_covers_new_changed_and_current() {
        let st = |hash: &str, emb: bool, model: Option<&str>| IndexState {
            text_hash: hash.into(),
            has_embedding: emb,
            model: model.map(str::to_string),
        };
        // New entity → always work.
        assert!(needs_index_work(None, "h1", "bge"));
        // Unchanged + already embedded by the current model → skip.
        assert!(!needs_index_work(Some(&st("h1", true, Some("bge"))), "h1", "bge"));
        // Text changed → work.
        assert!(needs_index_work(Some(&st("h1", true, Some("bge"))), "h2", "bge"));
        // Tracked but never embedded, backend now available → work (back-fill the vector).
        assert!(needs_index_work(Some(&st("h1", false, None)), "h1", "bge"));
        // Embedded by a different model → re-embed.
        assert!(needs_index_work(Some(&st("h1", true, Some("old"))), "h1", "bge"));
        // No backend configured: only (re)write when new or text changed, never to chase a vector.
        assert!(!needs_index_work(Some(&st("h1", false, None)), "h1", ""));
        assert!(needs_index_work(Some(&st("h1", false, None)), "h2", ""));
    }

    fn item(kind: EntityKind, id: i64, text: &str) -> ContextItem {
        ContextItem { kind, id, text: text.into(), score: None, embedding: None }
    }

    #[test]
    fn merge_dedupes_by_kind_and_id_keeping_priority_order() {
        let recall = vec![item(EntityKind::Page, 1, "a"), item(EntityKind::Task, 2, "b")];
        let neighbors = vec![item(EntityKind::Task, 2, "b-dup"), item(EntityKind::Event, 3, "c")];
        let budget = Budget { max_items: 10, max_chars: 1000 };
        let out = merge_and_trim(vec![recall, neighbors], &budget);
        let ids: Vec<_> = out.iter().map(|it| (it.kind, it.id)).collect();
        assert_eq!(ids, vec![(EntityKind::Page, 1), (EntityKind::Task, 2), (EntityKind::Event, 3)]);
        // A task with the same id but different kind is NOT a duplicate.
        let mixed = merge_and_trim(vec![vec![item(EntityKind::Page, 5, "p"), item(EntityKind::Task, 5, "t")]], &budget);
        assert_eq!(mixed.len(), 2);
    }

    #[test]
    fn merge_respects_item_and_char_budgets() {
        let many = (0..10).map(|i| item(EntityKind::Page, i, "x")).collect();
        let capped = merge_and_trim(vec![many], &Budget { max_items: 3, max_chars: 1000 });
        assert_eq!(capped.len(), 3);

        // First item always admitted; an oversized second is skipped but a small third still fits.
        let groups = vec![vec![item(EntityKind::Page, 1, "aaaa"), item(EntityKind::Page, 2, "bbbbbbbb"), item(EntityKind::Page, 3, "c")]];
        let out = merge_and_trim(groups, &Budget { max_items: 10, max_chars: 5 });
        let ids: Vec<_> = out.iter().map(|it| it.id).collect();
        assert_eq!(ids, vec![1, 3], "oversized #2 skipped, small #3 admitted");
    }

    #[test]
    fn text_hash_is_stable_and_sensitive() {
        // Stable across calls (regression guard on the constants).
        assert_eq!(text_hash("hello"), text_hash("hello"));
        assert_eq!(text_hash("hello"), "a430d84680aabd0b");
        assert_ne!(text_hash("hello"), text_hash("hellp"));
        assert_eq!(text_hash("hello").len(), 16);
    }
}
