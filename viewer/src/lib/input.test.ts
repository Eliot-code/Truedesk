import { describe, expect, it, vi } from "vitest";
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
  it("sends events in capture order when sends resolve out of order", async () => {
    const listeners = new Map<string, EventListener>();
    const canvas = {
      tabIndex: -1,
      addEventListener: (type: string, listener: EventListener) => listeners.set(type, listener),
      removeEventListener: () => {},
      focus: () => {},
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
    } as unknown as HTMLCanvasElement;
    const sent: number[] = [];
    let resolveFirst!: () => void;
    const firstSend = new Promise<void>((resolve) => {
      resolveFirst = resolve;
    });
    let sendCalls = 0;
    const send = () => {
      sendCalls += 1;
      sent.push(sendCalls);
      return sendCalls === 1 ? firstSend : Promise.resolve();
    };

    attachInput(canvas, send);
    const keydown = listeners.get("keydown")!;
    const keyup = listeners.get("keyup")!;
    keydown({ preventDefault: () => {}, key: "a", keyCode: 65, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false } as unknown as KeyboardEvent);
    keyup({ preventDefault: () => {}, key: "a", keyCode: 65, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false } as unknown as KeyboardEvent);
    keydown({ preventDefault: () => {}, key: "b", keyCode: 66, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false } as unknown as KeyboardEvent);

    await Promise.resolve();
    expect(sendCalls).toBe(1);
    expect(sent).toEqual([1]);
    resolveFirst();
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(sent).toEqual([1, 2, 3]);
  });

  it("encodes the actual printable character and modifier state", async () => {
    const listeners = new Map<string, EventListener>();
    const canvas = {
      tabIndex: 0,
      addEventListener: (type: string, listener: EventListener) => listeners.set(type, listener),
      removeEventListener: () => {},
      focus: () => {},
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
    } as unknown as HTMLCanvasElement;
    const send = vi.fn(() => Promise.resolve());
    attachInput(canvas, send);
    listeners.get("keydown")!({ preventDefault: () => {}, key: "!", keyCode: 49, shiftKey: true, ctrlKey: true, altKey: false, metaKey: false } as unknown as KeyboardEvent);
    await Promise.resolve();
    const bytes = (send.mock.calls[0] as unknown[])[0] as Uint8Array;
    expect(bytes[0]).toBe(5);
    expect(new DataView(bytes.buffer).getUint32(9)).toBe("!".codePointAt(0));
    expect(bytes[14]).toBe(0b0011);
  });
  describe("mouse-move coalescing", () => {
    const setup = () => {
      const listeners = new Map<string, EventListener>();
      const canvas = {
        tabIndex: 0,
        addEventListener: (type: string, listener: EventListener) => listeners.set(type, listener),
        removeEventListener: () => {},
        focus: () => {},
        getBoundingClientRect: () => ({ left: 0, top: 0, width: 65535, height: 65535 }),
      } as unknown as HTMLCanvasElement;
      const sent: string[] = [];
      let release!: () => void;
      const blocked = new Promise<void>((resolve) => { release = resolve; });
      const send = (bytes: Uint8Array) => {
        const view = new DataView(bytes.buffer);
        sent.push(bytes[0] === 1 ? `move ${view.getInt32(9)}` : bytes[0] === 2 ? `button ${bytes[10] ? "down" : "up"}` : `other ${bytes[0]}`);
        return sent.length === 1 ? blocked : Promise.resolve();
      };
      attachInput(canvas, send);
      const move = (x: number) => listeners.get("mousemove")!({ clientX: x, clientY: 0 } as MouseEvent);
      const down = () => listeners.get("mousedown")!({ button: 0 } as MouseEvent);
      const flush = async () => { release(); await new Promise((resolve) => setTimeout(resolve, 0)); };
      return { sent, move, down, flush };
    };

    it("sends only the latest move while the backend is busy", async () => {
      const { sent, move, flush } = setup();
      move(1);
      await Promise.resolve();
      for (let x = 2; x <= 50; x += 1) move(x);
      await flush();
      expect(sent).toEqual(["move 1", "move 50"]);
    });

    it("never moves a later position ahead of a click", async () => {
      const { sent, move, down, flush } = setup();
      move(1);
      await Promise.resolve();
      move(2);
      move(3);
      down();
      move(4);
      move(5);
      await flush();
      expect(sent).toEqual(["move 1", "move 3", "button down", "move 5"]);
    });
  });
});
