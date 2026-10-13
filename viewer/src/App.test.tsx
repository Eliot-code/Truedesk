// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Listener = (payload: unknown) => void;
const { listeners, mocks } = vi.hoisted(() => {
  const listeners = new Map<string, (payload: unknown) => void>();
  const on = (name: string) => (handler: (payload: unknown) => void) => { listeners.set(name, handler); return Promise.resolve(() => {}); };
  const mocks = {
    onStatusChanged: on("status_changed"), onIncomingRequest: on("incoming_request"), onSessionStarted: on("session_started"),
    onSessionEnded: on("session_ended"), onError: on("error"),
    startVideo: () => () => {}, requestKeyframe: vi.fn(), onAudioFrame: on("audio_frame"), onMonitorList: on("monitor_list"), onLinkStats: on("link_stats"),
    onFileOffer: on("file_offer"), onFileProgress: on("file_progress"), onFileDone: on("file_done"), onFileError: on("file_error"),
    onFileDrop: on("drop"), onPasswordRotated: on("password_rotated"),
    permissionStatus: vi.fn(), connect: vi.fn(), disconnect: vi.fn(),
    getIdentity: vi.fn(), getConfig: vi.fn(), getUnattendedStatus: vi.fn(), listContacts: vi.fn(), listAllowed: vi.fn(), startHosting: vi.fn(),
    sendInput: vi.fn(), respondToRequest: vi.fn(),
  };
  return { listeners, mocks };
});
vi.mock("./lib/api", () => mocks);
vi.mock("./lib/decode", () => ({ makeDecoder: () => ({ push: vi.fn(), close: vi.fn() }) }));

import App from "./App";

const emit = (name: string, payload: unknown) => act(() => { (listeners.get(name) as Listener)(payload); });

async function startConnecting() {
  render(<App />);
  fireEvent.change(await screen.findByPlaceholderText("000 000 000"), { target: { value: "987654321" } });
  fireEvent.change(screen.getByPlaceholderText("Password"), { target: { value: "secret" } });
  fireEvent.click(screen.getByRole("button", { name: "Connect" }));
  await screen.findByRole("heading", { name: /Waiting for 987 654 321 to accept/ });
}

describe("App", () => {
  beforeEach(() => {
    mocks.permissionStatus.mockResolvedValue({ screen_recording: true, accessibility: true });
    mocks.connect.mockResolvedValue(undefined);
    mocks.disconnect.mockResolvedValue(undefined);
    mocks.getIdentity.mockResolvedValue({ id: "123456789", password: "654321" });
    mocks.getConfig.mockResolvedValue({ relay_url: "wss://relay", relay_urls: ["wss://relay"], recent_ids: [], clipboard_sync: false });
    mocks.getUnattendedStatus.mockResolvedValue({ enabled: false, has_password: false, twofa_enabled: false });
    mocks.listContacts.mockResolvedValue([]);
    mocks.listAllowed.mockResolvedValue([]);
    mocks.startHosting.mockResolvedValue(undefined);
  });
  afterEach(() => { cleanup(); vi.useRealTimers(); listeners.clear(); });

  it("waits for the host to accept before showing the session", async () => {
    await startConnecting();
    expect(screen.queryByText("Encrypted")).toBeNull();
    emit("session_started", { role: "viewer", remote_id: "987654321" });
    expect(await screen.findByRole("toolbar", { name: "Session controls" })).toBeTruthy();
  });

  it("returns home with the error visible when the host declines", async () => {
    await startConnecting();
    emit("error", { message: "The host declined the connection request." });
    emit("session_ended", { reason: "error" });
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", expect.stringContaining("The host declined the connection request."));
    expect((screen.getByPlaceholderText("000 000 000") as HTMLInputElement).value).toBe("987654321");
  });

  it("cancels a pending connection", async () => {
    await startConnecting();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(mocks.disconnect).toHaveBeenCalledOnce();
    expect(await screen.findByPlaceholderText("000 000 000")).toBeTruthy();
  });

  it("shows backend errors as a dismissible toast in plain language", async () => {
    render(<App />);
    await screen.findByText("123456789");
    emit("error", { message: "target ID must be 9 digits" });
    const toast = await screen.findByRole("alert");
    expect(toast.textContent).toContain("Enter the 9-digit ID.");
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("clears the toast once the host is online", async () => {
    render(<App />);
    await screen.findByText("123456789");
    emit("error", { message: "relay unreachable; retrying host registration" });
    expect(await screen.findByRole("alert")).toBeTruthy();
    emit("status_changed", { state: "online" });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("auto-clears informational notices", async () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn(() => Promise.resolve()) } });
    render(<App />);
    await screen.findByText("123456789");
    vi.useFakeTimers();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copy device ID" })); });
    expect(screen.getByRole("status").textContent).toContain("ID copied.");
    act(() => { vi.advanceTimersByTime(4000); });
    expect(screen.queryByText("ID copied.")).toBeNull();
  });

  it("opens Permissions on launch when one is missing", async () => {
    mocks.permissionStatus.mockResolvedValue({ screen_recording: false, accessibility: true });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Permissions" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Skip for now" }));
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Permissions" })).toBeNull());
    expect(mocks.permissionStatus).toHaveBeenCalledTimes(2);
  });
});
