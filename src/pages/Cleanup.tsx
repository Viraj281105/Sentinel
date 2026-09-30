import { Eye, Info, ShieldCheck } from "lucide-react";
import { useState } from "react";
import type { PreviewResponse } from "../bindings/PreviewResponse";
import type { ProviderInfo } from "../bindings/ProviderInfo";
import type { Risk } from "../bindings/Risk";
import { PreviewResult } from "../components/PreviewResult";
import { QuarantinePanel } from "../components/QuarantinePanel";
import { ErrorNote, Loading, PageHeader, Panel } from "../components/ui";
import { CATEGORY } from "../lib/categories";
import { describeError, ipc } from "../lib/ipc";
import { useCommand } from "../lib/useCommand";

const RISK: Record<Risk, string> = {
  safe: "Safe",
  lowRisk: "Low risk",
  mediumRisk: "Medium risk",
  highRisk: "High risk",
  protected: "Protected",
};

function ProviderCard({ info, onMoved }: { info: ProviderInfo; onMoved: () => void }) {
  const [state, setState] = useState<
    { s: "idle" } | { s: "loading" } | { s: "done"; response: PreviewResponse } | { s: "error"; message: string }
  >({ s: "idle" });
  const run = async () => {
    setState({ s: "loading" });
    try {
      setState({ s: "done", response: await ipc.cleanupPreview(info.id) });
    } catch (err) {
      setState({ s: "error", message: describeError(err) });
    }
  };
  return (
    <Panel title={CATEGORY[info.category].label}>
      <div className="flex items-start gap-4">
        <div className="flex-1 space-y-1.5 text-sm">
          <h3 className="text-base font-medium">{info.name}</h3>
          <p className="flex items-center gap-1.5 text-xs text-emerald-700 dark:text-emerald-400">
            <ShieldCheck className="size-3.5" aria-hidden />
            {RISK[info.risk]} · only items unchanged for {info.minAgeDays} days
          </p>
          <p className="text-slate-600 dark:text-slate-300">{info.description}</p>
          <p className="text-slate-500 dark:text-slate-400">{info.onRemoval}</p>
          {info.note && (
            <p className="text-amber-700 dark:text-amber-300">
              {info.canClean ? "" : "Analysis only. "}
              {info.note}
            </p>
          )}
        </div>
        <button
          type="button"
          onClick={() => void run()}
          disabled={state.s === "loading"}
          className="flex shrink-0 items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-700 disabled:opacity-40"
        >
          <Eye className="size-4" aria-hidden />
          {state.s === "done" ? "Preview again" : "Preview"}
        </button>
      </div>
      {state.s === "loading" && (
        <div className="mt-4">
          <Loading />
        </div>
      )}
      {state.s === "error" && (
        <div className="mt-4">
          <ErrorNote message={state.message} />
        </div>
      )}
      {state.s === "done" && (
        <PreviewResult
          key={state.response.operationId}
          response={state.response}
          providerName={info.name}
          onRun={(paths) => ipc.cleanupRun(info.id, paths)}
          onMoved={onMoved}
        />
      )}
    </Panel>
  );
}

export function Cleanup() {
  const providers = useCommand(ipc.cleanupProviders);
  const [version, setVersion] = useState(0);
  return (
    <>
      <PageHeader title="Cleanup" subtitle="See exactly what a cleanup would remove, and why." />
      <div className="mb-4 flex items-start gap-3 rounded-lg border border-slate-200 bg-white p-4 text-sm dark:border-slate-800 dark:bg-slate-900">
        <Info className="mt-0.5 size-4 shrink-0 text-slate-400" aria-hidden />
        <div className="space-y-1 text-slate-600 dark:text-slate-300">
          <p>
            <span className="font-medium">Nothing is removed without your confirmation.</span> A preview only reads
            folder listings and file dates. When you confirm, items move to a private quarantine folder on the same
            drive, where you can restore them for 14 days; after that they are deleted permanently.
          </p>
          <p className="text-slate-500 dark:text-slate-400">
            Files in use are skipped. Which programs or projects an item belongs to is not shown yet.
          </p>
        </div>
      </div>
      {providers.status === "loading" && <Loading />}
      {providers.status === "error" && <ErrorNote message={providers.message} />}
      {providers.status === "ok" && (
        <div className="space-y-4">
          {providers.data.map((p) => (
            <ProviderCard key={p.id} info={p} onMoved={() => setVersion((v) => v + 1)} />
          ))}
        </div>
      )}
      <div className="mt-4">
        <QuarantinePanel version={version} />
      </div>
    </>
  );
}
