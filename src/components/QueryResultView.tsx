import type { QueryResult } from "../bindings/QueryResult";
import { CATEGORY } from "../lib/categories";
import { formatAge, formatBytes, formatDateTime } from "../lib/format";

const lower = (p: string) => p.toLowerCase().replace(/[\\/]+$/, "");

/** Items that sit inside another listed item (a folder inside a listed folder). */
function nestedPaths(items: QueryResult["items"]): Set<string> {
  const paths = items.map((i) => lower(i.path));
  const nested = new Set<string>();
  for (const p of paths) {
    if (paths.some((o) => o !== p && p.startsWith(`${o}\\`))) nested.add(p);
  }
  return nested;
}

/**
 * The results of a structured query, with what could not be answered shown first. With
 * `outermostOnly`, an item inside another listed item is not listed again (its bytes are
 * already part of the outer one), so a list of folders is not mostly repeats of one tree.
 */
export function QueryResultView({ result, outermostOnly = false }: { result: QueryResult; outermostOnly?: boolean }) {
  const noun = result.target === "folders" ? "folder" : "file";
  const nested = outermostOnly ? nestedPaths(result.items) : new Set<string>();
  const items = result.items.filter((i) => !nested.has(lower(i.path)));
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
      {nested.size > 0 && (
        <p className="text-xs text-slate-500 dark:text-slate-400">
          Showing the outermost folders only; {nested.size.toLocaleString()} {nested.size === 1 ? "folder" : "folders"}{" "}
          inside them {nested.size === 1 ? "is" : "are"} not listed again, and the total counts each byte once.
        </p>
      )}
      {items.length > 0 && (
        <ul className="divide-y divide-slate-100 dark:divide-slate-800">
          {items.map((i) => (
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
