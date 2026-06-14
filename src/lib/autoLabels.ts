import type { LabelKind } from "./ipc";

export interface AutoLabelTarget {
  kind: Extract<LabelKind, "task" | "event">;
  id: number;
  title: string;
  text?: string;
}

export interface AutoLabelSuggestion {
  key: string;
  kind: Extract<LabelKind, "task" | "event">;
  entityId: number;
  entityTitle: string;
  labelName: string;
  color: string;
  reason: string;
}

const RULES = [
  {
    labelName: "Health",
    color: "#10b981",
    keywords: ["gym", "workout", "exercise", "run", "yoga", "doctor", "dentist", "therapy", "medication", "health"],
  },
  {
    labelName: "Work",
    color: "#0ea5e9",
    keywords: ["meeting", "standup", "stand up", "sync", "call", "client", "presentation", "deck", "slides", "interview", "work"],
  },
  {
    labelName: "Errands",
    color: "#f59e0b",
    keywords: ["errand", "errands", "grocery", "groceries", "store", "pickup", "pick up", "buy", "bank", "post office", "pharmacy"],
  },
] as const;

function normalize(text: string): string {
  return text.toLowerCase().replace(/[#@]/g, "").replace(/[-_/]+/g, " ").replace(/\s+/g, " ").trim();
}

function containsKeyword(text: string, keyword: string): boolean {
  const haystack = normalize(text);
  const needle = normalize(keyword);
  if (!haystack || !needle) return false;
  if (needle.includes(" ")) return haystack.includes(needle);
  return new RegExp(`(^|\\W)${escapeRegExp(needle)}($|\\W)`).test(haystack);
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Deterministic keyword -> label pass for new planner output. It intentionally avoids model calls
 * and prompt changes; the user still confirms before anything is stored. */
export function suggestAutoLabels(targets: AutoLabelTarget[], userText: string): AutoLabelSuggestion[] {
  const useFallback = targets.length === 1;
  const out: AutoLabelSuggestion[] = [];
  const seen = new Set<string>();

  for (const target of targets) {
    const text = [target.title, target.text, useFallback ? userText : ""].filter(Boolean).join(" ");
    for (const rule of RULES) {
      const matched = rule.keywords.find((keyword) => containsKeyword(text, keyword));
      if (!matched) continue;
      const key = `${target.kind}:${target.id}:${rule.labelName.toLowerCase()}`;
      if (!seen.has(key)) {
        seen.add(key);
        out.push({
          key,
          kind: target.kind,
          entityId: target.id,
          entityTitle: target.title,
          labelName: rule.labelName,
          color: rule.color,
          reason: matched,
        });
      }
      break;
    }
  }

  return out;
}
