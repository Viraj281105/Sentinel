import { Archive, Loader2, RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";
import type { QuarantineContents } from "../bindings/QuarantineContents";
import { formatBytes, formatDate, formatDateTime } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { ErrorNote, Loading, Panel } from "./ui";

/** What is in quarantine, with Restore. `version` changes trigger a reload. */
export function QuarantinePanel({ version }: { version: number }) {
  const [contents, setContents] = useState<QuarantineContents | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let cancelled = false;
    ipc.quarantineContents().then(
      (c) => {
        if (!cancelled) {
          setContents(c);
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
  }, [version, reload]);

  const restore = async (operationId: string, index: number) => {
    const key = `${operationId}/${index}`;
    setBusy(key);
    try {
      await ipc.quarantineRestore(operationId, index);
      setReload((r) => r + 1);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  };

  const ops = (contents?.operations ?? []).filter((o) => o.entries.some((e) => e.status.state === "quarantined"));

  return (
    <Panel title="Quarantine">
      {!contents && !error && <Loading />}
      {error && <ErrorNote message={error} />}
      {contents && ops.length === 0 && (
        <p className="text-sm text-slate-500 dark:text-slate-400">Nothing is in quarantine.</p>
      )}
      {contents?.problems.map((p) => (
        <p key={p} className="text-xs text-amber-700 dark:text-amber-300">
          {p}
        </p>
      ))}
      <div className="space-y-4">
        {ops.map((op) => (
          <div key={op.operationId}>
            <p className="flex items-center gap-2 text-sm">
              <Archive className="size-4 text-slate-400" aria-hidden />
              Moved {formatDateTime(op.createdAtMs)}
              <span className="text-slate-500 dark:text-slate-400">
                · kept until {formatDate(op.expiresAtMs)}, then deleted permanently
              </span>
            </p>
            <ul className="mt-1 divide-y divide-slate-100 text-sm dark:divide-slate-800">
              {op.entries
                .filter((e) => e.status.state === "quarantined" || e.status.state === "restored")
                .map((e) => {
                  const key = `${op.operationId}/${e.index}`;
                  return (
                    <li key={key} className="flex items-center gap-3 py-1.5 pl-6">
                      <span className="w-0 flex-1 truncate font-mono text-xs" title={e.originalPath}>
                        {e.originalPath}
                      </span>
                      <span className="w-20 text-right tabular-nums">{formatBytes(e.bytes)}</span>
                      {e.status.state === "quarantined" ? (
                        <button
                          type="button"
                          onClick={() => void restore(op.operationId, e.index)}
                          disabled={busy !== null}
                          aria-label={`Restore ${e.originalPath}`}
                          className="flex w-24 items-center justify-center gap-1 rounded-md border border-slate-300 px-2 py-1 text-xs hover:bg-slate-100 disabled:opacity-40 dark:border-slate-700 dark:hover:bg-slate-800"
                        >
                          {busy === key ? (
                            <Loader2 className="size-3.5 animate-spin" aria-hidden />
                          ) : (
                            <RotateCcw className="size-3.5" aria-hidden />
                          )}
                          Restore
                        </button>
                      ) : (
                        <span className="w-24 text-center text-xs text-slate-500">Restored</span>
                      )}
                    </li>
                  );
                })}
            </ul>
          </div>
        ))}
      </div>
      {contents && (
        <p className="mt-3 text-xs text-slate-500 dark:text-slate-400" title={contents.root}>
          Stored in {contents.root}
        </p>
      )}
    </Panel>
  );
}
