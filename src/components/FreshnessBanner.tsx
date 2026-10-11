import { RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import type { WatchStatus } from "../bindings/WatchStatus";
import { formatDateTime } from "../lib/format";
import { ipc } from "../lib/ipc";

const POLL_MS = 5000;

/**
 * Says whether the analysis below is still current, from the folder watcher, and offers
 * to bring it up to date. Never rescans by itself: an update only happens on request.
 * A failing status call just hides the banner, since the analysis itself is unaffected.
 */
export function FreshnessBanner({
  scanId,
  finishedAtMs,
  busy,
  onUpdate,
}: {
  scanId: number;
  finishedAtMs: number;
  busy: boolean;
  onUpdate: () => void;
}) {
  const [status, setStatus] = useState<WatchStatus | null>(null);

  // `scanId` is a dependency on purpose: after an update the counts must be re-read.
  useEffect(() => {
    let disposed = false;
    const read = () =>
      ipc
        .watchStatus()
        .then((s) => !disposed && setStatus(s))
        .catch(() => !disposed && setStatus(null));
    void read();
    const t = setInterval(() => void read(), POLL_MS);
    return () => {
      disposed = true;
      clearInterval(t);
    };
  }, [scanId]);

  if (!status?.watching) return null;
  const changed = status.changedDirs + status.changedSubtrees;
  const sinceLater = status.sinceMs !== null && status.sinceMs > finishedAtMs;

  let text: string;
  let action: string | null = null;
  if (status.lostEvents) {
    text = status.broken
      ? `Sentinel stopped being able to watch this folder (${status.broken}), so the analysis may be out of date.`
      : "Some changes were missed, so the analysis may be out of date.";
    action = "Analyze again";
  } else if (status.afterRestart?.state === "changedWhileClosed") {
    text =
      "Something on this drive changed while Sentinel was closed. Sentinel cannot tell where without administrator rights, so the analysis may be out of date.";
    action = "Analyze again";
  } else if (status.afterRestart?.state === "journalReset") {
    text =
      "Windows reset this drive's change record while Sentinel was closed, so what changed meanwhile cannot be known and the analysis may be out of date.";
    action = "Analyze again";
  } else if (changed > 0) {
    text = `${changed} folder${changed === 1 ? "" : "s"} changed since this analysis.`;
    action = status.canUpdateIncrementally ? "Update (re-reads only those folders)" : "Analyze again";
  } else if (status.afterRestart?.state === "verified") {
    text =
      "Windows recorded no changes on this drive while Sentinel was closed, so this analysis should still be current. No other changes seen since.";
  } else if (sinceLater && status.sinceMs !== null) {
    text = `No changes seen since ${formatDateTime(status.sinceMs)}. Changes before then are not known.`;
  } else {
    text = "No changes seen since this analysis.";
  }

  return (
    <div
      role="status"
      className="flex items-center gap-3 rounded-lg border border-slate-200 bg-white px-4 py-2 text-sm dark:border-slate-800 dark:bg-slate-900"
    >
      <span className="flex-1">{text}</span>
      {action && (
        <button
          type="button"
          disabled={busy}
          onClick={onUpdate}
          className="flex shrink-0 items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1 text-sm font-medium text-white hover:bg-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
        >
          <RefreshCw className="size-4" aria-hidden />
          {action}
        </button>
      )}
    </div>
  );
}
