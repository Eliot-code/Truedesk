// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import IncomingRequest from "./IncomingRequest";
import * as api from "../lib/api";

vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  return { ...actual, respondToRequest: vi.fn() };
});

describe("IncomingRequest", () => {
  afterEach(() => { cleanup(); vi.mocked(api.respondToRequest).mockReset(); });

  it("allows a device with the selected always-allow setting", async () => {
    vi.mocked(api.respondToRequest).mockResolvedValue(undefined);
    const onResolved = vi.fn();
    render(<IncomingRequest peerId="123456789" onResolved={onResolved} />);
    fireEvent.click(screen.getByLabelText("Always allow this device"));
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(api.respondToRequest).toHaveBeenCalledWith("123456789", true, true));
    expect(onResolved).toHaveBeenCalledOnce();
  });

  it("rejects without adding the device to the allow-list", async () => {
    vi.mocked(api.respondToRequest).mockResolvedValue(undefined);
    render(<IncomingRequest peerId="987654321" onResolved={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Reject" }));
    await waitFor(() => expect(api.respondToRequest).toHaveBeenCalledWith("987654321", false, false));
  });

  it("marks a known device but still asks for approval", () => {
    render(<IncomingRequest peerId="987654321" known onResolved={vi.fn()} />);
    expect(screen.getByText("Known device")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Allow" })).toBeTruthy();
  });

  it("does not show an unverifiable fingerprint", () => {
    render(<IncomingRequest peerId="987654321" onResolved={vi.fn()} />);
    expect(screen.queryByText(/fingerprint/i)).toBeNull();
    expect(screen.queryByText(/verify/i)).toBeNull();
  });

  it("focuses Reject, keeps Tab inside, and rejects on Escape", async () => {
    vi.mocked(api.respondToRequest).mockResolvedValue(undefined);
    render(<IncomingRequest peerId="987654321" onResolved={vi.fn()} />);
    const reject = screen.getByRole("button", { name: "Reject" });
    const allow = screen.getByRole("button", { name: "Allow" });
    const checkbox = screen.getByLabelText("Always allow this device");
    expect(document.activeElement).toBe(reject);

    allow.focus();
    fireEvent.keyDown(allow, { key: "Tab" });
    expect(document.activeElement).toBe(checkbox);
    fireEvent.keyDown(checkbox, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(allow);

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(api.respondToRequest).toHaveBeenCalledWith("987654321", false, false));
  });
});
