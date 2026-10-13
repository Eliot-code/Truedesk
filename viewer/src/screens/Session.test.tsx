// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { onVirtualDisplaySupport, onVirtualDisplayAdded, onVirtualDisplayRefused, addVirtualDisplay, removeVirtualDisplay, startVideo, requestKeyframe, closeDecoder, onAudioFrame, onMonitorList, onLinkStats, onFileOffer, onFileProgress, onFileDone, onFileError, onFileDrop, pickFile, startRecording, appendRecording, finishRecording, sendFile, respondFile, sendInput, selectMonitor, setAudioMuted, setClipboardSync, getConfig, decode } = vi.hoisted(() => ({
  onVirtualDisplaySupport: vi.fn<(handler: (payload: { max: number }) => void) => Promise<() => void>>(() => Promise.resolve(vi.fn())),
  onVirtualDisplayAdded: vi.fn<(handler: (payload: { id: number }) => void) => Promise<() => void>>(() => Promise.resolve(vi.fn())),
  onVirtualDisplayRefused: vi.fn<(handler: (payload: { code: string }) => void) => Promise<() => void>>(() => Promise.resolve(vi.fn())),
  addVirtualDisplay: vi.fn(() => Promise.resolve()),
  removeVirtualDisplay: vi.fn(() => Promise.resolve()),
  onFileDrop: vi.fn(() => Promise.resolve(vi.fn())),
  pickFile: vi.fn(),
  startRecording: vi.fn(() => Promise.resolve(3)),
  appendRecording: vi.fn(() => Promise.resolve()),
  finishRecording: vi.fn(() => Promise.resolve("/Users/me/Downloads/session.webm")),
  startVideo: vi.fn<(handler: (message: ArrayBuffer) => void) => () => void>(() => vi.fn()),
  requestKeyframe: vi.fn(() => Promise.resolve()),
  closeDecoder: vi.fn(),
  onAudioFrame: vi.fn(() => Promise.resolve(vi.fn())),
  onMonitorList: vi.fn<(handler: (payload: { monitors: { id: number; name: string; width: number; height: number; is_primary: boolean; is_virtual?: boolean }[] }) => void) => Promise<() => void>>(() => Promise.resolve(vi.fn())),
  onLinkStats: vi.fn<(handler: (payload: { path: "direct" | "relay"; rtt_ms: number | null }) => void) => Promise<() => void>>(() => Promise.resolve(vi.fn())),
  onFileOffer: vi.fn(() => Promise.resolve(vi.fn())),
  onFileProgress: vi.fn(() => Promise.resolve(vi.fn())),
  onFileDone: vi.fn(() => Promise.resolve(vi.fn())),
  onFileError: vi.fn(() => Promise.resolve(vi.fn())),
  sendFile: vi.fn(),
  respondFile: vi.fn(),
  sendInput: vi.fn<(bytes: Uint8Array) => Promise<void>>(() => Promise.resolve()),
  selectMonitor: vi.fn(() => Promise.resolve()),
  setAudioMuted: vi.fn(() => Promise.resolve()),
  setClipboardSync: vi.fn(() => Promise.resolve()),
  getConfig: vi.fn(() => Promise.resolve({ relay_url: "", relay_urls: [], recent_ids: [], clipboard_sync: false, allow_virtual_displays: true })),
  decode: vi.fn(),
}));

vi.mock("../lib/api", () => ({ onVirtualDisplaySupport, onVirtualDisplayAdded, onVirtualDisplayRefused, addVirtualDisplay, removeVirtualDisplay, startVideo, requestKeyframe, onAudioFrame, onMonitorList, onLinkStats, onFileOffer, onFileProgress, onFileDone, onFileError, onFileDrop, pickFile, startRecording, appendRecording, finishRecording, sendFile, respondFile, sendInput, selectMonitor, setAudioMuted, setClipboardSync, getConfig }));
vi.mock("../lib/decode", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/decode")>()),
  makeDecoder: vi.fn(() => ({ push: decode, close: closeDecoder })),
}));

import Session from "./Session";
import { makeDecoder } from "../lib/decode";
import { NotifyContext } from "../lib/notify";

