import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DockerOverview } from "../bindings/DockerOverview";
import type { WslInventory } from "../bindings/WslInventory";
import { Containers } from "./Containers";

// IPC-boundary doubles only.
const GB = 1e9;
const wsl: WslInventory = {
  distros: [
    {
      name: "docker-desktop",
      version: 2,
      isDefault: false,
      dockerDesktop: true,
      basePath: String.raw`C:\Docker\wsl\main`,
      disks: [{ path: String.raw`C:\Docker\wsl\disk\docker_data.vhdx`, bytes: 20 * GB, modifiedMs: Date.now() }],
      diskBytes: 20 * GB,
    },
    { name: "Ubuntu", version: 2, isDefault: true, dockerDesktop: false, basePath: null, disks: [], diskBytes: 0 },
  ],
  totalDiskBytes: 20 * GB,
  warnings: [],
};
const running: DockerOverview = {
  state: "running",
  cliPath: "docker.exe",
  serverVersion: "29.7.2",
  usage: [{ kind: "Images", total: 3, active: 0, sizeBytes: 6 * GB, reclaimableBytes: 6 * GB }],
  containers: [{ id: "c1", name: "web", image: "app:latest", state: "exited", status: "Exited (0) 3 days ago" }],
  images: [
    { id: "a", repository: "app", tag: "latest", sizeBytes: 2.7 * GB, created: "8 days ago", dangling: false, containers: 0 },
    { id: "b", repository: "<none>", tag: "<none>", sizeBytes: 1 * GB, created: "2 months ago", dangling: true, containers: 0 },
  ],
  volumes: [{ name: "shop_pgdata", driver: "local", sizeBytes: 51e6, links: 0, composeProject: "shop" }],
  warnings: [],
};

function backend(docker: DockerOverview) {
  mockIPC((cmd) => {
    if (cmd === "docker_overview") return docker;
    if (cmd === "wsl_distros") return wsl;
    throw new Error(`unexpected ${cmd}`);
  });
}

describe("Containers page", () => {
  it("shows Docker and WSL on request, stating what is approximate and what is never touched", async () => {
    backend(running);
    render(<Containers />);
    expect(screen.queryByText(/Images \(/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Inspect/ }));

    expect(await screen.findByText("Images (2)")).toBeInTheDocument();
    expect(screen.getAllByText("app:latest")).toHaveLength(2); // the image and the container using it
    expect(screen.getByText("<untagged>")).toBeInTheDocument();
    expect(screen.getByText(/rounded figures Docker prints/)).toBeInTheDocument();
    expect(screen.getByText(/volumes can hold databases/)).toBeInTheDocument();
    expect(screen.getByText(/Compose: shop · no container uses it/)).toBeInTheDocument();
    expect(screen.getByText(/Docker counts ≈ .* as reclaimable/)).toBeInTheDocument();

    expect(screen.getByText("docker-desktop")).toBeInTheDocument();
    expect(screen.getByText("Ubuntu (default)")).toBeInTheDocument();
    expect(screen.getByText(/never offers to delete or shrink/)).toBeInTheDocument();
  });

  it("says plainly when Docker is not running and does not offer to start it", async () => {
    backend({ ...running, state: "notRunning", usage: [], images: [], containers: [], volumes: [] });
    render(<Containers />);
    await userEvent.click(screen.getByRole("button", { name: /Inspect/ }));
    expect(await screen.findByText(/engine is not running/)).toBeInTheDocument();
    expect(screen.getByText(/does not start Docker/)).toBeInTheDocument();
    expect(screen.getByText("docker-desktop")).toBeInTheDocument(); // WSL still listed
  });

  it("says when Docker is not installed", async () => {
    backend({ ...running, state: "notInstalled", cliPath: null });
    render(<Containers />);
    await userEvent.click(screen.getByRole("button", { name: /Inspect/ }));
    expect(await screen.findByText(/Docker is not installed/)).toBeInTheDocument();
  });
});
