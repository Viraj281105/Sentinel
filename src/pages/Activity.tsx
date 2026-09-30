import { AlertTriangle, Eye, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import type { AuditEntry } from "../bindings/AuditEntry";
import type { AuditKind } from "../bindings/AuditKind";
import type { Outcome } from "../bindings/Outcome";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { formatBytes, formatDateTime } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

const PAGE = 50;

const KIND: Record<AuditKind, string> = {
  preview: "Dry run",
  quarantine: "Moved to quarantine",
  restore: "Restored from quarantine",
  purge: "Expired quarantine removed",
};

const OUTCOME: Record<Outcome, string> = {
  started: "Started",
  noChanges: "Nothing changed",
  succeeded: "Succeeded",
  partiallySucceeded: "Partly succeeded",
  failed: "Failed",
};

function summary(e: AuditEntry): string {
  const what = `${e.items.toLocaleString()} ${e.items === 1 ? "item" : "items"} (${formatBytes(e.bytes)})`;
  return e.kind === "preview" ? `${what} would be moved to quarantine` : what;
}

function Integrity() {
  const check = useCommand(ipc.auditVerify);
  if (check.status === "loading") return <Loading />;
  if (check.status === "error") return <ErrorNote message={check.message} />;
  const v = check.data;
  if (v.intact) {
    return (
      <p className="flex items-center gap-2 text-sm text-emerald-700 dark:text-emerald-400">
        <ShieldCheck className="size-4 shrink-0" aria-hidden />
        History verified: all {v.records.toLocaleString()} entries are intact and in order.
      </p>
    );
  }
  return (
    <p role="alert" className="flex items-start gap-2 text-sm text-red-700 dark:text-red-400">
      <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
      History has been altered: entry {v.firstBadSeq} {v.problem}. Entries from that point on cannot be trusted.
    </p>
  );
}

export function Activity() {
  const providers = useCommand(ipc.cleanupProviders);
  const [entries, setEntries] = useState<AuditEntry[] | null>(null);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc.auditLog(PAGE, null).then(
      (page) => {
        if (cancelled) return;
        setEntries(page);
        setMore(page.length === PAGE);
      },
      (err: unknown) => {
        if (!cancelled) setError(describeError(err));
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const loadOlder = useCallback(async (before: number) => {
    try {
      const page = await ipc.auditLog(PAGE, before);
      setEntries((prev) => [...(prev ?? []), ...page]);
      setMore(page.length === PAGE);
    } catch (err) {
      setError(describeError(err));
    }
  }, []);

  const providerName = (id: string | null) =>
    id === null
      ? null
      : providers.status === "ok"
        ? (providers.data.find((p) => p.id === id)?.name ?? id)
        : id;

  return (
    <>
      <PageHeader
        title="Activity"
        subtitle="Every cleanup operation, recorded in a log that cannot be edited without detection."
      />
      <div className="space-y-4">
        <Panel title="Integrity">
          <Integrity />
        </Panel>
        <Panel title="History">
          {error && <ErrorNote message={error} />}
          {!error && entries === null && <Loading />}
          {entries?.length === 0 && (
            <p className="text-sm text-slate-500 dark:text-slate-400">
              Nothing recorded yet. Previews on the Cleanup page are recorded here.
            </p>
          )}
          {entries && entries.length > 0 && (
            <ol className="divide-y divide-slate-100 dark:divide-slate-800">
              {entries.map((e) => (
                <li key={e.seq} className="flex gap-3 py-3 text-sm">
                  <Eye className="mt-0.5 size-4 shrink-0 text-slate-400" aria-hidden />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <p>
                      <span className="font-medium">{KIND[e.kind]}</span>
                      {e.provider && <> · {providerName(e.provider)}</>}
                      <span className="text-slate-500 dark:text-slate-400"> · {OUTCOME[e.outcome]}</span>
                    </p>
                    <p>{summary(e)}</p>
                    <p className="text-xs text-slate-500 dark:text-slate-400">{e.policy}</p>
                    {e.errors.map((err) => (
                      <p key={err} className="text-xs text-amber-700 dark:text-amber-300">
                        {err}
                      </p>
                    ))}
                    <p className="text-xs text-slate-500 dark:text-slate-400">
                      {formatDateTime(e.atMs)} · {e.user} · entry {e.seq} ·{" "}
                      <span className="font-mono" title={`Operation ${e.operationId}, hash ${e.hash}`}>
                        {e.operationId.slice(0, 11)}
                      </span>
                    </p>
                  </div>
                </li>
              ))}
            </ol>
          )}
          {more && entries && entries.length > 0 && (
            <button
              type="button"
              onClick={() => void loadOlder(entries[entries.length - 1]?.seq ?? 0)}
              className="mt-3 rounded-md border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-100 dark:border-slate-700 dark:hover:bg-slate-800"
            >
              Load older entries
            </button>
          )}
        </Panel>
      </div>
    </>
  );
}
