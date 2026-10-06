import { useEffect, useState } from "react";
import type { AnomalyReport } from "../bindings/AnomalyReport";
import type { ChangeKind } from "../bindings/ChangeKind";
import type { Contributor } from "../bindings/Contributor";
import type { Driver } from "../bindings/Driver";
import { CATEGORY } from "../lib/categories";
import { formatBytes, formatDate, formatDelta } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { ErrorNote, Loading } from "./ui";

const KIND: Record<ChangeKind, string> = {
  added: "New folder",
  grew: "Grew",
  shrank: "Shrank",
  removed: "Removed",
};

const DRIVER: Record<Driver, string> = {
  ownFiles: "mostly files directly in this folder",
  smallFolders: "mostly many small folders added together",
  manyFolders: "spread over several subfolders, none large enough to single out",
  wholeFolder: "the whole folder",
};

function Row({ c }: { c: Contributor }) {
  const lowerBound = c.baseline === "belowThreshold" && c.changeBytes > 0;
  return (
    <li className="py-1.5 text-sm">
      <span className="flex items-baseline gap-2">
        <span className="w-0 flex-1 truncate font-mono text-xs" title={c.path}>
          {c.path}
        </span>
        <span
          className={`shrink-0 tabular-nums ${c.changeBytes > 0 ? "text-amber-700 dark:text-amber-300" : "text-emerald-700 dark:text-emerald-400"}`}
        >
          {lowerBound && "at least "}
          {formatDelta(c.changeBytes)}
        </span>
      </span>
      <span className="block text-xs text-slate-500 dark:text-slate-400">
        {KIND[c.kind]}
        {c.beforeBytes !== null && c.kind !== "added" && ` from ${formatBytes(c.beforeBytes)}`}
        {c.kind !== "removed" && ` to ${formatBytes(c.nowBytes)}`}
        {" · "}
        {DRIVER[c.driver]}
        {c.shareOfNetChange !== null && ` · ${Math.round(c.shareOfNetChange)}% of the net change`}
        {c.classification && ` · ${CATEGORY[c.classification.category].label}`}
      </span>
    </li>
  );
}

/**
 * What grew or shrank most since an earlier analysis and which folders account for it.
 * Describes changes in size only; it does not say why, or that anything should be removed.
 */
export function AnomalyPanel({ scanId }: { scanId: number }) {
  const [report, setReport] = useState<AnomalyReport | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let off = false;
    ipc.anomalyReport(scanId).then(
      (r) => !off && setReport(r),
      (e: unknown) => {
        if (off) return;
        // "Nothing earlier to compare with" is an ordinary state, not a failure.
        const kind = e && typeof e === "object" && "kind" in e ? e.kind : null;
        if (kind === "notFound") setNote(describeError(e));
        else setError(describeError(e));
      },
    );
    return () => {
      off = true;
    };
  }, [scanId]);

  if (error) return <ErrorNote message={error} />;
  if (note) return <p className="text-sm text-slate-500 dark:text-slate-400">{note}</p>;
  if (!report) return <Loading />;

  const nothing = report.grew.length === 0 && report.shrank.length === 0 && report.largeFiles.length === 0;
  return (
    <div className="space-y-4">
      <p className="text-sm">
        Since {formatDate(report.baseline.finishedAtMs)} the drive changed by{" "}
        <span className="font-semibold tabular-nums">{formatDelta(report.netChangeBytes)}</span> (
        {formatBytes(report.baseline.totalBytes)} to {formatBytes(report.current.totalBytes)}).
      </p>
      {nothing && (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          No single folder changed by {formatBytes(report.reportingThresholdBytes)} or more, so the change is many small
          ones.
        </p>
      )}
      {report.grew.length > 0 && (
        <div>
          <h3 className="mb-1 text-sm font-medium">Where it grew</h3>
          <ul className="divide-y divide-slate-100 dark:divide-slate-800">
            {report.grew.map((c) => (
              <Row key={c.path} c={c} />
            ))}
          </ul>
        </div>
      )}
      {report.shrank.length > 0 && (
        <div>
          <h3 className="mb-1 text-sm font-medium">Where it shrank</h3>
          <ul className="divide-y divide-slate-100 dark:divide-slate-800">
            {report.shrank.map((c) => (
              <Row key={c.path} c={c} />
            ))}
          </ul>
        </div>
      )}
      {report.largeFiles.length > 0 && (
        <div>
          <h3 className="mb-1 text-sm font-medium">New or bigger large files</h3>
          <ul className="text-sm">
            {report.largeFiles.map((f) => (
              <li key={f.path} className="flex gap-2 py-0.5">
                <span className="w-0 flex-1 truncate font-mono text-xs" title={f.path}>
                  {f.path}
                </span>
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {f.beforeBytes === null ? "new" : `was ${formatBytes(f.beforeBytes)}`}
                </span>
                <span className="w-24 text-right tabular-nums">{formatBytes(f.bytes)}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <p className="text-xs text-slate-500 dark:text-slate-400">
        Not accounted for by the folders above: <span className="tabular-nums">{formatDelta(report.unattributedBytes)}</span>
        {" "}(many smaller changes, and anything under {formatBytes(report.reportingThresholdBytes)}). These are changes in
        space used between two moments; they do not say why a folder changed or that it should be cleaned.
      </p>
      {report.caveats.map((w) => (
        <p key={w} className="text-xs text-amber-700 dark:text-amber-300">
          {w}
        </p>
      ))}
    </div>
  );
}
