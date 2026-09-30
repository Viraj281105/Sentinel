import { Construction } from "lucide-react";
import type { PageDef } from "../lib/nav";
import { PageHeader } from "./ui";

/** Honest placeholder for pages whose backend does not exist yet. Shows no data. */
export function NotImplemented({ page }: { page: PageDef }) {
  return (
    <>
      <PageHeader title={page.label} subtitle={page.summary} />
      <div className="flex items-start gap-3 rounded-lg border border-dashed border-slate-300 p-6 dark:border-slate-700">
        <Construction className="mt-0.5 size-5 shrink-0 text-amber-500" aria-hidden />
        <div>
          <p className="font-medium">Not implemented yet</p>
          <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            This page is planned for roadmap phase {page.plannedPhase}. Sentinel does not show
            placeholder or sample data.
          </p>
        </div>
      </div>
    </>
  );
}
