// @vitest-environment jsdom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { listWakeable, listContacts, wakeDevice } = vi.hoisted(() => ({
  listWakeable: vi.fn(() => Promise.resolve([] as string[])),
  listContacts: vi.fn(() => Promise.resolve([] as { id: string; alias: string; last_connected: number | null }[])),
  wakeDevice: vi.fn(() => Promise.resolve({ sent: 1, relayed: true })),
}));

vi.mock("../lib/api", () => ({ listWakeable, listContacts, wakeDevice }));

import WakePanel from "./WakePanel";
import { NotifyContext } from "../lib/notify";

describe("WakePanel", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("lists the devices it can wake, by alias, without the one being viewed", async () => {
    listWakeable.mockResolvedValue(["111111111", "222222222", "333333333"]);
    listContacts.mockResolvedValue([{ id: "222222222", alias: "Backup server", last_connected: null }]);
    const view = render(<WakePanel open currentId="111111111" onClose={vi.fn()} />);
    expect(await view.findByRole("button", { name: "Wake Backup server" })).toBeTruthy();
    expect(view.getByRole("button", { name: "Wake 333333333" })).toBeTruthy();
    expect(view.queryByRole("button", { name: "Wake 111111111" })).toBeNull();
  });

  it("reports a failed wake", async () => {
    const notify = vi.fn();
    listWakeable.mockResolvedValue(["222222222"]);
    listContacts.mockResolvedValue([]);
    wakeDevice.mockRejectedValueOnce("Truedesk doesn't know how to wake this device yet. Connect to it once while it's on.");
    const view = render(<NotifyContext.Provider value={notify}><WakePanel open currentId="111111111" onClose={vi.fn()} /></NotifyContext.Provider>);
    fireEvent.click(await view.findByRole("button", { name: "Wake 222222222" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("Truedesk doesn't know how to wake this device yet. Connect to it once while it's on."));
  });

  it("explains how devices get here when there are none, and loads only while open", async () => {
    const view = render(<WakePanel open={false} currentId="111111111" onClose={vi.fn()} />);
    expect(listWakeable).not.toHaveBeenCalled();
    view.rerender(<WakePanel open currentId="111111111" onClose={vi.fn()} />);
    expect(await view.findByText(/learns how to wake a computer/)).toBeTruthy();
  });
});
