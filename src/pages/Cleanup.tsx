import { Eye, File, Folder, Info, Link2, Lock, ShieldCheck } from "lucide-react";
import { useState } from "react";
import type { Decision } from "../bindings/Decision";
import type { ItemKind } from "../bindings/ItemKind";
import type { Preview } from "../bindings/Preview";
import type { PreviewResponse } from "../bindings/PreviewResponse";
import type { PreviewItem } from "../bindings/PreviewItem";
import type { ProviderInfo } from "../bindings/ProviderInfo";
import type { Risk } from "../bindings/Risk";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { CATEGORY } from "../lib/categories";
import { formatAge, formatBytes, formatDateTime } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

const RISK: Record<Risk, string> = {
  safe: "Safe",
  lowRisk: "Low risk",
  mediumRisk: "Medium risk",
  highRisk: "High risk",
  protected: "Protected",
};

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

function PreviewResult({ response }: { response: PreviewResponse }) {
  const preview: Preview = response.preview;
  const [filter, setFilter] = useState<Filter>("eligible");
  const eligible = preview.items.filter((i) => i.decision.state === "eligible");
  const kept = preview.items.filter((i) => i.decision.state !== "eligible");
  const keptBytes = kept.reduce((s, i) => s + i.bytes, 0);
  const shown: PreviewItem[] = filter === "eligible" ? eligible : filter === "kept" ? kept : preview.items;
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

function ProviderCard({ info }: { info: ProviderInfo }) {
  const [state, setState] = useState<
    { s: "idle" } | { s: "loading" } | { s: "done"; response: PreviewResponse } | { s: "error"; message: string }
  >({ s: "idle" });
  const run = async () => {
    setState({ s: "loading" });
    try {
      setState({ s: "done", response: await ipc.cleanupPreview(info.id) });
    } catch (err) {
      setState({ s: "error", message: describeError(err) });
    }
  };
  return (
    <Panel title={CATEGORY[info.category].label}>
      <div className="flex items-start gap-4">
        <div className="flex-1 space-y-1.5 text-sm">
          <h3 className="text-base font-medium">{info.name}</h3>
          <p className="flex items-center gap-1.5 text-xs text-emerald-700 dark:text-emerald-400">
            <ShieldCheck className="size-3.5" aria-hidden />
            {RISK[info.risk]} · only items unchanged for {info.minAgeDays} days
          </p>
          <p className="text-slate-600 dark:text-slate-300">{info.description}</p>
          <p className="text-slate-500 dark:text-slate-400">{info.onRemoval}</p>
        </div>
        <button
          type="button"
          onClick={() => void run()}
          disabled={state.s === "loading"}
          className="flex shrink-0 items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-40"
        >
          <Eye className="size-4" aria-hidden />
          {state.s === "done" ? "Preview again" : "Preview"}
        </button>
      </div>
      {state.s === "loading" && (
        <div className="mt-4">
          <Loading />
        </div>
      )}
      {state.s === "error" && (
        <div className="mt-4">
          <ErrorNote message={state.message} />
        </div>
      )}
      {state.s === "done" && <PreviewResult response={state.response} />}
    </Panel>
  );
}

export function Cleanup() {
  const providers = useCommand(ipc.cleanupProviders);
  return (
    <>
      <PageHeader title="Cleanup" subtitle="See exactly what a cleanup would remove, and why." />
      <div className="mb-4 flex items-start gap-3 rounded-lg border border-slate-200 bg-white p-4 text-sm dark:border-slate-800 dark:bg-slate-900">
        <Info className="mt-0.5 size-4 shrink-0 text-slate-400" aria-hidden />
        <div className="space-y-1 text-slate-600 dark:text-slate-300">
          <p>
            <span className="font-medium">Preview only.</span> Sentinel cannot remove anything yet. A preview reads
            folder listings and file dates; it never opens, moves or deletes files.
          </p>
          <p className="text-slate-500 dark:text-slate-400">
            When cleanup is available, items will be moved to a quarantine folder on the same drive for 14 days so
            they can be restored. Files in use will be skipped. Which programs or projects an item belongs to is
            not shown yet.
          </p>
        </div>
      </div>
      {providers.status === "loading" && <Loading />}
      {providers.status === "error" && <ErrorNote message={providers.message} />}
      {providers.status === "ok" && (
        <div className="space-y-4">
          {providers.data.map((p) => (
            <ProviderCard key={p.id} info={p} />
          ))}
        </div>
      )}
    </>
  );
}
