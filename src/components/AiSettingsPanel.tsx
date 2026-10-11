import { useEffect, useState } from "react";
import type { AiSettings } from "../bindings/AiSettings";
import { describeError, ipc } from "../lib/ipc";
import { ErrorNote, Loading, Panel } from "./ui";

/** The one place the AI assistant is switched on. Off until someone does it here. */
export function AiSettingsPanel() {
  const [saved, setSaved] = useState<AiSettings | null>(null);
  const [draft, setDraft] = useState<AiSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    ipc.aiSettings().then(
      (s) => {
        if (cancelled) return;
        setSaved(s);
        setDraft(s);
      },
      (e: unknown) => {
        if (!cancelled) setError(describeError(e));
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  async function save() {
    if (!draft) return;
    setBusy(true);
    setError(null);
    try {
      const s = await ipc.setAiSettings(draft);
      setSaved(s);
      setDraft(s);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const changed = saved !== null && draft !== null && JSON.stringify(saved) !== JSON.stringify(draft);

  return (
    <Panel title="AI assistant (optional)">
      <p className="mb-3 text-sm text-slate-500 dark:text-slate-400">
        Lets you ask questions about a saved analysis in your own words. It uses a model running on
        this computer (Ollama); nothing is sent over the internet and addresses that are not on this
        computer are refused. Only your typed question is sent, with names and paths hidden first. The
        model cannot change files: its answer is a search that Sentinel checks and runs read-only.
        Everything else in Sentinel works without it.
      </p>
      {!draft && !error && <Loading />}
      {draft && (
        <form
          className="space-y-3 text-sm"
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={draft.enabled}
              onChange={(e) => setDraft({ ...draft, enabled: e.target.checked })}
            />
            <span>Turn on the AI assistant</span>
          </label>
          <label className="grid grid-cols-[8rem_1fr] items-center gap-2">
            <span className="text-slate-500">Local model</span>
            <input
              value={draft.model}
              onChange={(e) => setDraft({ ...draft, model: e.target.value })}
              className="rounded-md border border-slate-300 bg-transparent px-2 py-1 font-mono text-xs dark:border-slate-700"
            />
          </label>
          <label className="grid grid-cols-[8rem_1fr] items-center gap-2">
            <span className="text-slate-500">Ollama address</span>
            <input
              value={draft.baseUrl}
              onChange={(e) => setDraft({ ...draft, baseUrl: e.target.value })}
              className="rounded-md border border-slate-300 bg-transparent px-2 py-1 font-mono text-xs dark:border-slate-700"
            />
          </label>
          <button
            type="submit"
            disabled={busy || !changed}
            className="rounded-md bg-emerald-600 px-3 py-1.5 font-medium text-white hover:bg-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
          >
            Save
          </button>
        </form>
      )}
      {error && <ErrorNote message={error} />}
    </Panel>
  );
}
