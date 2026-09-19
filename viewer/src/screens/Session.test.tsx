// @vitest-environment jsdom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { onVideoFrame, onFileOffer, onFileProgress, onFileDone, onFileError, sendFile, respondFile, sendInput, decode } = vi.hoisted(() => ({
  onVideoFrame: vi.fn(),
  onFileOffer: vi.fn(() => Promise.resolve(vi.fn())),
  onFileProgress: vi.fn(() => Promise.resolve(vi.fn())),
  onFileDone: vi.fn(() => Promise.resolve(vi.fn())),
  onFileError: vi.fn(() => Promise.resolve(vi.fn())),
  sendFile: vi.fn(),
  respondFile: vi.fn(),
  sendInput: vi.fn<(bytes: Uint8Array) => Promise<void>>(() => Promise.resolve()),
  decode: vi.fn(),
}));

vi.mock("../lib/api", () => ({ onVideoFrame, onFileOffer, onFileProgress, onFileDone, onFileError, sendFile, respondFile, sendInput }));
vi.mock("../lib/decode", () => ({ makeDecoder: vi.fn(() => decode) }));

import Session from "./Session";

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
    let frameHandler!: (frame: { seq: number; timestamp_us: number; keyframe: boolean; width: number; height: number; data: number[] }) => void;
    onVideoFrame.mockImplementation((handler) => {
      frameHandler = handler;
      return Promise.resolve(vi.fn());
    });
    const { container } = render(<Session remoteId="123" role="viewer" onDisconnect={vi.fn()} />);

    await waitFor(() => expect(frameHandler).toBeTypeOf("function"));
    frameHandler({ seq: 7, timestamp_us: 99, keyframe: true, width: 640, height: 480, data: [0, 1, 2] });

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
});
