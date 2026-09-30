import { Archive, File, Folder, Link2, Lock } from "lucide-react";
import { useState } from "react";
import type { CleanupRunResponse } from "../bindings/CleanupRunResponse";
import type { Decision } from "../bindings/Decision";
import type { ItemKind } from "../bindings/ItemKind";
import type { Preview } from "../bindings/Preview";
import type { PreviewItem } from "../bindings/PreviewItem";
import type { PreviewResponse } from "../bindings/PreviewResponse";
import { formatAge, formatBytes, formatDateTime } from "../lib/format";
import { describeError } from "../lib/ipc";
import { statusText } from "../lib/quarantine";
import { ConfirmMove } from "./ConfirmMove";
import { ErrorNote } from "./ui";

const KIND_ICON: Record<ItemKind, typeof File> = { file: File, folder: Folder, link: Link2 };
const count = (n: number) => n.toLocaleString();
const MAX_ROWS = 200;

function decisionText(d: Decision): string {
  switch (d.state) {
    case "eligible":
      return "Would be moved to quarantine";
    case "tooRecent":
      return `Kept: changed ${formatAge(d.newestModifiedMs)}`;
    case "protected":
      return `Protected: ${d.reason}`;
    case "skipped":
      return `Skipped: ${d.reason}`;
  }
}

type Filter = "eligible" | "kept" | "all";

function RunResult({ result }: { result: CleanupRunResponse }) {
  const entries = result.manifest.entries;
  const moved = entries.filter((e) => e.status.state === "quarantined");
  const left = entries.filter((e) => e.status.state !== "quarantined");
  const bytes = moved.reduce((s, e) => s + e.bytes, 0);
  return (
    <div role="status" className="rounded-md border border-slate-200 p-3 text-sm dark:border-slate-800">
      <p className="flex items-center gap-2">
        <Archive className="size-4 text-slate-400" aria-hidden />
        Moved {count(moved.length)} {moved.length === 1 ? "item" : "items"} ({formatBytes(bytes)}) to quarantine.
        {left.length > 0 && ` ${count(left.length)} left in place.`}
      </p>
      {left.length > 0 && (
        <ul className="mt-2 space-y-0.5 text-xs text-slate-500 dark:text-slate-400">
          {left.map((e) => (
            <li key={e.index}>
              <span className="font-mono">{e.originalPath}</span>: {statusText(e.status)}
            </li>
          ))}
        </ul>
      )}
      <p className="mt-2 text-xs text-slate-500 dark:text-slate-400">
        Recorded in Activity. Restore from Cleanup → Quarantine. Preview again to see the current state.
      </p>
    </div>
  );
}

