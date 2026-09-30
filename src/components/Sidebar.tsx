import { ShieldCheck } from "lucide-react";
import { PAGES, type PageId } from "../lib/nav";

interface Props {
  current: PageId;
  onSelect: (id: PageId) => void;
}

export function Sidebar({ current, onSelect }: Props) {
  return (
    <aside className="flex w-56 shrink-0 flex-col border-r border-slate-800 bg-slate-900 text-slate-300">
      <div className="flex items-center gap-2 px-5 pt-5 pb-6">
        <ShieldCheck className="size-6 text-emerald-400" aria-hidden />
        <span className="text-lg font-semibold tracking-tight text-white">Sentinel</span>
      </div>
      <nav aria-label="Main" className="flex-1 space-y-0.5 px-2">
        {PAGES.map(({ id, label, icon: Icon, plannedPhase }) => {
          const active = id === current;
          return (
            <button
              key={id}
              type="button"
              onClick={() => onSelect(id)}
              aria-current={active ? "page" : undefined}
              className={`flex w-full items-center gap-3 rounded-md px-3 py-2 text-left text-sm transition-colors focus-visible:outline-2 focus-visible:outline-emerald-400 ${
                active ? "bg-slate-800 text-white" : "hover:bg-slate-800/60 hover:text-white"
              }`}
            >
              <Icon className={`size-4 ${active ? "text-emerald-400" : ""}`} aria-hidden />
              <span className="flex-1">{label}</span>
              {plannedPhase !== null && (
                <span className="text-[10px] uppercase tracking-wide text-slate-500" aria-hidden>
                  soon
                </span>
              )}
            </button>
          );
        })}
      </nav>
    </aside>
  );
}
