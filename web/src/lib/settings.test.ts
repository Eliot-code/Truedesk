// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { DEFAULT_RELAY, formatId, generateViewerId, normalizeId, relayUrlError, saveRelay, savedRelay, viewerId, inviteParams } from "./settings";
import { friendlyError, unsupportedReason } from "./errors";

beforeEach(() => localStorage.clear());

describe("viewer ID", () => {
  it("is nine digits and stays the same across visits", () => {
    const id = viewerId();
    expect(id).toMatch(/^[1-9]\d{8}$/);
    expect(viewerId()).toBe(id);
  });

  it("covers the desktop range and rejects biased samples", () => {
    const values = [0xffffffff, 0, 899_999_999];
    const take = (array: Uint32Array) => { array[0] = values.shift()!; };
    expect(generateViewerId(take)).toBe("100000000");
    expect(generateViewerId(take)).toBe("999999999");
  });

  it("accepts spaced or dashed IDs", () => {
    expect(normalizeId(" 123 456-789 ")).toBe("123456789");
    expect(formatId("123456789")).toBe("123 456 789");
  });
});

describe("relay URL", () => {
  it("requires wss:// except on this computer", () => {
    expect(relayUrlError("wss://relay.example.com/ws")).toBeNull();
    expect(relayUrlError("ws://127.0.0.1:8080/ws", true)).toBeNull();
    expect(relayUrlError("ws://127.0.0.1:8080/ws", false)).toMatch(/wss:\/\//);
    expect(relayUrlError("ws://relay.example.com/ws")).toMatch(/wss:\/\//);
    expect(relayUrlError("https://relay.example.com")).toMatch(/wss:\/\//);
    expect(relayUrlError("not a url")).toMatch(/relay URL/);
  });

  it("remembers only a non-default relay", () => {
    expect(savedRelay()).toBe(DEFAULT_RELAY);
    saveRelay("wss://mine.example/ws");
    expect(savedRelay()).toBe("wss://mine.example/ws");
    saveRelay(DEFAULT_RELAY);
    expect(localStorage.length).toBe(0);
  });
});

describe("errors", () => {
  it("uses the desktop wording", () => {
    expect(friendlyError("key confirmation failed; authentication failed—check the password and connect again (it changes after every session)"))
      .toMatch(/^Wrong password\./);
    expect(friendlyError("relay closed while connecting")).toBe("The relay closed the connection. Check the ID and try again.");
    expect(friendlyError("That device is offline or already in a session.")).toBe("That device is offline or already in a session.");
    expect(friendlyError("could not reach the relay at wss://x/ws; gave up reconnecting after 30s")).toMatch(/^Couldn't reach the relay/);
    expect(friendlyError("connection closed during session; gave up reconnecting after 30s")).toBe("Connection closed during session; gave up reconnecting after 30s.");
  });

  it("explains missing browser features", () => {
    expect(unsupportedReason({ isSecureContext: true, WebSocket: {}, VideoDecoder: {} })).toBeNull();
    expect(unsupportedReason({ isSecureContext: true, WebSocket: {} })).toMatch(/WebCodecs/);
    expect(unsupportedReason({ isSecureContext: false })).toMatch(/https/);
  });
});

describe("inviteParams", () => {
  it("reads a valid id and relay from the query", () => {
    expect(inviteParams("?id=123456789")).toEqual({ id: "123456789", relay: null });
    expect(inviteParams("?id=123-456-789")).toEqual({ id: "123456789", relay: null });
    expect(inviteParams("?id=123456789&relay=wss://relay.example.com/ws")).toEqual({ id: "123456789", relay: "wss://relay.example.com/ws" });
  });
  it("ignores a bad id, a bad relay, and an empty query", () => {
    expect(inviteParams("?id=abc")).toEqual({ id: null, relay: null });
    expect(inviteParams("?id=123456789&relay=not-a-url")).toEqual({ id: "123456789", relay: null });
    expect(inviteParams("")).toEqual({ id: null, relay: null });
  });
});
