import { formatBytes } from "../lib/format";

export interface SparkPoint {
  /** Milliseconds. */
  x: number;
  /** Bytes, or `null` when the size is not known at that time. */
  y: number | null;
  /** Shown hollow: a lower bound, not part of the trend. */
  partial?: boolean;
}

const W = 300;
const H = 56;
const PAD = 6;

/**
 * A small line of sizes over time. The vertical scale runs from the smallest to the
 * largest value shown, not from zero, so a small change on a large total still shows;
 * the range is printed beside it so the slope is never read as bigger than it is.
 */
export function Sparkline({ points, label }: { points: SparkPoint[]; label: string }) {
  const known = points.filter((p): p is SparkPoint & { y: number } => p.y !== null);
  if (known.length === 0) return null;
  const xs = points.map((p) => p.x);
  const [x0, x1] = [Math.min(...xs), Math.max(...xs)];
  const ys = known.map((p) => p.y);
  const [y0, y1] = [Math.min(...ys), Math.max(...ys)];
  const px = (x: number) => (x1 === x0 ? W / 2 : PAD + ((x - x0) / (x1 - x0)) * (W - 2 * PAD));
  const py = (y: number) => (y1 === y0 ? H / 2 : H - PAD - ((y - y0) / (y1 - y0)) * (H - 2 * PAD));
  const line = known
    .filter((p) => !p.partial)
    .map((p) => `${px(p.x).toFixed(1)},${py(p.y).toFixed(1)}`)
    .join(" ");
  return (
    <figure className="flex items-center gap-3">
      <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={label} className="h-14 w-full max-w-md text-emerald-600">
        <polyline points={line} fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
        {known.map((p) => (
          <circle
            key={`${p.x}`}
            cx={px(p.x)}
            cy={py(p.y)}
            r="3.5"
            fill={p.partial ? "none" : "currentColor"}
            stroke="currentColor"
            strokeWidth="1.5"
          />
        ))}
      </svg>
      <figcaption className="shrink-0 text-xs text-slate-500 dark:text-slate-400">
        {y0 === y1 ? formatBytes(y0) : `${formatBytes(y0)} to ${formatBytes(y1)}`}
        <br />
        scale does not start at zero
      </figcaption>
    </figure>
  );
}
