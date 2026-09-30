import { AlertTriangle, ChevronRight, CornerDownRight, EyeOff, File, Folder, Link2, Loader2, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { DirChild } from "../bindings/DirChild";
import type { DirListing } from "../bindings/DirListing";
import type { LargeFile } from "../bindings/LargeFile";
import type { NodeStatus } from "../bindings/NodeStatus";
import type { SavedScan } from "../bindings/SavedScan";
import type { ScanProgressEvent } from "../bindings/ScanProgressEvent";
import { formatBytes, formatDateTime, formatDelta, percent } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { ErrorNote, Loading, Panel } from "./ui";

const count = (n: number) => n.toLocaleString();

export function ScanProgressView({ run, onCancel }: { run: ScanProgressEvent; onCancel: (id: number) => void }) {
  const p = run.progress;
  return (
    <Panel title={`Analyzing ${run.root}`}>
      <div className="flex items-center gap-4">
        <Loader2 className="size-5 shrink-0 animate-spin text-emerald-500" aria-hidden />
        <p className="flex-1 text-sm tabular-nums" aria-live="polite">
          {formatBytes(p.bytes)} counted in {count(p.files)} files and {count(p.dirs)} folders
          {p.problems > 0 && <span className="text-slate-500"> · {count(p.problems)} unreadable</span>}
        </p>
        <button
          type="button"
          onClick={() => onCancel(run.id)}
          className="flex items-center gap-1 rounded-md border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-100 dark:border-slate-700 dark:hover:bg-slate-800"
        >
          <X className="size-4" aria-hidden /> Cancel
        </button>
      </div>
    </Panel>
  );
}

function ScanSummary({ last }: { last: SavedScan }) {
  const s = last.stats;
  const notes: string[] = [];
  if (s.cancelled) notes.push("The analysis was cancelled, so totals are incomplete.");
  if (s.truncated) notes.push("A safety limit on depth or entry count was reached, so totals are incomplete.");
  if (s.accessDenied > 0)
    notes.push(`${count(s.accessDenied)} folders could not be read without administrator rights and are not counted.`);
  if (s.errors > 0) notes.push(`${count(s.errors)} folders could not be read because of errors.`);
  const prev = last.previous;
  return (
    <div className="space-y-2 text-sm">
      <p className="text-slate-500 dark:text-slate-400">Analyzed {formatDateTime(last.finishedAtMs)}</p>
      <p>
        <span className="font-semibold">{formatBytes(s.totalBytes)}</span> on disk in {count(s.files)} files and{" "}
        {count(s.dirs)} folders, analyzed in {(s.elapsedMs / 1000).toFixed(1)} s.
        {s.linksSkipped > 0 && (
          <span className="text-slate-500 dark:text-slate-400">
            {" "}
            {count(s.linksSkipped)} links (junctions and symlinks) were not followed.
          </span>
        )}
      </p>
      {prev && (
        <p>
          Since the previous analysis on {formatDateTime(prev.finishedAtMs)}:{" "}
          <span className="font-semibold tabular-nums">{formatDelta(s.totalBytes - prev.totalBytes)}</span>
        </p>
      )}
      {notes.map((n) => (
        <p key={n} className="flex items-start gap-2 text-amber-700 dark:text-amber-300">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
          {n}
        </p>
      ))}
    </div>
  );
}

function statusLabel(status: NodeStatus): { text: string; icon: typeof Link2 } | null {
  switch (status.state) {
    case "complete":
      return null;
    case "accessDenied":
      return { text: "Access denied, not counted", icon: AlertTriangle };
    case "error":
      return { text: `Unreadable: ${status.message}`, icon: AlertTriangle };
    case "link":
      return { text: status.kind === "symlink" ? "Symbolic link, not followed" : "Junction, not followed", icon: Link2 };
    case "excluded":
      return { text: "Excluded", icon: EyeOff };
    case "notScanned":
      return {
        text:
          status.reason === "onlineOnly"
            ? "Online-only cloud folder, not downloaded"
            : status.reason === "cancelled"
              ? "Not scanned (cancelled)"
              : "Not scanned (safety limit)",
        icon: EyeOff,
      };
  }
}

function changeText(child: DirChild, compared: boolean): string {
  if (!compared || child.status.state !== "complete") return "";
  if (child.previousBytes === null) return "new or was under 1 MB";
  return formatDelta(child.totalBytes - child.previousBytes);
}

function Row({
  child,
  parentTotal,
  compared,
  onOpen,
}: {
  child: DirChild;
  parentTotal: number;
  compared: boolean;
  onOpen: () => void;
}) {
  const label = statusLabel(child.status);
  const pct = percent(child.totalBytes, parentTotal);
  const openable = child.hasChildren && child.status.state === "complete";
  const content = (
    <>
      <Folder className="size-4 shrink-0 text-slate-400" aria-hidden />
      <span className="w-64 min-w-0 truncate" title={child.name}>
        {child.name}
      </span>
      {label ? (
        <span className="flex flex-1 items-center gap-1.5 text-xs text-slate-500 dark:text-slate-400">
          <label.icon className="size-3.5" aria-hidden />
          {label.text}
        </span>
      ) : (
        <span className="flex flex-1 items-center" aria-hidden>
          <span className="h-2 w-full overflow-hidden rounded-full bg-emerald-100 dark:bg-emerald-950">
            <span
              className="block h-full rounded-full bg-emerald-600 dark:bg-emerald-400"
              style={{ width: `${Math.max(pct, child.totalBytes > 0 ? 0.5 : 0)}%` }}
            />
          </span>
        </span>
      )}
      {/* Links, denied and skipped folders were not measured: show no size rather than "0 bytes". */}
      <span className="w-20 text-right tabular-nums">{label ? "" : formatBytes(child.totalBytes)}</span>
      <span className="w-12 text-right text-xs tabular-nums text-slate-500 dark:text-slate-400">
        {pct >= 0.1 ? `${pct.toFixed(1)}%` : ""}
      </span>
      {compared && (
        <span className="w-36 text-right text-xs tabular-nums text-slate-500 dark:text-slate-400">
          {changeText(child, compared)}
        </span>
      )}
      <ChevronRight className={`size-4 shrink-0 ${openable ? "text-slate-400" : "invisible"}`} aria-hidden />
    </>
  );
  const cls = "flex w-full items-center gap-3 rounded-md px-2 py-1.5 text-left text-sm";
  const tip = label
    ? `${child.name}: ${label.text}`
    : `${child.name}: ${formatBytes(child.totalBytes)} in ${count(child.fileCount)} files`;
  return (
    <li>
      {openable ? (
        <button type="button" onClick={onOpen} title={tip} className={`${cls} hover:bg-slate-100 dark:hover:bg-slate-800`}>
          {content}
        </button>
      ) : (
        <div className={cls} title={tip}>
          {content}
        </div>
      )}
    </li>
  );
}

function FolderExplorer({ scanId }: { scanId: number }) {
  const [node, setNode] = useState(0);
  const [listing, setListing] = useState<DirListing | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc.scanListing(scanId, node).then(
      (l) => {
        if (!cancelled) {
          setListing(l);
          setError(null);
        }
      },
      (err: unknown) => {
        if (!cancelled) setError(describeError(err));
      },
    );
    return () => {
      cancelled = true;
    };
  }, [node, scanId]);

  if (error) return <ErrorNote message={error} />;
  if (!listing) return <Loading />;
  return (
    <div>
      <nav aria-label="Folder path" className="mb-3 flex flex-wrap items-center gap-1 text-sm">
        {listing.crumbs.map((c, i) => {
          const last = i === listing.crumbs.length - 1;
          return (
            <span key={c.id} className="flex items-center gap-1">
              {i > 0 && <ChevronRight className="size-3.5 text-slate-400" aria-hidden />}
              {last ? (
                <span className="font-medium" aria-current="location">
                  {c.name}
                </span>
              ) : (
                <button type="button" onClick={() => setNode(c.id)} className="text-emerald-700 hover:underline dark:text-emerald-400">
                  {c.name}
                </button>
              )}
            </span>
          );
        })}
        <span className="ml-auto tabular-nums text-slate-500 dark:text-slate-400">
          {formatBytes(listing.totalBytes)}
          {listing.comparedTo && listing.previousTotalBytes !== null && (
            <> ({formatDelta(listing.totalBytes - listing.previousTotalBytes)} since {formatDateTime(listing.comparedTo.finishedAtMs)})</>
          )}
        </span>
      </nav>
      <ul>
        {listing.children.map((c) => (
          <Row key={c.id} child={c} parentTotal={listing.totalBytes} compared={listing.comparedTo !== null} onOpen={() => setNode(c.id)} />
        ))}
        {listing.filesHere > 0 && (
          <li className="flex items-center gap-3 px-2 py-1.5 text-sm text-slate-500 dark:text-slate-400">
            <CornerDownRight className="size-4 shrink-0" aria-hidden />
            <span className="flex-1">
              {count(listing.filesHere)} {listing.filesHere === 1 ? "file" : "files"} directly in this folder
            </span>
            <span className="w-20 text-right tabular-nums">{formatBytes(listing.filesBytes)}</span>
            <span className="w-12" />
            {listing.comparedTo && <span className="w-36" />}
            <span className="size-4" />
          </li>
        )}
        {listing.hiddenChildren > 0 && (
          <li className="px-2 py-1.5 text-sm text-slate-500 dark:text-slate-400">
            {count(listing.hiddenChildren)} smaller folders ({formatBytes(listing.hiddenBytes)}) not listed individually
          </li>
        )}
        {listing.children.length === 0 && listing.filesHere === 0 && (
          <li className="px-2 py-1.5 text-sm text-slate-500">This folder is empty.</li>
        )}
      </ul>
    </div>
  );
}

