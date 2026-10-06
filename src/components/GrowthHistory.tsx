import { useEffect, useState } from "react";
import type { FolderHistory as FolderHistoryData } from "../bindings/FolderHistory";
import type { GrowthHistory as GrowthHistoryData } from "../bindings/GrowthHistory";
import type { GrowthSummary } from "../bindings/GrowthSummary";
import { CATEGORY } from "../lib/categories";
import { formatBytes, formatDate, formatDateTime, formatDelta } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { Sparkline } from "./Sparkline";
import { ErrorNote, Loading } from "./ui";

function SummaryLine({ s, what }: { s: GrowthSummary; what: string }) {
  return (
    <p className="text-sm">
      {what} went from <span className="tabular-nums">{formatBytes(s.fromBytes)}</span> on {formatDate(s.fromMs)} to{" "}
      <span className="tabular-nums">{formatBytes(s.toBytes)}</span> on {formatDate(s.toMs)} (
      {formatDelta(s.changeBytes)}
      {s.bytesPerDay !== null && `, about ${formatDelta(Math.round(s.bytesPerDay))} a day`}), across {s.analyses} complete
      analyses.
    </p>
  );
}

/** How the analyzed drive or folder changed across the saved analyses of its root. */
export function GrowthHistory({ scanId }: { scanId: number }) {
  const [h, setH] = useState<GrowthHistoryData | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let off = false;
    ipc.growthHistory(scanId).then(
      (r) => !off && setH(r),
      (e: unknown) => !off && setError(describeError(e)),
    );
    return () => {
      off = true;
    };
  }, [scanId]);

  if (error) return <ErrorNote message={error} />;
  if (!h) return <Loading />;
  if (h.points.length < 2) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        History appears after a second analysis of {h.root}. Sentinel keeps up to {h.keptAnalyses} analyses of each
        drive.
      </p>
    );
  }
  const first = h.points[0];
  const last = h.points[h.points.length - 1];
  return (
    <div className="space-y-3">
      <Sparkline
        label={`Size of ${h.root} across ${h.points.length} analyses`}
        points={h.points.map((p) => ({ x: p.finishedAtMs, y: p.totalBytes, partial: p.partial }))}
      />
      {h.summary ? (
        <SummaryLine s={h.summary} what={h.root} />
      ) : (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Only one complete analysis, so there is no trend yet.
        </p>
      )}
      <p className="text-xs text-slate-500 dark:text-slate-400">
        {h.points.length} analyses from {first && formatDateTime(first.finishedAtMs)} to{" "}
        {last && formatDateTime(last.finishedAtMs)}. Sentinel keeps up to {h.keptAnalyses} per drive: the newest three always, then one per day for two weeks and one per
        week after that, so older history is sparser and analyses made close together are merged. Hollow dots are cancelled
        or incomplete analyses, left out of the trend.
      </p>
      {h.categoryChanges.length > 0 && (
        <div>
          <p className="mb-1 text-xs text-slate-500 dark:text-slate-400">What changed most, by category</p>
          <ul className="text-sm">
            {h.categoryChanges.slice(0, 6).map((c) => (
              <li key={c.category} className="flex gap-2 py-0.5">
                <span className="flex-1">{CATEGORY[c.category].label}</span>
                <span className="w-24 text-right tabular-nums">{formatBytes(c.lastBytes)}</span>
                <span
                  className={`w-24 text-right tabular-nums ${c.changeBytes > 0 ? "text-amber-700 dark:text-amber-300" : "text-emerald-700 dark:text-emerald-400"}`}
                >
                  {formatDelta(c.changeBytes)}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/** One folder's size across analyses, read by path. Loaded only when asked for. */
export function FolderHistory({ scanId, node }: { scanId: number; node: number }) {
  const [h, setH] = useState<FolderHistoryData | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let off = false;
    ipc.folderHistory(scanId, node).then(
      (r) => !off && setH(r),
      (e: unknown) => !off && setError(describeError(e)),
    );
    return () => {
      off = true;
    };
  }, [scanId, node]);

  if (error) return <ErrorNote message={error} />;
  if (!h) return <Loading />;
  return (
    <div className="space-y-2 rounded-md border border-slate-200 p-3 dark:border-slate-800">
      <Sparkline
        label={`Size of ${h.path} across ${h.points.length} analyses`}
        points={h.points.map((p) => ({ x: p.finishedAtMs, y: p.bytes, partial: p.partial }))}
      />
      {h.summary ? (
        <SummaryLine s={h.summary} what="This folder" />
      ) : (
        <p className="text-sm text-slate-500 dark:text-slate-400">Not enough complete analyses that stored this folder for a trend.</p>
      )}
      <ul className="text-xs text-slate-600 dark:text-slate-300">
        {h.points.map((p) => (
          <li key={p.scanId} className="flex gap-2 py-0.5">
            <span className="w-40">{formatDateTime(p.finishedAtMs)}</span>
            <span className="tabular-nums">
              {p.state === "known" && p.bytes !== null && formatBytes(p.bytes)}
              {p.state === "belowThreshold" && `under ${formatBytes(p.maxBytes ?? 0)} (too small to be stored)`}
              {p.state === "missing" && "not there"}
              {p.partial && " (incomplete analysis)"}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
