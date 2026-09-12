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
  afterEach(cleanup);

  it("allows a device with the selected always-allow setting", async () => {
    vi.mocked(api.respondToRequest).mockResolvedValue(undefined);
    const onResolved = vi.fn();
    render(<IncomingRequest peerId="123456789" fingerprint="7F3A · B921 · C4D2" onResolved={onResolved} />);
    fireEvent.click(screen.getByLabelText("Always allow this device"));
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(api.respondToRequest).toHaveBeenCalledWith("123456789", true, true));
    expect(onResolved).toHaveBeenCalledOnce();
  });

  it("rejects without adding the device to the allow-list", async () => {
    vi.mocked(api.respondToRequest).mockResolvedValue(undefined);
    render(<IncomingRequest peerId="987654321" fingerprint="AAAA · BBBB · CCCC" onResolved={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Reject" }));
    await waitFor(() => expect(api.respondToRequest).toHaveBeenCalledWith("987654321", false, false));
  });
});
