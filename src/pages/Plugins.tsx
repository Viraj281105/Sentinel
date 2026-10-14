import { open } from "@tauri-apps/plugin-dialog";
import { Puzzle } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import type { Inspection } from "../bindings/Inspection";
import type { PluginSummary } from "../bindings/PluginSummary";
import { PluginApproval } from "../components/PluginApproval";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { formatDateTime, formatDays } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { RISK } from "../lib/risk";

function PluginCard({
  plugin,
  onChange,
  onError,
}: {
  plugin: PluginSummary;
  onChange: () => void;
  onError: (m: string) => void;
}) {
  const [confirmRemove, setConfirmRemove] = useState(false);
  const run = async (what: () => Promise<unknown>) => {
    try {
      await what();
      onChange();
    } catch (e) {
      onError(describeError(e));
    }
  };
  const btn =
    "rounded-md border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 dark:border-slate-700 dark:hover:bg-slate-800";
  return (
    <Panel title={plugin.problem ? "Not in use" : plugin.enabled ? "On" : "Off"}>
      <div className="space-y-2 text-sm">
        <div className="flex items-start gap-3">
          <div className="flex-1">
            <h3 className="text-base font-medium">{plugin.name}</h3>
            <p className="text-slate-500">
              by {plugin.author || "unknown"} · version {plugin.version} · approved{" "}
              {plugin.approvedAtMs ? formatDateTime(plugin.approvedAtMs) : "—"} · fingerprint{" "}
              <span className="font-mono text-xs">{plugin.sha256.slice(0, 12)}…</span>
            </p>
          </div>
          {!plugin.problem && (
            <button
              type="button"
              className={btn}
              onClick={() => void run(() => ipc.pluginSetEnabled(plugin.id, !plugin.enabled))}
            >
              {plugin.enabled ? "Switch off" : "Switch on"}
            </button>
          )}
          {confirmRemove ? (
            <>
              <button
                type="button"
                className="rounded-md bg-red-600 px-3 py-1 text-sm font-medium text-white hover:bg-red-700"
                onClick={() => void run(() => ipc.pluginRemove(plugin.id))}
              >
                Remove
              </button>
              <button type="button" className={btn} onClick={() => setConfirmRemove(false)}>
                Keep
              </button>
            </>
          ) : (
            <button type="button" className={btn} onClick={() => setConfirmRemove(true)}>
              Remove…
            </button>
          )}
        </div>
        {confirmRemove && (
          <p className="text-slate-500">
            Removing a plugin only forgets it. Anything already moved to quarantine stays there and can still be
            restored.
          </p>
        )}
        {plugin.problem && <ErrorNote message={plugin.problem} />}
        <ul className="space-y-1 text-slate-600 dark:text-slate-300">
          {plugin.providers.map((p) => (
            <li key={p.id}>
              <span className="font-medium">{p.name}</span>: <span className="font-mono text-xs">{p.root ?? "—"}</span>{" "}
              · {RISK[p.risk]} · items unchanged for {formatDays(p.minAgeDays)}
            </li>
          ))}
        </ul>
      </div>
    </Panel>
  );
}

export function Plugins() {
  const [plugins, setPlugins] = useState<PluginSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    ipc.pluginsList().then(setPlugins, (e: unknown) => setError(describeError(e)));
  }, []);
  useEffect(load, [load]);

  const choose = async () => {
    setError(null);
    const picked = await open({
      multiple: false,
      title: "Choose a plugin file",
      filters: [{ name: "Sentinel plugin (JSON)", extensions: ["json"] }],
    });
    if (typeof picked !== "string") return;
    setBusy(true);
    try {
      setInspection(await ipc.pluginInspect(picked));
    } catch (e) {
      setInspection(null);
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };

  const approve = async () => {
    if (!inspection) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.pluginInstall(inspection.plugin.sha256);
      setInspection(null);
      load();
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHeader
        title="Plugins"
        subtitle="Teach Sentinel about another tool's cache folders. A plugin is a data file, never code."
      />
      <div className="space-y-4">
        {error && <ErrorNote message={error} />}
        {inspection ? (
          <Panel title="Review before approving">
            <PluginApproval
              inspection={inspection}
              busy={busy}
              onApprove={() => void approve()}
              onCancel={() => setInspection(null)}
            />
          </Panel>
        ) : (
          <Panel title="Add a plugin">
            <p className="mb-3 text-sm text-slate-500 dark:text-slate-400">
              Choosing a file only reads it and shows what it would do; nothing is installed until you approve it on the
              next screen. A plugin cannot lower any protection, risk or age limit, and anything it offers still needs
              your confirmation before it is moved.
            </p>
            <button
              type="button"
              disabled={busy}
              onClick={() => void choose()}
              className="flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-40"
            >
              <Puzzle className="size-4" aria-hidden />
              Choose a plugin file…
            </button>
          </Panel>
        )}
        {plugins === null && !error && <Loading />}
        {plugins?.length === 0 && !inspection && (
          <p className="text-sm text-slate-500">No plugins are installed. Sentinel works fully without any.</p>
        )}
        {plugins?.map((p) => (
          <PluginCard key={p.id} plugin={p} onChange={load} onError={setError} />
        ))}
      </div>
    </>
  );
}
