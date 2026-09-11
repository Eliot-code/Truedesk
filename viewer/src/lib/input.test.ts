import { describe, expect, it } from "vitest";
import { encodeInputEvent } from "./input";

describe("encodeInputEvent", () => {
  it("matches the protocol mouse-move wire bytes exactly", () => {
    const encoded = encodeInputEvent({ kind: "mouseMove", x: 30000, y: 45000 }, 123);
    expect(Array.from(encoded)).toEqual([
      0x01,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7b,
      0x00, 0x00, 0x75, 0x30,
      0x00, 0x00, 0xaf, 0xc8,
    ]);
  });
});
