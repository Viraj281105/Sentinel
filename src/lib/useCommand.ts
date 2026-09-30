import { useEffect, useState } from "react";
import { describeError } from "./ipc";

export type CommandState<T> =
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; message: string };

/** Run a backend command once on mount and track its result. */
export function useCommand<T>(run: () => Promise<T>): CommandState<T> {
  const [state, setState] = useState<CommandState<T>>({ status: "loading" });
  useEffect(() => {
    let cancelled = false;
    run().then(
      (data) => {
        if (!cancelled) setState({ status: "ok", data });
      },
      (err: unknown) => {
        if (!cancelled) setState({ status: "error", message: describeError(err) });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [run]);
  return state;
}
