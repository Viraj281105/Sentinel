import { Sparkles } from "lucide-react";
import { useState } from "react";
import type { AskResponse } from "../bindings/AskResponse";
import { QueryResultView } from "../components/QueryResultView";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { describeError, ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

export function Assistant() {
  const settings = useCommand(ipc.aiSettings);
  const [question, setQuestion] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [answer, setAnswer] = useState<AskResponse | null>(null);

  async function ask() {
    setBusy(true);
    setError(null);
    setAnswer(null);
    try {
      setAnswer(await ipc.askAssistant(question));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <PageHeader
        title="AI Assistant"
        subtitle="Ask about a saved analysis in your own words. Optional, advisory and read-only."
      />
      {settings.status === "loading" && <Loading />}
      {settings.status === "error" && <ErrorNote message={settings.message} />}
      {settings.status === "ok" && !settings.data.enabled && (
        <Panel title="Turned off">
          <p className="text-sm text-slate-500 dark:text-slate-400">
            The AI assistant is off, and nothing in Sentinel needs it. You can turn it on in Settings; it
            uses a model on this computer only.
          </p>
        </Panel>
      )}
      {settings.status === "ok" && settings.data.enabled && (
        <div className="space-y-4">
          <Panel title="Ask a question">
            <form
              className="flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                if (question.trim() && !busy) void ask();
              }}
            >
              <input
                aria-label="Your question"
                value={question}
                maxLength={500}
                placeholder="For example: old developer dependency folders over 1 GB"
                onChange={(e) => setQuestion(e.target.value)}
                className="flex-1 rounded-md border border-slate-300 bg-transparent px-3 py-1.5 text-sm dark:border-slate-700"
              />
              <button
                type="submit"
                disabled={busy || !question.trim()}
                className="flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
              >
                <Sparkles className="size-4" aria-hidden />
                Ask
              </button>
            </form>
            {busy && <p className="mt-3 text-sm text-slate-400">Asking the local model…</p>}
            {error && (
              <div className="mt-3">
                <ErrorNote message={error} />
              </div>
            )}
          </Panel>
          {answer && (
            <Panel title="AI answered this">
              <div className="mb-4 space-y-2 text-sm">
                <p className="inline-block rounded bg-violet-100 px-2 py-0.5 text-xs font-medium text-violet-800 dark:bg-violet-950 dark:text-violet-200">
                  AI answered this · {answer.provider} · {answer.model}
                </p>
                <p>
                  <span className="text-slate-500">What the model was sent: </span>
                  <span className="font-mono text-xs">{answer.sentQuestion}</span>
                </p>
                <p>
                  <span className="text-slate-500">Sentinel ran this search: </span>
                  {answer.queryDescription}
                </p>
              </div>
              <QueryResultView result={answer.result} />
            </Panel>
          )}
        </div>
      )}
    </>
  );
}
