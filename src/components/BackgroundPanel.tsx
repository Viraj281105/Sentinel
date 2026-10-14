import { useEffect, useState } from "react";
import type { Preferences } from "../bindings/Preferences";
import { describeError, ipc } from "../lib/ipc";
import { ErrorNote, Loading, Panel } from "./ui";

function Toggle({
  label,
  detail,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  detail: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <label className={`flex items-start gap-2 text-sm ${disabled ? "opacity-60" : ""}`}>
      <input
        type="checkbox"
        className="mt-0.5"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span>
        <span className="block">{label}</span>
        <span className="block text-slate-500 dark:text-slate-400">{detail}</span>
      </span>
    </label>
  );
}

/** Opt-in alerts and background mode. Everything is off until switched on; each change is applied at once. */
export function BackgroundPanel() {
  const [prefs, setPrefs] = useState<Preferences | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    ipc.preferences().then(
      (p) => !cancelled && setPrefs(p),
      (e: unknown) => !cancelled && setError(describeError(e)),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  async function change(next: Preferences) {
    setBusy(true);
    setError(null);
    try {
      setPrefs(await ipc.setPreferences(next));
    } catch (e) {
      // Nothing was changed: show what is really in effect.
      setError(describeError(e));
      try {
        setPrefs(await ipc.preferences());
      } catch {
        // keep what is shown
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Notifications and background">
      {!prefs && !error && <Loading />}
      {prefs && (
        <div className="space-y-3">
          <Toggle
            label="Tell me when a drive is low on space"
            detail="Sentinel checks your fixed drives every 15 minutes while it is running and shows one Windows notification when a drive has less than 10% free. It names only the drive and its free space, and warns again only after the drive has recovered."
            checked={prefs.lowSpaceAlerts}
            disabled={busy}
            onChange={(v) => void change({ ...prefs, lowSpaceAlerts: v })}
          />
          <Toggle
            label="Keep Sentinel running in the notification area"
            detail="Closing the window hides it instead of quitting, so alerts keep working. Open it again from the tray icon or by starting Sentinel; choose Quit Sentinel from the icon's menu to exit."
            checked={prefs.runInBackground}
            disabled={busy}
            onChange={(v) => void change({ ...prefs, runInBackground: v })}
          />
          <Toggle
            label="Start Sentinel when I sign in"
            detail={
              prefs.runInBackground
                ? "Adds one entry for your account to Windows' startup list. Sentinel then starts hidden in the notification area. Turn this off here, or in Windows Settings under Apps, Startup."
                : "Needs the option above, so Sentinel always has a tray icon to open it from."
            }
            checked={prefs.startWithWindows}
            disabled={busy || !prefs.runInBackground}
            onChange={(v) => void change({ ...prefs, startWithWindows: v })}
          />
          <p className="text-xs text-slate-500 dark:text-slate-400">All of these are off by default.</p>
        </div>
      )}
      {error && <ErrorNote message={error} />}
    </Panel>
  );
}
