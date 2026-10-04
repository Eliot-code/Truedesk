// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Home from "./Home";
import * as api from "../lib/api";

vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  return { ...actual, getIdentity: vi.fn(), getConfig: vi.fn(), getUnattendedStatus: vi.fn(), listContacts: vi.fn(), listAllowed: vi.fn(), startHosting: vi.fn(), connect: vi.fn() };
});
describe("Home", () => {
  afterEach(cleanup);
  beforeEach(() => {
    vi.mocked(api.getIdentity).mockResolvedValue({ id: "123456789", password: "654321" });
    vi.mocked(api.getConfig).mockResolvedValue({ relay_url: "wss://relay", relay_urls: ["wss://relay"], recent_ids: [] });
    vi.mocked(api.getUnattendedStatus).mockResolvedValue({ enabled: false, has_password: false, twofa_enabled: false });
    vi.mocked(api.listContacts).mockResolvedValue([]);
    vi.mocked(api.listAllowed).mockResolvedValue([]);
    vi.mocked(api.startHosting).mockResolvedValue(undefined); vi.mocked(api.connect).mockResolvedValue(undefined);
  });
  it("renders the identity returned by the backend", async () => { render(<Home status="online" onConnect={vi.fn()} incomingRequest={null} onDismissRequest={vi.fn()} />); expect(await screen.findByText("123456789")).toBeTruthy(); });
  it("connects with the entered target ID and password", async () => {
    const onConnect = vi.fn().mockResolvedValue(undefined);
    const prompt = vi.spyOn(window, "prompt").mockReturnValue(null);
    render(<Home status="online" onConnect={onConnect} incomingRequest={null} onDismissRequest={vi.fn()} />);
    fireEvent.change(screen.getByPlaceholderText("000 000 000"), { target: { value: "987654321" } }); fireEvent.change(screen.getByPlaceholderText("Password"), { target: { value: "remote-secret" } }); fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(onConnect).toHaveBeenCalledWith("987654321", "remote-secret"));
    expect(prompt).not.toHaveBeenCalled();
  });
  it("passes an entered 2FA code when connecting", async () => {
    const onConnect = vi.fn().mockResolvedValue(undefined);
    render(<Home status="online" onConnect={onConnect} incomingRequest={null} onDismissRequest={vi.fn()} />);
    fireEvent.change(screen.getByPlaceholderText("000 000 000"), { target: { value: "987654321" } }); fireEvent.change(screen.getByPlaceholderText("Password"), { target: { value: "remote-secret" } }); fireEvent.change(screen.getByLabelText("2FA code (optional)"), { target: { value: "123456" } }); fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(onConnect).toHaveBeenCalledWith("987654321", "remote-secret", "123456"));
  });
});
