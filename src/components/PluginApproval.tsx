import { ShieldCheck } from "lucide-react";
import type { Inspection } from "../bindings/Inspection";
import { formatBytes } from "../lib/format";
import { RISK } from "../lib/risk";

/** Text the plugin wrote, set apart so it is never mistaken for Sentinel's own words. */
function ByPlugin({ children }: { children: string }) {
  return (
    <blockquote className="border-l-2 border-violet-300 pl-3 text-slate-700 italic dark:border-violet-700 dark:text-slate-300">
      {children}
      <span className="ml-2 text-xs text-slate-400 not-italic">(written by the plugin)</span>
    </blockquote>
  );
}

/**
 * Everything a person needs to decide: who wrote it, exactly which folders on this computer it
 * points at, what it would offer right now, and a fingerprint of the file being approved.
 */
export function PluginApproval({
  inspection,
  busy,
  onApprove,
  onCancel,
}: {
  inspection: Inspection;
  busy: boolean;
  onApprove: () => void;
  onCancel: () => void;
}) {
  const p = inspection.plugin;
  return (
    <div className="space-y-4 text-sm">
      <p className="flex items-start gap-2 rounded-md bg-slate-100 p-3 dark:bg-slate-800">
        <ShieldCheck className="mt-0.5 size-4 shrink-0 text-emerald-600" aria-hidden />
        <span>
          This plugin can only ask Sentinel to offer the folders below for cleanup. It cannot run code, use the
          network, or remove anything without your confirmation each time. Protected locations stay protected.
        </span>
      </p>

      <dl className="grid grid-cols-[9rem_1fr] gap-y-1">
        <dt className="text-slate-500">Name</dt>
        <dd className="font-medium">{p.name}</dd>
        <dt className="text-slate-500">Author</dt>
        <dd>{p.author}</dd>
        <dt className="text-slate-500">Version</dt>
        <dd>{p.version}</dd>
        <dt className="text-slate-500">Identifier</dt>
        <dd className="font-mono text-xs">{p.id}</dd>
        <dt className="text-slate-500">Fingerprint (SHA-256)</dt>
        <dd className="font-mono text-xs break-all">{p.sha256}</dd>
      </dl>
      <ByPlugin>{p.description}</ByPlugin>

      {inspection.alreadyInstalled && (
        <p role="note" className="rounded-md border border-slate-300 p-2 dark:border-slate-700">
          This exact plugin is already installed. There is nothing to approve.
        </p>
      )}
      {inspection.replaces && !inspection.alreadyInstalled && (
        <p
          role="note"
          className="rounded-md border border-amber-300 bg-amber-50 p-2 text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200"
        >
          This is a different file from the version {inspection.replaces.version} you have installed (fingerprint{" "}
          <span className="font-mono text-xs">{inspection.replaces.sha256.slice(0, 12)}…</span>). Approving replaces
          it. Compare the folders and risks below with what you approved before.
        </p>
      )}

      {p.providers.map((prov, i) => {
        const run = inspection.dryRuns[i];
        return (
          <section key={prov.id} aria-label={prov.name} className="space-y-2 rounded-md border border-slate-200 p-3 dark:border-slate-800">
            <h3 className="font-medium">{prov.name}</h3>
            <ByPlugin>{prov.description}</ByPlugin>
            <dl className="grid grid-cols-[9rem_1fr] gap-y-1">
              <dt className="text-slate-500">Looks in</dt>
              <dd className="font-mono text-xs break-all">{prov.root ?? "(unavailable on this computer)"}</dd>
              <dt className="text-slate-500">Offers items named</dt>
              <dd className="font-mono text-xs">{prov.patterns.join(", ")}</dd>
              <dt className="text-slate-500">Risk (limited by Sentinel)</dt>
              <dd>{RISK[prov.risk]}</dd>
              <dt className="text-slate-500">Only items unchanged for</dt>
              <dd>{prov.minAgeDays} days</dd>
              <dt className="text-slate-500">If removed, it says</dt>
              <dd>
                <ByPlugin>{prov.onRemoval}</ByPlugin>
              </dd>
            </dl>
            {run && (
              <div className="space-y-1">
                <p>
                  <span className="font-medium">Right now</span> ({run.root}):{" "}
                  {run.eligibleItems.toLocaleString()} {run.eligibleItems === 1 ? "item" : "items"},{" "}
                  {formatBytes(run.eligibleBytes)} would be offered. Nothing is moved by approving.
                </p>
                {run.items.length > 0 && (
                  <ul className="divide-y divide-slate-100 text-xs dark:divide-slate-800">
                    {run.items.map((it) => (
                      <li key={it.path} className="flex gap-3 py-1">
                        <span className="w-0 flex-1 truncate font-mono" title={it.path}>
                          {it.path}
                        </span>
                        <span className="w-64 truncate text-slate-500" title={it.verdict}>
                          {it.verdict}
                        </span>
                        <span className="w-20 text-right tabular-nums">{formatBytes(it.bytes)}</span>
                      </li>
                    ))}
                    {run.moreItems > 0 && (
                      <li className="py-1 text-slate-500">{run.moreItems.toLocaleString()} more not shown</li>
                    )}
                  </ul>
                )}
              </div>
            )}
          </section>
        );
      })}

      <div className="flex justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          className="rounded-md border border-slate-300 px-3 py-1.5 hover:bg-slate-100 disabled:opacity-40 dark:border-slate-700 dark:hover:bg-slate-800"
        >
          Cancel
        </button>
        <button
          type="button"
          onClick={onApprove}
          disabled={busy || inspection.alreadyInstalled}
          className="rounded-md bg-emerald-600 px-3 py-1.5 font-medium text-white hover:bg-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {inspection.replaces ? "Approve and replace" : "Approve this plugin"}
        </button>
      </div>
    </div>
  );
}
