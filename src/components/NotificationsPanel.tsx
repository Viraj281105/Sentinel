import { useEffect, useState } from "react";
import type { Preferences } from "../bindings/Preferences";
import { describeError, ipc } from "../lib/ipc";
import { ErrorNote, Loading, Panel } from "./ui";

/** Opt-in alerts. Off until someone turns them on; each change is saved at once. */
export function NotificationsPanel() {
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
      setError(describeError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Notifications">
      {!prefs && !error && <Loading />}
      {prefs && (
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-0.5"
            checked={prefs.lowSpaceAlerts}
            disabled={busy}
            onChange={(e) => void change({ ...prefs, lowSpaceAlerts: e.target.checked })}
          />
          <span>
            <span className="block">Tell me when a drive is low on space</span>
            <span className="block text-slate-500 dark:text-slate-400">
              Sentinel checks your fixed drives every 15 minutes while it is open and shows one Windows notification
              when a drive has less than 10% free. It names only the drive and its free space, and warns again only
              after the drive has recovered. Off by default.
            </span>
          </span>
        </label>
      )}
      {error && <ErrorNote message={error} />}
    </Panel>
  );
}
