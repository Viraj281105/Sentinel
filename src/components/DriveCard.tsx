import { AlertTriangle, HardDrive, Usb, Network, Disc, MemoryStick, HelpCircle } from "lucide-react";
import type { ReactNode } from "react";
import type { Drive } from "../bindings/Drive";
import type { DriveKind } from "../bindings/DriveKind";
import { formatBytes, percent } from "../lib/format";

const KIND: Record<DriveKind, { label: string; icon: typeof HardDrive }> = {
  fixed: { label: "Local disk", icon: HardDrive },
  removable: { label: "Removable", icon: Usb },
  network: { label: "Network drive", icon: Network },
  optical: { label: "Optical drive", icon: Disc },
  ramDisk: { label: "RAM disk", icon: MemoryStick },
  unknown: { label: "Unknown", icon: HelpCircle },
};

function driveName(d: Drive): string {
  const letter = d.root.replace(/\\$/, "");
  return d.label ? `${d.label} (${letter})` : `${KIND[d.kind].label} (${letter})`;
}

function statusText(d: Drive): string | null {
  switch (d.status.state) {
    case "ready":
      return null;
    case "noMedia":
      return "No media inserted.";
    case "notQueried":
      return d.kind === "unknown"
        ? "Windows reports no usable volume here."
        : "Not queried: network and optical drives can take a long time to respond.";
    case "error":
      return `Could not read this drive: ${d.status.message}`;
  }
}

/** Usage meter. Fill = accent, or warning when low; track = lighter step of the same ramp. */
function UsageMeter({ used, total, low, name }: { used: number; total: number; low: boolean; name: string }) {
  const pct = percent(used, total);
  return (
    <div
      role="meter"
      aria-label={`${name} used space`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
      aria-valuetext={`${Math.round(pct)}% used`}
      className={`h-2 overflow-hidden rounded-full ${
        low ? "bg-amber-100 dark:bg-amber-950" : "bg-emerald-100 dark:bg-emerald-950"
      }`}
    >
      <div
        className={`h-full rounded-full ${low ? "bg-amber-500 dark:bg-amber-400" : "bg-emerald-600 dark:bg-emerald-400"}`}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

export function DriveCard({ drive, action }: { drive: Drive; action?: ReactNode }) {
  const { icon: Icon, label: kindLabel } = KIND[drive.kind];
  const name = driveName(drive);
  const space = drive.space;
  const note = statusText(drive);
  return (
    <article className="rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-800 dark:bg-slate-900">
      <header className="mb-4 flex items-start gap-3">
        <Icon className="mt-0.5 size-5 shrink-0 text-slate-400" aria-hidden />
        <div className="min-w-0 flex-1">
          <h3 className="truncate font-medium">{name}</h3>
          <p className="text-xs text-slate-500 dark:text-slate-400">
            {[kindLabel, drive.fileSystem, drive.isSystem ? "Windows installed here" : null]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>
        {space?.lowSpace && (
          <span className="flex items-center gap-1 rounded-full bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-800 dark:bg-amber-950 dark:text-amber-300">
            <AlertTriangle className="size-3.5" aria-hidden />
            Low space
          </span>
        )}
        {action}
      </header>
      {space ? (
        <>
          <UsageMeter used={space.usedBytes} total={space.totalBytes} low={space.lowSpace} name={name} />
          <dl className="mt-3 grid grid-cols-3 gap-2 text-sm">
            <div>
              <dt className="text-xs text-slate-500 dark:text-slate-400">Free</dt>
              <dd className="font-semibold tabular-nums">{formatBytes(space.freeBytes)}</dd>
            </div>
            <div>
              <dt className="text-xs text-slate-500 dark:text-slate-400">Used</dt>
              <dd className="tabular-nums">
                {formatBytes(space.usedBytes)}{" "}
                <span className="text-slate-500 dark:text-slate-400">
                  ({Math.round(percent(space.usedBytes, space.totalBytes))}%)
                </span>
              </dd>
            </div>
            <div>
              <dt className="text-xs text-slate-500 dark:text-slate-400">Capacity</dt>
              <dd className="tabular-nums">{formatBytes(space.totalBytes)}</dd>
            </div>
          </dl>
          {space.availableBytes < space.freeBytes && (
            <p className="mt-2 text-xs text-slate-500 dark:text-slate-400">
              {formatBytes(space.availableBytes)} available to you (disk quota applies).
            </p>
          )}
        </>
      ) : (
        <p className="text-sm text-slate-500 dark:text-slate-400">{note}</p>
      )}
    </article>
  );
}
