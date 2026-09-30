import { AlertTriangle, Loader2 } from "lucide-react";
import { useEffect, useRef } from "react";
import type { PreviewItem } from "../bindings/PreviewItem";
import { formatBytes } from "../lib/format";

interface Props {
  providerName: string;
  items: PreviewItem[];
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/** Explicit confirmation listing exactly what will move. Nothing moves without it. */
export function ConfirmMove({ providerName, items, busy, onConfirm, onCancel }: Props) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const bytes = items.reduce((s, i) => s + i.bytes, 0);

  useEffect(() => {
    // Default focus on Cancel, so Enter does not confirm by accident.
    cancelRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onCancel]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-move-title"
        className="flex max-h-full w-full max-w-2xl flex-col rounded-lg border border-slate-200 bg-white p-5 shadow-xl dark:border-slate-700 dark:bg-slate-900"
      >
        <h2 id="confirm-move-title" className="text-lg font-semibold">
          Move {items.length} {items.length === 1 ? "item" : "items"} ({formatBytes(bytes)}) to quarantine?
        </h2>
        <p className="mt-2 text-sm text-slate-600 dark:text-slate-300">
          From <span className="font-medium">{providerName}</span>. They will be moved to Sentinel's quarantine folder
          on the same drive. You can restore any of them for 14 days; after that they are deleted permanently.
        </p>
        <p className="mt-2 flex items-start gap-2 text-sm text-slate-500 dark:text-slate-400">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden />
          Each item is checked again just before it moves. Anything that changed since this preview, gained a protected
          file, or is in use by a program is left where it is.
        </p>
        <ul className="mt-3 min-h-0 flex-1 overflow-y-auto rounded-md border border-slate-200 text-sm dark:border-slate-800">
          {items.map((i) => (
            <li key={i.path} className="flex gap-3 border-b border-slate-100 px-3 py-1.5 last:border-0 dark:border-slate-800">
              <span className="w-0 flex-1 truncate font-mono text-xs" title={i.path}>
                {i.path}
              </span>
              <span className="tabular-nums">{i.kind === "link" ? "link only" : formatBytes(i.bytes)}</span>
            </li>
          ))}
        </ul>
        <div className="mt-4 flex justify-end gap-2">
          <button
            ref={cancelRef}
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="rounded-md border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-100 disabled:opacity-40 dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={busy}
            className="flex items-center gap-1.5 rounded-md bg-amber-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-amber-700 disabled:opacity-40"
          >
            {busy && <Loader2 className="size-4 animate-spin" aria-hidden />}
            Move to quarantine
          </button>
        </div>
      </div>
    </div>
  );
}
