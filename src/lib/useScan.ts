import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { ScanFailedEvent } from "../bindings/ScanFailedEvent";
import type { ScanFinishedEvent } from "../bindings/ScanFinishedEvent";
import type { ScanProgressEvent } from "../bindings/ScanProgressEvent";
import type { SavedScan } from "../bindings/SavedScan";
import { describeError, ipc } from "./ipc";

export interface ScanState {
  loaded: boolean;
  running: ScanProgressEvent | null;
  last: SavedScan | null;
  error: string | null;
}

/**
 * Tracks the backend's single background scan. State comes from `scan_status` on
 * mount and is then kept current by `scan-*` events, so leaving and returning to the
 * page never loses a running scan.
 */
export function useScan() {
  const [state, setState] = useState<ScanState>({ loaded: false, running: null, last: null, error: null });

  useEffect(() => {
    let disposed = false;
    const unlisten: UnlistenFn[] = [];
    const sub = async () => {
      const handlers = await Promise.all([
        listen<ScanProgressEvent>("scan-progress", (e) =>
          setState((s) => (s.running && s.running.id !== e.payload.id ? s : { ...s, running: e.payload })),
        ),
        listen<ScanFinishedEvent>("scan-finished", (e) =>
          setState((s) => ({ ...s, running: null, last: e.payload.scan, error: null })),
        ),
        listen<ScanFailedEvent>("scan-failed", (e) =>
          setState((s) => ({ ...s, running: null, error: e.payload.message })),
        ),
      ]);
      if (disposed) handlers.forEach((u) => u());
      else unlisten.push(...handlers);
      const status = await ipc.scanStatus();
      if (!disposed) {
        setState((s) => ({ ...s, loaded: true, running: status.running ?? null, last: status.last ?? s.last }));
      }
    };
    sub().catch((err: unknown) => {
      if (!disposed) setState((s) => ({ ...s, loaded: true, error: describeError(err) }));
    });
    return () => {
      disposed = true;
      unlisten.forEach((u) => u());
    };
  }, []);

  const start = useCallback(async (root: string) => {
    try {
      const id = await ipc.startScan(root);
      setState((s) => ({
        ...s,
        error: null,
        running: s.running?.id === id ? s.running : { id, root, progress: { dirs: 0, files: 0, bytes: 0, problems: 0 } },
      }));
    } catch (err) {
      setState((s) => ({ ...s, error: describeError(err) }));
    }
  }, []);

  const cancel = useCallback(async (id: number) => {
    try {
      await ipc.cancelScan(id);
    } catch (err) {
      setState((s) => ({ ...s, error: describeError(err) }));
    }
  }, []);

  return { state, start, cancel };
}