function LargestFiles({ scanId }: { scanId: number }) {
  const [files, setFiles] = useState<LargeFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    ipc.scanLargestFiles(scanId).then(
      (f) => !cancelled && setFiles(f),
      (err: unknown) => !cancelled && setError(describeError(err)),
    );
    return () => {
      cancelled = true;
    };
  }, [scanId]);
  if (error) return <ErrorNote message={error} />;
  if (!files) return <Loading />;
  if (files.length === 0) return <p className="text-sm text-slate-500">No files found.</p>;
  return (
    <ol className="divide-y divide-slate-100 text-sm dark:divide-slate-800">
      {files.slice(0, 25).map((f) => (
        <li key={f.path} className="flex items-center gap-3 py-1.5">
          <File className="size-4 shrink-0 text-slate-400" aria-hidden />
          <span className="flex-1 truncate font-mono text-xs" title={f.path}>
            {f.path}
          </span>
          <span className="w-20 text-right tabular-nums">{formatBytes(f.bytes)}</span>
        </li>
      ))}
    </ol>
  );
}

export function ScanResults({ last }: { last: SavedScan }) {
  return (
    <div className="space-y-4">
      <Panel title={`Analysis of ${last.root}`}>
        <ScanSummary last={last} />
      </Panel>
      <Panel title="Largest folders">
        <FolderExplorer key={last.scanId} scanId={last.scanId} />
      </Panel>
      <Panel title="Largest files">
        <LargestFiles scanId={last.scanId} />
      </Panel>
    </div>
  );
}
