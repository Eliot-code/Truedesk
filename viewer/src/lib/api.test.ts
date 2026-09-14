// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

// Mock the Tauri invoke so we can assert the exact payload KEYS sent to the
// backend. Tauri v2 exposes Rust snake_case params to JS as camelCase, so
// multi-word args must be passed as targetId/peerId — not target_id/peer_id.
const invoke = vi.fn(() => Promise.resolve());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

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
});
