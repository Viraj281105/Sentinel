import { Coffee, Loader2 } from "lucide-react";
import { useState } from "react";
import type { ArtifactCache } from "../bindings/ArtifactCache";
import type { JvmAnalysis } from "../bindings/JvmAnalysis";
import { formatBytes, formatDate } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { ErrorNote, Panel } from "./ui";

const leaf = (path: string) => path.split("\\").filter(Boolean).pop() ?? path;
const SHOWN = 15;

function Repository({ title, cache }: { title: string; cache: ArtifactCache }) {
  return (
    <div className="space-y-2">
      <h3 className="text-sm font-medium">{title}</h3>
      <p className="text-sm">
        <span className="font-semibold tabular-nums">{formatBytes(cache.totalBytes)}</span> in{" "}
        {cache.artifactVersions.toLocaleString()} artifact versions.{" "}
        {cache.multiVersionArtifacts > 0 && (
          <>
            {cache.multiVersionArtifacts} artifacts have more than one version cached;{" "}
            <span className="tabular-nums">{formatBytes(cache.olderVersionsBytes)}</span> is in versions other than the
            most recently downloaded.{" "}
          </>
        )}
        {cache.failedDownloads > 0 && `${cache.failedDownloads} failed downloads left markers behind. `}
        {cache.truncated && "The analysis stopped at a safety limit, so figures are incomplete."}
      </p>
      <p className="text-xs text-slate-500 dark:text-slate-400" title={cache.root}>
        {cache.root}
      </p>
      <div className="grid gap-4 md:grid-cols-2">
        <div>
          <p className="mb-1 text-xs text-slate-500 dark:text-slate-400">Largest groups</p>
          <ul className="text-sm">
            {cache.groups.slice(0, 8).map((g) => (
              <li key={g.group} className="flex gap-2">
                <span className="w-0 flex-1 truncate font-mono text-xs" title={g.group}>
                  {g.group}
                </span>
                <span className="text-xs text-slate-500">{g.versions} versions</span>
                <span className="w-20 text-right tabular-nums">{formatBytes(g.bytes)}</span>
              </li>
            ))}
          </ul>
        </div>
        <div>
          <p className="mb-1 text-xs text-slate-500 dark:text-slate-400">Largest artifacts</p>
          <ul className="text-sm">
            {cache.artifacts.slice(0, SHOWN).map((a) => {
              const declared = a.versions.flatMap((v) => v.declaredBy.map((p) => `${v.version} by ${leaf(p)}`));
              return (
                <li key={`${a.group}:${a.artifact}`} className="py-0.5">
                  <span className="flex gap-2">
                    <span className="w-0 flex-1 truncate font-mono text-xs" title={`${a.group}:${a.artifact}`}>
                      {a.artifact}
                    </span>
                    <span className="text-xs text-slate-500">
                      {a.versions.length > 1 ? `${a.versions.length} versions` : a.versions[0]?.version}
                    </span>
                    <span className="w-20 text-right tabular-nums">{formatBytes(a.bytes)}</span>
                  </span>
                  {declared.length > 0 && (
                    <span className="block text-xs text-emerald-700 dark:text-emerald-400">
                      Declared: {declared.join(", ")}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      </div>
    </div>
  );
}

function Results({ a, at }: { a: JvmAnalysis; at: number }) {
  return (
    <div className="mt-4 space-y-5 border-t border-slate-200 pt-4 dark:border-slate-800">
      <p className="text-xs text-slate-500 dark:text-slate-400">
        Maven and Gradle do not record when a cached file was last used, so dates are download dates. Most cached files
        are dependencies of dependencies, which only a build can resolve; "declared" lists only exact versions your
        projects name directly, so an artifact without it may still be in use. Read {a.projectsRead} Java project
        {a.projectsRead === 1 ? "" : "s"} from your searched folders.
      </p>
      {a.maven ? <Repository title="Maven local repository" cache={a.maven} /> : <p className="text-sm">No Maven local repository.</p>}
      {a.gradle ? (
        <div className="space-y-2">
          <h3 className="text-sm font-medium">Gradle</h3>
          <p className="text-sm">
            <span className="font-semibold tabular-nums">{formatBytes(a.gradle.totalBytes)}</span> in {a.gradle.root}
          </p>
          {a.gradle.versions.length > 0 && (
            <ul className="text-sm">
              {a.gradle.versions.map((v) => (
                <li key={v.path} className="flex gap-2" title={v.path}>
                  <span className="w-32">Gradle {v.version}</span>
                  <span className="flex-1 text-xs text-slate-500 dark:text-slate-400">
                    {v.what} ·{" "}
                    {v.usedBy.length > 0
                      ? `used by ${v.usedBy.map(leaf).join(", ")}`
                      : "no searched project's wrapper uses this version"}
                  </span>
                  <span className="w-20 text-right tabular-nums">{formatBytes(v.bytes)}</span>
                </li>
              ))}
            </ul>
          )}
          {a.gradle.modules && <Repository title="Gradle dependency cache" cache={a.gradle.modules} />}
        </div>
      ) : (
        <p className="text-sm">No Gradle home.</p>
      )}
      {a.warnings.map((w) => (
        <p key={w} className="text-xs text-amber-700 dark:text-amber-300">
          {w}
        </p>
      ))}
      <p className="text-xs text-slate-500 dark:text-slate-400">
        Analysis only: Sentinel does not clean these caches. Analyzed {formatDate(at)}.
      </p>
    </div>
  );
}

/** Read-only analysis of Maven and Gradle caches. */
export function JvmCaches() {
  const [state, setState] = useState<
    { s: "idle" } | { s: "loading" } | { s: "done"; a: JvmAnalysis; at: number } | { s: "error"; message: string }
  >({ s: "idle" });
  const run = async () => {
    setState({ s: "loading" });
    try {
      const a = await ipc.jvmCaches();
      setState({ s: "done", a, at: Date.now() });
    } catch (err) {
      setState({ s: "error", message: describeError(err) });
    }
  };
  return (
    <Panel title="Java build caches">
      <div className="flex items-start gap-4">
        <p className="flex-1 text-sm text-slate-600 dark:text-slate-300">
          What Maven and Gradle keep in your user folder, which versions are cached, and which Gradle versions your
          projects still use. Nothing is changed.
        </p>
        <button
          type="button"
          onClick={() => void run()}
          disabled={state.s === "loading"}
          className="flex shrink-0 items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-40"
        >
          {state.s === "loading" ? (
            <Loader2 className="size-4 animate-spin" aria-hidden />
          ) : (
            <Coffee className="size-4" aria-hidden />
          )}
          {state.s === "done" ? "Analyze again" : "Analyze"}
        </button>
      </div>
      {state.s === "error" && (
        <div className="mt-3">
          <ErrorNote message={state.message} />
        </div>
      )}
      {state.s === "done" && <Results a={state.a} at={state.at} />}
    </Panel>
  );
}
