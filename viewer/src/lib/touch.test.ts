import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { attachInput, attachKeyboard, KEY, LONG_PRESS_MS, MODIFIER, NotchAccumulator, PIXELS_PER_NOTCH, wheelPixels } from "./input";

/** The sent events as short strings. */
function decode(bytes: Uint8Array): string {
  const view = new DataView(bytes.buffer);
  switch (bytes[0]) {
    case 1: return `move ${view.getInt32(9)},${view.getInt32(13)}`;
    case 2: return `button ${bytes[9]} ${bytes[10] ? "down" : "up"}`;
    case 3: return `scroll ${view.getInt32(9)},${view.getInt32(13)}`;
    case 4: return `key ${view.getUint32(9)} ${bytes[13] ? "down" : "up"}${bytes[14] ? ` +${bytes[14]}` : ""}`;
    case 5: return `char ${String.fromCodePoint(view.getUint32(9))} ${bytes[13] ? "down" : "up"}${bytes[14] ? ` +${bytes[14]}` : ""}`;
    default: return `other ${bytes[0]}`;
  }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("wheel notches", () => {
  it("turns wheel pixels, lines and pages into notches", () => {
    expect(wheelPixels({ deltaX: 0, deltaY: 100, deltaMode: 0 })).toEqual({ dx: 0, dy: 100 });
    expect(wheelPixels({ deltaX: 0, deltaY: 3, deltaMode: 1 })).toEqual({ dx: 0, dy: PIXELS_PER_NOTCH });
    expect(wheelPixels({ deltaX: 1, deltaY: 0, deltaMode: 2 }).dx).toBe(PIXELS_PER_NOTCH * 8);
  });

  it("adds small movements up and keeps the remainder", () => {
    const notches = new NotchAccumulator(100);
    expect(notches.add(0, 40)).toEqual({ dx: 0, dy: 0 });
    expect(notches.add(0, 40)).toEqual({ dx: 0, dy: 0 });
    expect(notches.add(0, 40)).toEqual({ dx: 0, dy: 1 });
    expect(notches.add(-250, 0)).toEqual({ dx: -2, dy: 0 });
    expect(notches.add(0, 80)).toEqual({ dx: 0, dy: 1 });
  });
});

/** A canvas filling a 400x400 view at the origin, showing a 400x400 frame. */
function fakeCanvas() {
  const listeners = new Map<string, (event: unknown) => void>();
  const canvas = {
    tabIndex: 0, width: 400, height: 400, offsetLeft: 0, offsetTop: 0, offsetWidth: 400, offsetHeight: 400, offsetParent: null,
    style: {} as Record<string, string>,
    addEventListener: (type: string, listener: (event: unknown) => void) => listeners.set(type, listener),
    removeEventListener: (type: string) => listeners.delete(type),
    focus: () => {},
    setPointerCapture: () => {},
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 400, height: 400 }),
  };
  return { canvas: canvas as unknown as HTMLCanvasElement, style: canvas.style, listeners };
}

describe("canvas input", () => {
  let sent: string[];
  let view: ReturnType<typeof fakeCanvas>;
  let detach: () => void;
  const fire = (type: string, event: Record<string, unknown>) => view.listeners.get(type)!({ type, preventDefault: () => {}, ...event });
  const touch = (type: string, id: number, x: number, y: number) => fire(type, { pointerId: id, pointerType: "touch", clientX: x, clientY: y });
  // 0..400 client pixels map to 0..65535.
  const at = (pixels: number) => Math.round((pixels / 400) * 65535);

  beforeEach(() => {
    sent = [];
    view = fakeCanvas();
    detach = attachInput(view.canvas, (bytes) => { sent.push(decode(bytes)); return Promise.resolve(); });
  });
  afterEach(() => { detach(); vi.useRealTimers(); });

  it("scrolls a wheel notch as one notch, not a hundred", async () => {
    fire("wheel", { clientX: 200, clientY: 200, deltaX: 0, deltaY: 100, deltaMode: 0 });
    fire("wheel", { clientX: 200, clientY: 200, deltaX: 0, deltaY: 30, deltaMode: 0 });
    await flush();
    expect(sent).toEqual(["scroll 0,1"]);
  });

  it("clicks where a finger taps", async () => {
    touch("pointerdown", 1, 100, 200);
    touch("pointerup", 1, 101, 201);
    await flush();
    expect(sent).toEqual([`move ${at(100)},${at(200)}`, "button 1 down", "button 1 up"]);
    expect(view.style.touchAction).toBe("none");
  });

  it("right-clicks on a long press", async () => {
    vi.useFakeTimers();
    touch("pointerdown", 1, 40, 40);
    vi.advanceTimersByTime(LONG_PRESS_MS);
    touch("pointerup", 1, 40, 40);
    vi.useRealTimers();
    await flush();
    expect(sent).toEqual([`move ${at(40)},${at(40)}`, "button 2 down", "button 2 up"]);
  });

  it("drags with the left button when a finger moves", async () => {
    touch("pointerdown", 1, 100, 100);
    touch("pointermove", 1, 150, 100);
    touch("pointermove", 1, 200, 100);
    touch("pointerup", 1, 200, 100);
    await flush();
    expect(sent[0]).toBe(`move ${at(100)},${at(100)}`);
    expect(sent[1]).toBe("button 1 down");
    expect(sent[sent.length - 1]).toBe("button 1 up");
    expect(sent).toContain(`move ${at(200)},${at(100)}`);
  });

  it("scrolls with two fingers, content following them", async () => {
    touch("pointerdown", 1, 100, 300);
    touch("pointerdown", 2, 200, 300);
    // Both fingers move up 120px together: three 40px notches further down.
    for (let y = 290; y >= 180; y -= 10) {
      touch("pointermove", 1, 100, y);
      touch("pointermove", 2, 200, y);
    }
    touch("pointerup", 1, 100, 180);
    touch("pointerup", 2, 200, 180);
    await flush();
    const notches = sent.filter((event) => event.startsWith("scroll")).map((event) => Number(event.split(",")[1])).reduce((a, b) => a + b, 0);
    expect(notches).toBe(3);
    expect(sent.some((event) => event.startsWith("button"))).toBe(false);
  });

  it("zooms the picture on a pinch and back to 1x below that", async () => {
    touch("pointerdown", 1, 150, 200);
    touch("pointerdown", 2, 250, 200);
    touch("pointermove", 2, 350, 200);
    expect(view.style.transform).toMatch(/scale\(2\)/);
    touch("pointerup", 1, 150, 200);
    touch("pointerup", 2, 350, 200);
    expect(view.style.transform).toMatch(/scale\(2\)/);
    touch("pointerdown", 1, 100, 200);
    touch("pointerdown", 2, 300, 200);
    touch("pointermove", 2, 190, 200);
    touch("pointerup", 1, 100, 200);
    touch("pointerup", 2, 190, 200);
    expect(view.style.transform).toBe("");
    await flush();
    expect(sent).toEqual([]);
  });

  it("leaves mouse and pen pointers to the mouse handlers", async () => {
    fire("pointerdown", { pointerId: 9, pointerType: "mouse", clientX: 10, clientY: 10 });
    fire("pointerup", { pointerId: 9, pointerType: "mouse", clientX: 10, clientY: 10 });
    await flush();
    expect(sent).toEqual([]);
  });
});

