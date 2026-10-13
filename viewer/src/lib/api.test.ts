// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri invoke so we can assert the exact payload KEYS sent to the
// backend. Tauri v2 exposes Rust snake_case params to JS as camelCase, so
// multi-word args must be passed as targetId/peerId — not target_id/peer_id.
const invoke = vi.fn(() => Promise.resolve());
const listen = vi.fn((_event: string, _handler: unknown) => Promise.resolve(vi.fn()));
class FakeChannel {
  id = 41;
  constructor(public onmessage: (message: unknown) => void) {}
}
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: FakeChannel }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));

const api = await import("./api");

describe("api invoke payload keys (camelCase contract)", () => {
  afterEach(() => invoke.mockClear());

  it("connect passes targetId (not target_id)", async () => {
    await api.connect("123456789", "pw");
    expect(invoke).toHaveBeenCalledWith("connect", { targetId: "123456789", password: "pw" });
  });

  it("respondToRequest passes peerId (not peer_id)", async () => {
    await api.respondToRequest("123456789", true, false);
    expect(invoke).toHaveBeenCalledWith("respond_to_request", { peerId: "123456789", allow: true, always: false });
  });

  it("sendInput passes raw bytes to the backend", async () => {
    await api.sendInput(new Uint8Array([1, 2, 255]));
    expect(invoke).toHaveBeenCalledWith("send_input", { bytes: [1, 2, 255] });
  });

  it("appendRecording sends the chunk as base64", async () => {
    await api.appendRecording(7, new Uint8Array([0, 1, 254, 255]).buffer);
    expect(invoke).toHaveBeenCalledWith("append_recording", { id: 7, chunk: "AAH+/w==" });
  });

  it("startRecording passes suggestedName", async () => {
    await api.startRecording("s.webm");
    expect(invoke).toHaveBeenCalledWith("start_recording", { suggestedName: "s.webm" });
  });

  it("startVideo hands the backend a binary channel that feeds the handler", async () => {
    const handler = vi.fn();
    const stop = api.startVideo(handler);
    expect(invoke).toHaveBeenCalledWith("start_video", { channel: expect.any(FakeChannel) });
    const channel = (invoke.mock.calls[0] as unknown[])[1] as { channel: FakeChannel };
    const message = new ArrayBuffer(18);
    channel.channel.onmessage(message);
    expect(handler).toHaveBeenCalledWith(message);

    stop();
    stop();
    channel.channel.onmessage(message);
    expect(handler).toHaveBeenCalledOnce();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("stop_video", { id: 41 }));
    expect(invoke.mock.calls.filter((call) => (call as unknown[])[0] === "stop_video")).toHaveLength(1);
  });

  it("requestKeyframe invokes the backend command", async () => {
    await api.requestKeyframe();
    expect(invoke).toHaveBeenCalledWith("request_keyframe");
  });

  it("sendChat passes the text to the backend", async () => {
    await api.sendChat("hello");
    expect(invoke).toHaveBeenCalledWith("send_chat", { text: "hello" });
  });
});
