// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri invoke so we can assert the exact payload KEYS sent to the
// backend. Tauri v2 exposes Rust snake_case params to JS as camelCase, so
// multi-word args must be passed as targetId/peerId — not target_id/peer_id.
const invoke = vi.fn(() => Promise.resolve());
const listen = vi.fn((_event: string, _handler: unknown) => Promise.resolve(vi.fn()));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
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

  it("onVideoFrame listens to the backend event", async () => {
    const handler = vi.fn();
    await api.onVideoFrame(handler);
    expect(listen).toHaveBeenCalledWith("video_frame", expect.any(Function));
  });
});