/** A text field the way a phone keyboard drives it. */
function fakeField() {
  const listeners = new Map<string, (event: unknown) => void>();
  const field = {
    value: "",
    addEventListener: (type: string, listener: (event: unknown) => void) => listeners.set(type, listener),
    removeEventListener: (type: string) => listeners.delete(type),
  };
  const fire = (type: string, event: Record<string, unknown> = {}) => listeners.get(type)?.({ type, preventDefault: () => {}, ...event });
  const typeTo = (value: string) => { field.value = value; fire("input"); };
  return { field: field as unknown as HTMLInputElement, raw: field, fire, typeTo };
}

describe("phone keyboard", () => {
  let sent: string[];
  const setup = (onKeystroke = () => {}) => {
    sent = [];
    const view = fakeField();
    const bridge = attachKeyboard(view.field, (bytes) => { sent.push(decode(bytes)); return Promise.resolve(); }, onKeystroke);
    return { ...view, bridge };
  };

  it("types what is entered, with autocorrect fixing earlier letters", async () => {
    const { raw, typeTo } = setup();
    expect(raw.value).toBe(" ");
    typeTo(" te");
    typeTo(" teh");
    typeTo(" the ");
    await flush();
    expect(sent).toEqual([
      "char t down", "char t up", "char e down", "char e up",
      "char h down", "char h up",
      "key 8 down", "key 8 up", "key 8 down", "key 8 up",
      "char h down", "char h up", "char e down", "char e up", "char   down", "char   up",
    ]);
  });

  it("sends backspace even when the field looks empty", async () => {
    const { raw, typeTo } = setup();
    typeTo("");
    await flush();
    expect(sent).toEqual(["key 8 down", "key 8 up"]);
    expect(raw.value).toBe(" ");
  });

  it("presses Enter and the keys that change no text", async () => {
    const { fire } = setup();
    fire("keydown", { key: "Enter" });
    fire("keydown", { key: "ArrowLeft" });
    fire("keydown", { key: "a" });
    await flush();
    expect(sent).toEqual([`key ${KEY.enter} down`, `key ${KEY.enter} up`, `key ${KEY.left} down`, `key ${KEY.left} up`]);
  });

  it("holds modifiers for one keystroke", async () => {
    const onKeystroke = vi.fn();
    const { bridge, typeTo } = setup(onKeystroke);
    bridge.setModifiers(MODIFIER.ctrl);
    typeTo(" c");
    typeTo(" cd");
    bridge.press(KEY.escape);
    await flush();
    expect(sent).toEqual(["char c down +2", "char c up +2", "char d down", "char d up", "key 27 down", "key 27 up"]);
    expect(onKeystroke).toHaveBeenCalledOnce();
  });

  it("waits for a composition to finish before starting over", async () => {
    const { raw, fire, typeTo } = setup();
    fire("compositionstart");
    typeTo(` ${"a".repeat(40)}`);
    expect(raw.value).toBe(` ${"a".repeat(40)}`);
    fire("compositionend");
    expect(raw.value).toBe(" ");
    await flush();
    expect(sent.filter((event) => event === "char a down")).toHaveLength(40);
  });
});
