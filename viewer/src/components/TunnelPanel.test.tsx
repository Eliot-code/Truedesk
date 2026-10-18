// @vitest-environment jsdom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { listTunnels, openTunnel, closeTunnel, onTunnelsChanged } = vi.hoisted(() => ({
  listTunnels: vi.fn(() => Promise.resolve([] as { id: number; local_port: number; host: string; port: number; connections: number }[])),
  openTunnel: vi.fn(() => Promise.resolve({ id: 1, local_port: 50000, host: "localhost", port: 3389, connections: 0 })),
  closeTunnel: vi.fn(() => Promise.resolve()),
  onTunnelsChanged: vi.fn<(handler: () => void) => Promise<() => void>>(() => Promise.resolve(vi.fn())),
}));

vi.mock("../lib/api", () => ({ listTunnels, openTunnel, closeTunnel, onTunnelsChanged }));

import TunnelPanel, { displayTarget, parsePort, tunnelRefusal } from "./TunnelPanel";

describe("TunnelPanel", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("lists the open tunnels and closes one", async () => {
    listTunnels.mockResolvedValue([
      { id: 4, local_port: 13389, host: "192.168.1.20", port: 3389, connections: 1 },
      { id: 5, local_port: 2222, host: "::1", port: 22, connections: 0 },
    ]);
    const view = render(<TunnelPanel open onClose={vi.fn()} />);
    expect(await view.findByText("localhost:13389")).toBeTruthy();
    expect(view.getByText("192.168.1.20:3389")).toBeTruthy();
    expect(view.getByText("[::1]:22")).toBeTruthy();
    expect(view.getByText("1 connection")).toBeTruthy();
    expect(view.getByText("0 connections")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "Close tunnel on port 2222" }));
    await waitFor(() => expect(closeTunnel).toHaveBeenCalledWith(5));
  });

  it("opens a tunnel from the form, picking a free local port when it is empty", async () => {
    const view = render(<TunnelPanel open onClose={vi.fn()} />);
    expect(await view.findByText("No tunnels yet.")).toBeTruthy();
    fireEvent.change(view.getByLabelText("Remote host"), { target: { value: " 10.0.0.5 " } });
    fireEvent.change(view.getByLabelText("Remote port"), { target: { value: "22" } });
    fireEvent.click(view.getByRole("button", { name: "Add tunnel" }));
    await waitFor(() => expect(openTunnel).toHaveBeenCalledWith(0, "10.0.0.5", 22));
    fireEvent.change(view.getByLabelText("Local port"), { target: { value: "8080" } });
    fireEvent.change(view.getByLabelText("Remote port"), { target: { value: "80" } });
    fireEvent.click(view.getByRole("button", { name: "Add tunnel" }));
    await waitFor(() => expect(openTunnel).toHaveBeenLastCalledWith(8080, "10.0.0.5", 80));
  });

  it("explains bad input and backend errors instead of sending them", async () => {
    openTunnel.mockRejectedValueOnce("Port 3389 is already in use on this computer");
    const view = render(<TunnelPanel open onClose={vi.fn()} />);
    fireEvent.change(view.getByLabelText("Remote port"), { target: { value: "70000" } });
    fireEvent.click(view.getByRole("button", { name: "Add tunnel" }));
    expect((await view.findByRole("alert")).textContent).toBe("Enter the remote port, from 1 to 65535.");
    fireEvent.change(view.getByLabelText("Remote port"), { target: { value: "3389" } });
    fireEvent.change(view.getByLabelText("Local port"), { target: { value: "3389" } });
    fireEvent.click(view.getByRole("button", { name: "Add tunnel" }));
    await waitFor(() => expect(view.getByRole("alert").textContent).toBe("Port 3389 is already in use on this computer."));
    expect(openTunnel).toHaveBeenCalledOnce();
  });

  it("refreshes when the backend reports a change", async () => {
    render(<TunnelPanel open={false} onClose={vi.fn()} />);
    await waitFor(() => expect(onTunnelsChanged).toHaveBeenCalled());
    const changed = onTunnelsChanged.mock.calls[0][0];
    changed();
    await waitFor(() => expect(listTunnels).toHaveBeenCalledTimes(2));
  });

  it("parses ports and explains refusals", () => {
    expect(parsePort("22")).toBe(22);
    expect(parsePort(" 65535 ")).toBe(65535);
    expect(parsePort("")).toBeUndefined();
    expect(parsePort("", true)).toBe(0);
    expect(parsePort("0")).toBeUndefined();
    expect(parsePort("65536")).toBeUndefined();
    expect(parsePort("8e3")).toBeUndefined();
    expect(displayTarget("fe80::1", 22)).toBe("[fe80::1]:22");
    expect(tunnelRefusal("unreachable")).toContain("couldn't reach that address");
    expect(tunnelRefusal("weird")).toBe("The other computer refused the tunnel (weird).");
  });
});
