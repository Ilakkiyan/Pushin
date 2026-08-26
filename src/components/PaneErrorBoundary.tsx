import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertTriangle, RotateCcw } from "lucide-react";

/**
 * Catches a render/lifecycle crash in whatever pane is on screen.
 *
 * Without this, React unmounts the ENTIRE tree when any component throws — sidebar, palette and all
 * — so one bad row in one pane turned the whole window blank with no way back except restarting the
 * app. That is not hypothetical: a null payload in PeoplePane did exactly that.
 *
 * It wraps only the pane area, so the sidebar and ⌘K palette stay mounted and the user can simply
 * navigate somewhere else. `resetKey` (the current view) clears the error on navigation, so leaving
 * a broken pane and coming back gives it a fresh attempt rather than a sticky error screen.
 */
interface Props {
  children: ReactNode;
  /** Changing this clears the error — pass the active view so navigating away recovers. */
  resetKey: string;
}

interface State {
  error: Error | null;
}

export default class PaneErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Keep the detail somewhere a user can retrieve it; the on-screen copy stays short.
    console.error("Pane crashed:", error, info.componentStack);
  }

  componentDidUpdate(prev: Props) {
    if (prev.resetKey !== this.props.resetKey && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="flex-1 min-h-0 flex items-center justify-center p-8">
        <div className="max-w-md w-full border border-amber-500/30 bg-amber-500/5 p-5 space-y-3">
          <div className="flex items-center gap-2 text-amber-200 text-sm font-medium">
            <AlertTriangle className="size-4 shrink-0" />
            This view hit a problem
          </div>
          <p className="text-xs text-[var(--ink-muted)]">
            The rest of Pushin is still running — pick another view in the sidebar, or try this one
            again. Your data is safe and nothing was lost.
          </p>
          <pre className="text-[11px] text-[var(--ink-muted)] whitespace-pre-wrap break-words max-h-24 overflow-y-auto bg-black/20 p-2">
            {error.message || String(error)}
          </pre>
          <button
            onClick={() => this.setState({ error: null })}
            className="inline-flex items-center gap-1.5 text-xs px-3 py-1.5 bg-white/10 hover:bg-white/15"
          >
            <RotateCcw className="size-3.5" />
            Try again
          </button>
        </div>
      </div>
    );
  }
}
