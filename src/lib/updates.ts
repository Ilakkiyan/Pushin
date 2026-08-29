// In-app auto-update from GitHub Releases (Tauri updater plugin). The updater only swaps the
// installed app bundle — the SQLite DB, downloaded models, and engine all live in the OS app-data
// dir and are never touched, so updating preserves all user data.
//
// Desktop-only: the underlying plugins aren't built on mobile (see src-tauri/Cargo.toml). All calls
// here are wrapped so a dev run (no updater configured) or an offline check resolves quietly to null
// instead of throwing.
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/** Progress of the update download, as a 0–100 percentage (null while the size is still unknown). */
export type UpdateProgress = { downloaded: number; total: number; pct: number | null };

/**
 * Ask GitHub whether a newer release exists. Resolves to the {@link Update} if one is available, or
 * `null` if up to date / the updater isn't available (dev, offline, mobile). Never rejects.
 */
export async function checkForUpdate(): Promise<Update | null> {
  try {
    return await check();
  } catch {
    // No updater configured (dev/non-bundled), offline, or transient endpoint error — treat as
    // "nothing to update". The manual "Check for updates" button surfaces real failures separately.
    return null;
  }
}

/** Translate the plugin's raw download events into a running {@link UpdateProgress}. */
function progressReporter(onProgress?: (p: UpdateProgress) => void) {
  let downloaded = 0;
  let total = 0;
  return (event: { event: string; data?: { contentLength?: number; chunkLength?: number } }) => {
    switch (event.event) {
      case "Started":
        total = event.data?.contentLength ?? 0;
        break;
      case "Progress":
        downloaded += event.data?.chunkLength ?? 0;
        onProgress?.({ downloaded, total, pct: total > 0 ? Math.round((downloaded / total) * 100) : null });
        break;
      case "Finished":
        onProgress?.({ downloaded: total, total, pct: total > 0 ? 100 : null });
        break;
    }
  };
}

/**
 * Fetch the update package and stop there, leaving it staged in the backend for a later
 * {@link installDownloaded}. This is the unattended half: it runs on its own, in the background,
 * with no UI, so that by the time the user is asked anything there is nothing left to wait for.
 * Rejects if the download fails, leaving the caller to decide whether that is worth showing.
 */
export async function downloadUpdate(update: Update, onProgress?: (p: UpdateProgress) => void): Promise<void> {
  await update.download(progressReporter(onProgress));
}

/**
 * Install an update already fetched by {@link downloadUpdate}, then relaunch into the new version.
 * `relaunch()` ends the current process, so nothing after a successful call runs.
 */
export async function installDownloaded(update: Update): Promise<void> {
  await update.install();
  await relaunch();
}

/**
 * Download + install the pending update, then relaunch into the new version. `relaunch()` ends the
 * current process, so nothing after a successful call runs. Rejects if the download/install fails.
 */
export async function installUpdate(update: Update, onProgress?: (p: UpdateProgress) => void): Promise<void> {
  await update.downloadAndInstall(progressReporter(onProgress));
  await relaunch();
}
