import { describe, expect, it } from "vitest";
import { attachInput, encodeInputEvent } from "./input";

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

describe("attachInput", () => {
  it("sends sealed events in capture order when sealing resolves out of order", async () => {
    const listeners = new Map<string, EventListener>();
    const canvas = {
      tabIndex: -1,
      addEventListener: (type: string, listener: EventListener) => listeners.set(type, listener),
      focus: () => {},
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
    } as unknown as HTMLCanvasElement;
    const sent: number[] = [];
    const ws = {
      readyState: 1,
      send: (frame: Uint8Array) => sent.push(frame[1]),
    } as unknown as WebSocket;
    let resolveFirst!: (sealed: Uint8Array) => void;
    const firstSeal = new Promise<Uint8Array>((resolve) => {
      resolveFirst = resolve;
    });
    let sealCalls = 0;
    const seal = () => {
      sealCalls += 1;
      return sealCalls === 1 ? firstSeal : Promise.resolve(new Uint8Array([sealCalls]));
    };

    attachInput(ws, canvas, seal);
    const keydown = listeners.get("keydown")!;
    const keyup = listeners.get("keyup")!;
    keydown({ preventDefault: () => {}, keyCode: 65, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false } as unknown as KeyboardEvent);
    keyup({ preventDefault: () => {}, keyCode: 65, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false } as unknown as KeyboardEvent);
    keydown({ preventDefault: () => {}, keyCode: 66, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false } as unknown as KeyboardEvent);

    await Promise.resolve();
    expect(sealCalls).toBe(1);
    expect(sent).toEqual([]);
    resolveFirst(new Uint8Array([1]));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(sent).toEqual([1, 2, 3]);
  });
});