export function PreviewResult({
  response,
  providerName,
  onRun,
  onMoved,
}: {
  response: PreviewResponse;
  providerName: string;
  /** Moves exactly the given paths (the preview's eligible items) into quarantine. */
  onRun: (paths: string[]) => Promise<CleanupRunResponse>;
  onMoved: () => void;
}) {
  const preview: Preview = response.preview;
  const [filter, setFilter] = useState<Filter>("eligible");
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CleanupRunResponse | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const eligible = preview.items.filter((i) => i.decision.state === "eligible");
  const kept = preview.items.filter((i) => i.decision.state !== "eligible");
  const keptBytes = kept.reduce((s, i) => s + i.bytes, 0);
  const shown: PreviewItem[] = filter === "eligible" ? eligible : filter === "kept" ? kept : preview.items;
  const move = async () => {
    setRunning(true);
    setRunError(null);
    try {
      const res = await onRun(eligible.map((i) => i.path));
      setResult(res);
      onMoved();
    } catch (err) {
      setRunError(describeError(err));
    } finally {
      setRunning(false);
      setConfirming(false);
    }
  };
  const tabs: { id: Filter; label: string }[] = [
    { id: "eligible", label: `Would be removed (${count(eligible.length)})` },
    { id: "kept", label: `Kept (${count(kept.length)})` },
    { id: "all", label: `All (${count(preview.items.length)})` },
  ];

  return (
    <div className="mt-4 space-y-3 border-t border-slate-200 pt-4 dark:border-slate-800">
      <p className="text-xs text-slate-500 dark:text-slate-400">
        Preview from {formatDateTime(preview.generatedAtMs)}. Nothing was changed.{" "}
        {response.auditSeq !== null
          ? `Recorded in Activity as entry ${response.auditSeq}.`
          : "This preview could not be recorded in Activity (see the log file)."}
      </p>
      {preview.roots.map((r) =>
        r.state === "scanned" ? null : (
          <p key={r.path} className="text-sm text-slate-500 dark:text-slate-400">
            {r.state === "missing" ? `${r.path} does not exist on this computer.` : `${r.path} was not checked: ${r.reason}`}
          </p>
        ),
      )}
      <p className="text-sm">
        A cleanup would free <span className="text-lg font-semibold tabular-nums">{formatBytes(preview.eligibleBytes)}</span>{" "}
        by moving {count(preview.eligibleItems)} {preview.eligibleItems === 1 ? "item" : "items"} (
        {count(preview.eligibleFiles)} files) to quarantine.{" "}
        <span className="text-slate-500 dark:text-slate-400">
          {count(kept.length)} items ({formatBytes(keptBytes)}) would be kept.
        </span>
      </p>
      {eligible.length > 0 && !result && (
        <button
          type="button"
          onClick={() => setConfirming(true)}
          className="flex items-center gap-1.5 rounded-md bg-amber-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-amber-700"
        >
          <Archive className="size-4" aria-hidden />
          Move {count(eligible.length)} {eligible.length === 1 ? "item" : "items"} to quarantine…
        </button>
      )}
      {runError && <ErrorNote message={runError} />}
      {result && <RunResult result={result} />}
      {confirming && (
        <ConfirmMove
          providerName={providerName}
          items={eligible}
          busy={running}
          onConfirm={() => void move()}
          onCancel={() => setConfirming(false)}
        />
      )}
      {preview.incomplete && (
        <p role="alert" className="text-sm text-amber-700 dark:text-amber-300">
          The preview stopped early, so these totals are incomplete.
        </p>
      )}
      <div role="tablist" aria-label="Filter items" className="flex gap-1">
        {tabs.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={filter === t.id}
            onClick={() => setFilter(t.id)}
            className={`rounded-md px-3 py-1 text-sm ${
              filter === t.id
                ? "bg-slate-200 font-medium dark:bg-slate-800"
                : "text-slate-500 hover:bg-slate-100 dark:text-slate-400 dark:hover:bg-slate-800/60"
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>
      {shown.length === 0 ? (
        <p className="text-sm text-slate-500">Nothing here.</p>
      ) : (
        <ul className="divide-y divide-slate-100 text-sm dark:divide-slate-800">
          {shown.slice(0, MAX_ROWS).map((i) => {
            const Icon = KIND_ICON[i.kind];
            const text = decisionText(i.decision);
            return (
              <li key={i.path} className="flex items-center gap-3 py-1.5">
                <Icon className="size-4 shrink-0 text-slate-400" aria-hidden />
                <span className="w-0 flex-1 truncate font-mono text-xs" title={i.path}>
                  {i.path}
                </span>
                <span
                  className="flex w-80 items-center gap-1.5 truncate text-xs text-slate-500 dark:text-slate-400"
                  title={text}
                >
                  {i.decision.state === "protected" && <Lock className="size-3.5 shrink-0" aria-hidden />}
                  {text}
                </span>
                <span className="w-20 text-right tabular-nums">
                  {i.kind === "link" ? "link only" : formatBytes(i.bytes)}
                </span>
              </li>
            );
          })}
          {shown.length > MAX_ROWS && (
            <li className="py-1.5 text-sm text-slate-500">{count(shown.length - MAX_ROWS)} more not shown</li>
          )}
        </ul>
      )}
    </div>
  );
}