describe("Session viewer transport", () => {
  afterEach(() => {
    cleanup();
    startVideo.mockReset().mockImplementation(() => vi.fn());
    closeDecoder.mockClear();
    onFileOffer.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    onFileProgress.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    onFileDone.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    onFileError.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    sendInput.mockClear();
    decode.mockClear();
  });

  it("streams binary video messages straight to the decoder", () => {
    render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);

    expect(startVideo).toHaveBeenCalledOnce();
    const message = new ArrayBuffer(20);
    startVideo.mock.calls[0][0](message);
    expect(decode).toHaveBeenCalledWith(message);
  });

  it("asks the backend for keyframes on the decoder's behalf", () => {
    render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);
    const calls = vi.mocked(makeDecoder).mock.calls;
    const options = calls[calls.length - 1][1];
    options.requestKeyframe();
    expect(requestKeyframe).toHaveBeenCalledOnce();
  });

  it("stops the stream and closes the decoder on unmount", () => {
    const stop = vi.fn();
    startVideo.mockImplementation(() => stop);
    const view = render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);
    view.unmount();
    expect(stop).toHaveBeenCalledOnce();
    expect(closeDecoder).toHaveBeenCalledOnce();
  });

  it("sends canvas keyboard input through the backend", async () => {
    startVideo.mockImplementation(() => vi.fn());
    const { container } = render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);
    const canvas = container.querySelector("canvas") as HTMLCanvasElement;

    fireEvent.keyDown(canvas, { keyCode: 65 });

    await waitFor(() => expect(sendInput).toHaveBeenCalledOnce());
    expect(sendInput.mock.calls[0][0]).toBeInstanceOf(Uint8Array);
    expect((sendInput.mock.calls[0][0] as Uint8Array).length).toBe(15);
  });

  it("keeps the file panel hidden until Files is pressed", () => {
    startVideo.mockImplementation(() => vi.fn());
    const view = render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);
    const panel = view.getByLabelText("File transfer");
    expect(panel.hidden).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "Files" }));
    expect(panel.hidden).toBe(false);
  });

  it("reports where a finished recording was saved", async () => {
    startVideo.mockImplementation(() => vi.fn());
    class FakeRecorder {
      state = "inactive";
      ondataavailable?: (event: { data: Blob }) => void;
      onstop?: () => void;
      start() { this.state = "recording"; }
      stop() { this.state = "inactive"; this.onstop?.(); }
    }
    vi.stubGlobal("MediaRecorder", FakeRecorder);
    HTMLCanvasElement.prototype.captureStream = () => ({ getTracks: () => [] }) as unknown as MediaStream;
    const notify = vi.fn();
    const view = render(<NotifyContext.Provider value={notify}><Session remoteId="123" role="viewer" onDisconnect={vi.fn()} /></NotifyContext.Provider>);
    fireEvent.click(view.getByRole("button", { name: "Record" }));
    fireEvent.click(await view.findByRole("button", { name: "Stop recording" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("Recording saved to /Users/me/Downloads/session.webm", "info"));
    vi.unstubAllGlobals();
  });

  it("surfaces a recording that cannot start", async () => {
    startVideo.mockImplementation(() => vi.fn());
    vi.stubGlobal("MediaRecorder", class {});
    HTMLCanvasElement.prototype.captureStream = () => ({ getTracks: () => [] }) as unknown as MediaStream;
    startRecording.mockRejectedValueOnce("recording Downloads path must be a regular directory");
    const notify = vi.fn();
    const view = render(<NotifyContext.Provider value={notify}><Session remoteId="123" role="viewer" onDisconnect={vi.fn()} /></NotifyContext.Provider>);
    fireEvent.click(view.getByRole("button", { name: "Record" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("Could not start recording: Recording Downloads path must be a regular directory."));
    vi.unstubAllGlobals();
  });

  it("keeps the chosen monitor when the host's displays change", () => {
    const view = render(<Session remoteId="123" role="viewer" status="in_session" onDisconnect={vi.fn()} />);
    const list = onMonitorList.mock.calls[onMonitorList.mock.calls.length - 1][0];
    const monitor = (id: number) => ({ id, name: `D${id}`, width: 0, height: 0, is_primary: id === 1 });
    act(() => list({ monitors: [monitor(1), monitor(2)] }));
    const select = () => view.getByRole("combobox", { name: "Monitor" }) as HTMLSelectElement;
    fireEvent.change(select(), { target: { value: "2" } });
    expect(selectMonitor).toHaveBeenLastCalledWith(2);
    act(() => list({ monitors: [monitor(1), monitor(2), monitor(3)] }));
    expect(select().value).toBe("2");
    // Unplugged: the host went back to the main display.
    act(() => list({ monitors: [monitor(1), monitor(3)] }));
    expect(select().value).toBe("1");
  });

  it("adds a virtual display sized to the window, follows it, and removes it", async () => {
    const notify = vi.fn();
    const view = render(<NotifyContext.Provider value={notify}><Session remoteId="123" role="viewer" status="in_session" onDisconnect={vi.fn()} /></NotifyContext.Provider>);
    const last = <T,>(mock: { mock: { calls: T[][] } }) => mock.mock.calls[mock.mock.calls.length - 1][0];
    const list = last(onMonitorList);
    const monitor = (id: number, is_virtual = false) => ({ id, name: `D${id}`, width: 0, height: 0, is_primary: id === 1, is_virtual });
    act(() => list({ monitors: [monitor(1)] }));
    const select = () => view.getByRole("combobox", { name: "Monitor" }) as HTMLSelectElement;
    // Nothing to add until the host offers it.
    expect(view.queryByRole("option", { name: "Add display…" })).toBeNull();
    act(() => last(onVirtualDisplaySupport)({ max: 1 }));
    vi.stubGlobal("devicePixelRatio", 2);
    vi.stubGlobal("innerWidth", 1440);
    vi.stubGlobal("innerHeight", 900);
    fireEvent.change(select(), { target: { value: "add-display" } });
    expect(addVirtualDisplay).toHaveBeenLastCalledWith(2880, 1800, true);
    vi.unstubAllGlobals();
    act(() => list({ monitors: [monitor(1), monitor(7, true)] }));
    act(() => last(onVirtualDisplayAdded)({ id: 7 }));
    expect(select().value).toBe("7");
    // At the host's limit.
    expect(view.queryByRole("option", { name: "Add display…" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "Remove display" }));
    expect(removeVirtualDisplay).toHaveBeenLastCalledWith(7);
    act(() => last(onVirtualDisplayRefused)({ code: "failed" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("The other computer couldn't add a virtual display."));
  });

  it("shows link reports in the toolbar and clears them while reconnecting", () => {
    const view = render(<Session remoteId="123" role="viewer" status="in_session" onDisconnect={vi.fn()} />);
    const report = onLinkStats.mock.calls[onLinkStats.mock.calls.length - 1][0];
    expect(view.container.querySelector(".session-toolbar__link")).toBeNull();
    act(() => report({ path: "relay", rtt_ms: null }));
    expect(view.container.querySelector(".session-toolbar__link")!.textContent).toBe("Relayed·—");
    act(() => report({ path: "relay", rtt_ms: 48 }));
    expect(view.container.querySelector(".session-toolbar__link")!.textContent).toBe("Relayed·48 ms");
    view.rerender(<Session remoteId="123" role="viewer" status="connecting" onDisconnect={vi.fn()} />);
    expect(view.container.querySelector(".session-toolbar__link")).toBeNull();
    view.rerender(<Session remoteId="123" role="viewer" status="in_session" onDisconnect={vi.fn()} />);
    act(() => report({ path: "direct", rtt_ms: 12 }));
    expect(view.container.querySelector(".session-toolbar__link")!.textContent).toBe("Direct·12 ms");
  });
});

describe("Session host role", () => {
  afterEach(cleanup);

  it("shows only the timer, clipboard toggle and Disconnect", () => {
    const view = render(<Session remoteId="123" role="host" onDisconnect={vi.fn()} />);
    expect(view.container.querySelector("canvas")).toBeNull();
    expect(view.queryByLabelText("File transfer")).toBeNull();
    expect(view.getAllByRole("button").map((button) => button.getAttribute("aria-label") ?? button.textContent)).toEqual(["Sync clipboard", "Disconnect"]);
    expect(view.getByLabelText("Session timer")).toBeTruthy();
  });
});
