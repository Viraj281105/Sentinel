import type { QueryResult } from "../bindings/QueryResult";
import { CATEGORY } from "../lib/categories";
import { formatAge, formatBytes, formatDateTime } from "../lib/format";

/** The results of a structured query, with what could not be answered shown first. */
export function QueryResultView({ result }: { result: QueryResult }) {
  const noun = result.target === "folders" ? "folder" : "file";
  return (
    <div className="space-y-3 text-sm">
      {result.notApplied.map((n) => (
        <p
          key={n.field}
          role="note"
          className="rounded-md border border-amber-300 bg-amber-50 p-2 text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200"
        >
          Not applied: {n.reason}
        </p>
      ))}
      <p>
        <strong>{result.matches.toLocaleString()}</strong> {noun}
        {result.matches === 1 ? "" : "s"} matched, {formatBytes(result.uniqueBytes)} in total
        {result.moreThanShown && ` (showing the first ${result.items.length})`}. From the analysis of{" "}
        <span className="font-mono text-xs">{result.root}</span> finished {formatDateTime(result.finishedAtMs)}.
      </p>
      {result.items.length > 0 && (
        <ul className="divide-y divide-slate-100 dark:divide-slate-800">
          {result.items.map((i) => (
            <li key={i.path} className="flex items-baseline gap-3 py-1.5">
              <span className="flex-1 truncate font-mono text-xs" title={i.path}>
                {i.path}
              </span>
              {i.classification && (
                <span className="text-xs text-slate-500" title={i.classification.reason}>
                  {CATEGORY[i.classification.category].label}
                </span>
              )}
              {i.lastChangedMs !== null && (
                <span className="text-xs text-slate-500">changed {formatAge(i.lastChangedMs, result.finishedAtMs)}</span>
              )}
              <span className="w-24 text-right tabular-nums">{formatBytes(i.bytes)}</span>
            </li>
          ))}
        </ul>
      )}
      {result.notes.length > 0 && (
        <ul className="list-disc space-y-1 pl-5 text-xs text-slate-500 dark:text-slate-400">
          {result.notes.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
