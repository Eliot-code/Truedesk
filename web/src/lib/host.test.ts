import { describe, expect, it } from "vitest";
import {
  canShareDevice,
  hostSupported,
  initialShareState,
  parseHostEvent,
  reduceHostEvent,
  type HostEvent,
} from "./host";

describe("parseHostEvent", () => {
  it("parses each event the engine emits", () => {
    expect(parseHostEvent('{"t":"status","s":"online"}')).toEqual({ t: "status", s: "online" });
    expect(parseHostEvent('{"t":"request","peer":"123456789"}')).toEqual({ t: "request", peer: "123456789" });
    expect(parseHostEvent('{"t":"session_started","peer":"1"}')).toEqual({ t: "session_started", peer: "1" });
    expect(parseHostEvent('{"t":"session_ended"}')).toEqual({ t: "session_ended" });
    expect(parseHostEvent('{"t":"identity_in_use"}')).toEqual({ t: "identity_in_use" });
    expect(parseHostEvent('{"t":"error","message":"boom"}')).toEqual({ t: "error", message: "boom" });
  });

  it("rejects malformed or unknown lines", () => {
    expect(parseHostEvent("not json")).toBeNull();
    expect(parseHostEvent("[]")).toBeNull();
    expect(parseHostEvent('{"t":"status","s":"bogus"}')).toBeNull();
    expect(parseHostEvent('{"t":"request"}')).toBeNull();
    expect(parseHostEvent('{"t":"mystery"}')).toBeNull();
  });
});

describe("reduceHostEvent", () => {
  const fold = (events: HostEvent[]) => events.reduce(reduceHostEvent, initialShareState);

  it("tracks the connection lifecycle", () => {
    expect(fold([{ t: "status", s: "connecting" }]).phase).toBe("connecting");
    expect(fold([{ t: "status", s: "online" }]).phase).toBe("online");
    const live = fold([{ t: "request", peer: "9" }, { t: "session_started", peer: "9" }]);
    expect(live.phase).toBe("in_session");
    expect(live.peer).toBe("9");
    expect(live.request).toBeNull();
  });

  it("clears the peer when the session ends but stays online", () => {
    const ended = fold([{ t: "session_started", peer: "9" }, { t: "session_ended" }]);
    expect(ended.phase).toBe("online");
    expect(ended.peer).toBeNull();
  });

  it("flags an ID clash and surfaces errors", () => {
    expect(fold([{ t: "identity_in_use" }]).identityInUse).toBe(true);
    expect(fold([{ t: "error", message: "relay down" }]).error).toBe("relay down");
  });
});

describe("availability", () => {
  it("is off outside a Tauri app", () => {
    // jsdom has no __TAURI_INTERNALS__, so neither hosting nor the Share entry show.
    expect(hostSupported()).toBe(false);
    expect(canShareDevice("Mozilla/5.0 (Linux; Android 14)")).toBe(false);
  });
});
