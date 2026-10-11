import { Archive } from "lucide-react";
import { useState } from "react";
import type { PlanRunResponse } from "../bindings/PlanRunResponse";
import type { Risk } from "../bindings/Risk";
import type { RecoveryPlan } from "../bindings/RecoveryPlan";
import { formatBytes } from "../lib/format";
import { describeError, ipc } from "../lib/ipc";
import { RISK } from "../lib/risk";
import { ConfirmMove } from "./ConfirmMove";
import { RunResult } from "./PreviewResult";
import { ErrorNote, Panel } from "./ui";

const UNITS = { MB: 1024 ** 2, GB: 1024 ** 3 } as const;
type Unit = keyof typeof UNITS;
/** Plans stop at medium risk: higher-risk things are never offered here. */
const CEILINGS: Risk[] = ["safe", "lowRisk", "mediumRisk"];

/**
 * "Free up about N": a proposal drawn from what the cleanup types below would offer, lowest
 * risk first, with the reason for every item. Nothing moves until the person confirms the
 * full list; the executor then re-checks each item.
 */
export function RecoveryPlanPanel({ onMoved }: { onMoved: () => void }) {
  const [amount, setAmount] = useState("10");
  const [unit, setUnit] = useState<Unit>("GB");
  const [ceiling, setCeiling] = useState<Risk>("safe");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<RecoveryPlan | null>(null);

  const target = Math.round(Number(amount) * UNITS[unit]);
  const valid = Number.isFinite(target) && target > 0;

  async function make() {
    setBusy(true);
    setError(null);
    try {
      setResult(await ipc.planRecovery(target, ceiling));
    } catch (e) {
      setResult(null);
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  const field = "rounded-md border border-slate-300 bg-transparent px-2 py-1 dark:border-slate-700";
  return (
    <Panel title="Free up space">
      <p className="mb-3 text-sm text-slate-500 dark:text-slate-400">
        Say how much you want back and the highest risk you accept. Sentinel proposes items from the cleanup types
        below, safest first, and explains each one. Nothing moves until you confirm.
      </p>
      <form
        className="flex flex-wrap items-center gap-3 text-sm"
        onSubmit={(e) => {
          e.preventDefault();
          if (valid && !busy) void make();
        }}
      >
        <label className="flex items-center gap-2">
          Free about
          <input
            aria-label="Amount"
            inputMode="decimal"
            value={amount}
            onChange={(e) => setAmount(e.target.value)}
            className={`${field} w-20 text-right tabular-nums`}
          />
          <select aria-label="Unit" className={field} value={unit} onChange={(e) => setUnit(e.target.value as Unit)}>
            <option value="MB">MB</option>
            <option value="GB">GB</option>
          </select>
        </label>
        <label className="flex items-center gap-2">
          Highest risk
          <select
            aria-label="Highest risk"
            className={field}
            value={ceiling}
            onChange={(e) => setCeiling(e.target.value as Risk)}
          >
            {CEILINGS.map((r) => (
              <option key={r} value={r}>
                {RISK[r]}
              </option>
            ))}
          </select>
        </label>
        <button
          type="submit"
          disabled={!valid || busy}
          className="rounded-md bg-emerald-600 px-3 py-1.5 font-medium text-white hover:bg-emerald-700 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {busy ? "Looking…" : "Make a plan"}
        </button>
      </form>
      {error && (
        <div className="mt-3">
          <ErrorNote message={error} />
        </div>
      )}
      {result && <PlanView key={result.generatedAtMs} r={result} onMoved={onMoved} />}
    </Panel>
  );
}

function PlanView({ r, onMoved }: { r: RecoveryPlan; onMoved: () => void }) {
  const p = r.plan;
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);
  const [ran, setRan] = useState<PlanRunResponse | null>(null);

  async function move() {
    setRunning(true);
    setRunError(null);
    try {
      setRan(await ipc.runRecoveryPlan(p.entries.map((e) => ({ source: e.source, path: e.path }))));
      onMoved();
    } catch (e) {
      setRunError(describeError(e));
    } finally {
      setRunning(false);
      setConfirming(false);
    }
  }

  return (
    <div className="mt-4 space-y-4 text-sm">
      <p>
        {p.reachesTarget ? (
          <>
            This plan frees about <strong>{formatBytes(p.plannedBytes)}</strong> from {p.entries.length}{" "}
            {p.entries.length === 1 ? "item" : "items"}, reaching your {formatBytes(p.targetBytes)} goal.
          </>
        ) : (
          <>
            This plan frees <strong>{formatBytes(p.plannedBytes)}</strong>, which is{" "}
            <strong>{formatBytes(p.shortfallBytes)} short</strong> of your {formatBytes(p.targetBytes)} goal.
          </>
        )}{" "}
        <span className="text-slate-500">
          ({formatBytes(p.availableBytes)} is available at or below {RISK[p.maxRisk].toLowerCase()}.)
        </span>
      </p>
      {p.notes.map((n) => (
        <p
          key={n}
          role="note"
          className="rounded-md border border-amber-300 bg-amber-50 p-2 text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-200"
        >
          {n}
        </p>
      ))}
      {p.entries.length > 0 && (
        <ol className="divide-y divide-slate-100 dark:divide-slate-800">
          {p.entries.map((e, i) => (
            <li key={e.path} className="space-y-0.5 py-2">
              <span className="flex items-baseline gap-3">
                <span className="w-5 text-xs text-slate-400">{i + 1}.</span>
                <span className="w-0 flex-1 truncate font-mono text-xs" title={e.path}>
                  {e.path}
                </span>
                <span className="shrink-0 text-xs text-slate-500">{RISK[e.risk]}</span>
                <span className="w-24 shrink-0 text-right tabular-nums">{formatBytes(e.bytes)}</span>
              </span>
              <span className="block pl-8 text-xs text-slate-500 dark:text-slate-400">
                {e.sourceName}. {e.why}
              </span>
              <span className="block pl-8 text-xs text-slate-500 dark:text-slate-400">{e.onRemoval}</span>
            </li>
          ))}
        </ol>
      )}
      {p.entries.length > 0 && !ran && (
        <button
          type="button"
          onClick={() => setConfirming(true)}
          className="flex items-center gap-1.5 rounded-md bg-amber-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-amber-700"
        >
          <Archive className="size-4" aria-hidden />
          Move {p.entries.length} {p.entries.length === 1 ? "item" : "items"} to quarantine…
        </button>
      )}
      {runError && <ErrorNote message={runError} />}
      {ran && (
        <div className="space-y-2">
          {ran.parts.map((part) => (
            <div key={part.source}>
              <p className="text-xs font-medium text-slate-500">{part.sourceName}</p>
              {part.run && <RunResult result={part.run} />}
              {part.error && <ErrorNote message={`Nothing was moved: ${part.error}`} />}
            </div>
          ))}
          <p className="text-xs text-slate-500 dark:text-slate-400">
            This plan is now out of date. Make a new one to see what is left.
          </p>
        </div>
      )}
      {confirming && (
        <ConfirmMove
          providerName="this plan"
          items={p.entries}
          busy={running}
          onConfirm={() => void move()}
          onCancel={() => setConfirming(false)}
        />
      )}
      <div>
        <h3 className="mb-1 text-xs font-medium uppercase tracking-wide text-slate-500">What was looked at</h3>
        <ul className="space-y-0.5 text-xs text-slate-500 dark:text-slate-400">
          {r.considered.map((c) => (
            <li key={c.source}>
              {c.name} ({RISK[c.risk].toLowerCase()}): {c.eligibleItems} eligible{" "}
              {c.eligibleItems === 1 ? "item" : "items"}, {formatBytes(c.eligibleBytes)}
              {c.note && ` — ${c.note}`}
            </li>
          ))}
        </ul>
        <p className="mt-2 text-xs text-slate-500 dark:text-slate-400">
          Only items that are old enough, not in use, not protected and fully verified are ever offered. Moving
          them needs your confirmation, and each item is checked again just before it moves.
        </p>
      </div>
    </div>
  );
}
