// @vitest-environment jsdom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { onVideoFrame, onAudioFrame, onMonitorList, onFileOffer, onFileProgress, onFileDone, onFileError, onFileDrop, pickFile, startRecording, appendRecording, finishRecording, sendFile, respondFile, sendInput, selectMonitor, setAudioMuted, setClipboardSync, getConfig, decode } = vi.hoisted(() => ({
  onFileDrop: vi.fn(() => Promise.resolve(vi.fn())),
  pickFile: vi.fn(),
  startRecording: vi.fn(() => Promise.resolve(3)),
  appendRecording: vi.fn(() => Promise.resolve()),
  finishRecording: vi.fn(() => Promise.resolve("/Users/me/Downloads/session.webm")),
  onVideoFrame: vi.fn(),
  onAudioFrame: vi.fn(() => Promise.resolve(vi.fn())),
  onMonitorList: vi.fn(() => Promise.resolve(vi.fn())),
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
  getConfig: vi.fn(() => Promise.resolve({ relay_url: "", relay_urls: [], recent_ids: [], clipboard_sync: false })),
  decode: vi.fn(),
}));

vi.mock("../lib/api", () => ({ onVideoFrame, onAudioFrame, onMonitorList, onFileOffer, onFileProgress, onFileDone, onFileError, onFileDrop, pickFile, startRecording, appendRecording, finishRecording, sendFile, respondFile, sendInput, selectMonitor, setAudioMuted, setClipboardSync, getConfig }));
vi.mock("../lib/decode", () => ({ makeDecoder: vi.fn(() => ({ decode, close: vi.fn() })) }));

import Session from "./Session";
import { NotifyContext } from "../lib/notify";

describe("Session viewer transport", () => {
  afterEach(() => {
    cleanup();
    onVideoFrame.mockReset();
    onFileOffer.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    onFileProgress.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    onFileDone.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    onFileError.mockReset().mockReturnValue(Promise.resolve(vi.fn()));
    sendInput.mockClear();
    decode.mockClear();
  });

  it("feeds video_frame payloads to the decoder", async () => {
    let frameHandler!: (frame: { seq: number; timestamp_us: number; keyframe: boolean; width: number; height: number; data: string }) => void;
    onVideoFrame.mockImplementation((handler) => {
      frameHandler = handler;
      return Promise.resolve(vi.fn());
    });
    const { container } = render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);

    await waitFor(() => expect(frameHandler).toBeTypeOf("function"));
    frameHandler({ seq: 7, timestamp_us: 99, keyframe: true, width: 640, height: 480, data: btoa(String.fromCharCode(0, 1, 2)) });

    expect(decode).toHaveBeenCalledWith(new Uint8Array([0, 1, 2]), true, 99);
    expect((container.querySelector("canvas") as HTMLCanvasElement).width).toBe(640);
  });

  it("sends canvas keyboard input through the backend", async () => {
    onVideoFrame.mockImplementation(() => Promise.resolve(vi.fn()));
    const { container } = render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);
    const canvas = container.querySelector("canvas") as HTMLCanvasElement;

    fireEvent.keyDown(canvas, { keyCode: 65 });

    await waitFor(() => expect(sendInput).toHaveBeenCalledOnce());
    expect(sendInput.mock.calls[0][0]).toBeInstanceOf(Uint8Array);
    expect((sendInput.mock.calls[0][0] as Uint8Array).length).toBe(15);
  });

  it("keeps the file panel hidden until Files is pressed", () => {
    onVideoFrame.mockImplementation(() => Promise.resolve(vi.fn()));
    const view = render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);
    const panel = view.getByLabelText("File transfer");
    expect(panel.hidden).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "Files" }));
    expect(panel.hidden).toBe(false);
  });

  it("reports where a finished recording was saved", async () => {
    onVideoFrame.mockImplementation(() => Promise.resolve(vi.fn()));
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
    onVideoFrame.mockImplementation(() => Promise.resolve(vi.fn()));
    vi.stubGlobal("MediaRecorder", class {});
    HTMLCanvasElement.prototype.captureStream = () => ({ getTracks: () => [] }) as unknown as MediaStream;
    startRecording.mockRejectedValueOnce("recording Downloads path must be a regular directory");
    const notify = vi.fn();
    const view = render(<NotifyContext.Provider value={notify}><Session remoteId="123" role="viewer" onDisconnect={vi.fn()} /></NotifyContext.Provider>);
    fireEvent.click(view.getByRole("button", { name: "Record" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("Could not start recording: Recording Downloads path must be a regular directory."));
    vi.unstubAllGlobals();
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
