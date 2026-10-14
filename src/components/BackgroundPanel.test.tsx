import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Preferences } from "../bindings/Preferences";
import { BackgroundPanel } from "./BackgroundPanel";

// IPC-boundary doubles only.
const off: Preferences = { lowSpaceAlerts: false, runInBackground: false, startWithWindows: false };

/** A backend that keeps the preferences like the real one: sign-in needs background mode. */
function backend(failWith?: string) {
  let current = off;
  const saved: Preferences[] = [];
  mockIPC((cmd, args) => {
    if (cmd === "preferences") return current;
    if (cmd === "set_preferences") {
      if (failWith) throw { kind: "system", message: failWith };
      const { preferences } = args as { preferences: Preferences };
      saved.push(preferences);
      current = { ...preferences, startWithWindows: preferences.runInBackground && preferences.startWithWindows };
      return current;
    }
    throw new Error(`unexpected command ${cmd}`);
  });
  return saved;
}

const alerts = () => screen.findByRole("checkbox", { name: /low on space/ });
const background = () => screen.findByRole("checkbox", { name: /Keep Sentinel running/ });
const signIn = () => screen.findByRole("checkbox", { name: /Start Sentinel when I sign in/ });

describe("BackgroundPanel", () => {
  it("has everything off by default and saves each change at once", async () => {
    const saved = backend();
    render(<BackgroundPanel />);
    expect(await alerts()).not.toBeChecked();
    expect(await background()).not.toBeChecked();
    expect(await signIn()).not.toBeChecked();

    await userEvent.click(await alerts());
    expect(saved).toEqual([{ ...off, lowSpaceAlerts: true }]);
    expect(await alerts()).toBeChecked();
  });

  it("will not offer start-at-sign-in until background mode is on", async () => {
    const saved = backend();
    render(<BackgroundPanel />);
    expect(await signIn()).toBeDisabled();
    expect(screen.getByText(/Needs the option above/)).toBeInTheDocument();

    await userEvent.click(await background());
    expect(await signIn()).toBeEnabled();
    await userEvent.click(await signIn());
    expect(saved.at(-1)).toEqual({ lowSpaceAlerts: false, runInBackground: true, startWithWindows: true });
    expect(await signIn()).toBeChecked();
    expect(screen.getByText(/Adds one entry for your account/)).toBeInTheDocument();
  });

  it("turning background mode off turns sign-in off with it", async () => {
    backend();
    render(<BackgroundPanel />);
    await userEvent.click(await background());
    await userEvent.click(await signIn());
    await userEvent.click(await background());
    expect(await background()).not.toBeChecked();
    expect(await signIn()).not.toBeChecked();
    expect(await signIn()).toBeDisabled();
  });

  it("shows a failure and leaves the switches as they really are", async () => {
    backend("Windows would not create the notification-area icon: no tray");
    render(<BackgroundPanel />);
    await userEvent.click(await background());
    expect(await screen.findByRole("alert")).toHaveTextContent("no tray");
    expect(await background()).not.toBeChecked();
  });
});
