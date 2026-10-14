import { useState } from "react";
import type { QueryResult } from "../bindings/QueryResult";
import { formatBytes } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { QueryResultView } from "./QueryResultView";
import { ErrorNote } from "./ui";

const AGES = [30, 90, 180, 365];
const SIZES = [10 * 1024 ** 2, 100 * 1024 ** 2, 1024 ** 3];

/**
 * Folders nothing has changed in for a while, from the saved analysis. No AI involved: the
 * form builds the same structured query the assistant would, and it is read-only.
 */
export function StaleFolders({ scanId }: { scanId: number }) {
  const [days, setDays] = useState(90);
  const [minBytes, setMinBytes] = useState(SIZES[1]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<QueryResult | null>(null);

  async function show() {
    setBusy(true);
    setError(null);
    try {
      const query = { target: "folders", olderThanDays: days, minBytes, sort: "sizeDesc", limit: 50 };
      setResult(await ipc.querySavedAnalysis(scanId, JSON.stringify(query)));
    } catch (e) {
      setResult(null);
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const select = "rounded-md border border-slate-300 bg-transparent px-2 py-1 dark:border-slate-700";
  return (
    <div className="space-y-3">
      <p className="text-sm text-slate-500 dark:text-slate-400">
        Folders where nothing inside has changed for a while, as of this analysis. This only lists them;
        a folder that has not changed is not necessarily unneeded.
      </p>
      <form
        className="flex flex-wrap items-center gap-3 text-sm"
        onSubmit={(e) => {
          e.preventDefault();
          if (!busy) void show();
        }}
      >
        <label className="flex items-center gap-2">
          Not changed for
          <select className={select} value={days} onChange={(e) => setDays(Number(e.target.value))}>
            {AGES.map((d) => (
              <option key={d} value={d}>
                {d} days
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-2">
          and at least
          <select className={select} value={minBytes} onChange={(e) => setMinBytes(Number(e.target.value))}>
            {SIZES.map((b) => (
              <option key={b} value={b}>
                {formatBytes(b)}
              </option>
            ))}
          </select>
        </label>
        <button
          type="submit"
          disabled={busy}
          className="rounded-md border border-slate-300 px-3 py-1.5 hover:bg-slate-100 disabled:opacity-40 dark:border-slate-700 dark:hover:bg-slate-800"
        >
          Show folders
        </button>
      </form>
      {error && <ErrorNote message={error} />}
      {result && <QueryResultView result={result} outermostOnly />}
    </div>
  );
}
