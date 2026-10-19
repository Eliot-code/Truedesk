export type InputEvent =
  | { kind: "mouseMove"; x: number; y: number }
  | { kind: "mouseButton"; button: number; down: boolean }
  | { kind: "scroll"; dx: number; dy: number }
  | { kind: "key"; code: number; down: boolean; modifiers: number }
  | { kind: "char"; ch: string; down: boolean; modifiers: number };

/** Encode protocol::input::InputEvent using its big-endian wire layout. */
export function encodeInputEvent(ev: InputEvent, tsUs: number): Uint8Array {
  let length: number;
  switch (ev.kind) {
    case "mouseMove":
      length = 17;
      break;
    case "mouseButton":
      length = 11;
      break;
    case "scroll":
      length = 17;
      break;
    case "key":
    case "char":
      length = 15;
      break;
  }

  const bytes = new Uint8Array(length);
  const view = new DataView(bytes.buffer);
  view.setBigUint64(1, BigInt(tsUs), false);

  switch (ev.kind) {
    case "mouseMove":
      view.setUint8(0, 1);
      view.setInt32(9, ev.x, false);
      view.setInt32(13, ev.y, false);
      break;
    case "mouseButton":
      view.setUint8(0, 2);
      view.setUint8(9, ev.button);
      view.setUint8(10, ev.down ? 1 : 0);
      break;
    case "scroll":
      view.setUint8(0, 3);
      view.setInt32(9, ev.dx, false);
      view.setInt32(13, ev.dy, false);
      break;
    case "key":
      view.setUint8(0, 4);
      view.setUint32(9, ev.code, false);
      view.setUint8(13, ev.down ? 1 : 0);
      view.setUint8(14, ev.modifiers);
      break;
    case "char":
      if ([...ev.ch].length !== 1) throw new Error("character input must be one code point");
      view.setUint8(0, 5);
      view.setUint32(9, ev.ch.codePointAt(0)!, false);
      view.setUint8(13, ev.down ? 1 : 0);
      view.setUint8(14, ev.modifiers);
      break;
  }
  return bytes;
}

function timestampUs(): number {
  return Math.floor(Date.now() * 1000);
}

export interface ClientRect { left: number; top: number; width: number; height: number }

/**
 * The rectangle the frame actually occupies inside an element styled with
 * `object-fit: contain`: the frame is scaled to fit and centred, leaving bars
 * on two sides when the aspect ratios differ. Without a frame size yet, the
 * whole element is used.
 */
export function containedRect(element: ClientRect, contentWidth: number, contentHeight: number): ClientRect {
  if (!(contentWidth > 0 && contentHeight > 0) || element.width <= 0 || element.height <= 0) return element;
  const scale = Math.min(element.width / contentWidth, element.height / contentHeight);
  const width = contentWidth * scale;
  const height = contentHeight * scale;
  return {
    left: element.left + (element.width - width) / 2,
    top: element.top + (element.height - height) / 2,
    width,
    height,
  };
}

/**
 * Map a client-space pointer position to the protocol's 0..65535 remote
 * coordinates. Returns null when the pointer is over a letterbox bar.
 */
export function mapPointer(clientX: number, clientY: number, element: ClientRect, contentWidth: number, contentHeight: number): { x: number; y: number } | null {
  const drawn = containedRect(element, contentWidth, contentHeight);
  if (drawn.width <= 0 || drawn.height <= 0) return null;
  const fx = (clientX - drawn.left) / drawn.width;
  const fy = (clientY - drawn.top) / drawn.height;
  if (fx < 0 || fx > 1 || fy < 0 || fy > 1) return null;
  return { x: Math.round(fx * 65535), y: Math.round(fy * 65535) };
}

function modifiers(event: KeyboardEvent): number {
  return (
    (event.shiftKey ? 1 : 0) |
    (event.ctrlKey ? 1 << 1 : 0) |
    (event.altKey ? 1 << 2 : 0) |
    (event.metaKey ? 1 << 3 : 0)
  );
}

function button(button: number): number | null {
  if (button === 0) return 1; // Left
  if (button === 2) return 2; // Right
  if (button === 1) return 3; // Middle
  return null;
}

function namedKeyCode(key: string): number | null {
  const codes: Record<string, number> = {
    Backspace: 8,
    Tab: 9,
    Enter: 13,
    CapsLock: 20,
    Escape: 27,
    PageUp: 33,
    PageDown: 34,
    End: 35,
    Home: 36,
    ArrowLeft: 37,
    ArrowUp: 38,
    ArrowRight: 39,
    ArrowDown: 40,
    Delete: 46,
  };
  if (key in codes) return codes[key];
  const functionKey = /^F([1-9]|1[0-2])$/.exec(key);
  return functionKey ? 111 + Number(functionKey[1]) : null;
}

function keyCode(event: KeyboardEvent): number | null {
  const key = event.key || "";
  // A single printable code point preserves punctuation, shifted symbols, and
  // non-ASCII input without relying on a keyboard-layout-specific key table.
  if ([...key].length === 1 && !/^\p{Cc}|\p{Cf}$/u.test(key)) {
    return key.codePointAt(0) ?? null;
  }
  return namedKeyCode(key) ?? (event.keyCode >>> 0);
}

function isModifierKey(key: string): boolean {
  return key === "Shift" || key === "Control" || key === "Alt" || key === "Meta";
}

/** Pixels of wheel movement per notch of a mouse wheel; hosts scroll by notches. */
export const PIXELS_PER_NOTCH = 100;
/** A two-finger drag this long, in pixels, scrolls one notch. */
export const TOUCH_PIXELS_PER_NOTCH = 40;

/** A wheel event's deltas in pixels, with line and page deltas converted. */
export function wheelPixels(event: { deltaX: number; deltaY: number; deltaMode: number }): { dx: number; dy: number } {
  // DOM_DELTA_LINE: browsers scroll 3 lines per notch. DOM_DELTA_PAGE: rare.
  const scale = event.deltaMode === 1 ? PIXELS_PER_NOTCH / 3 : event.deltaMode === 2 ? PIXELS_PER_NOTCH * 8 : 1;
  return { dx: event.deltaX * scale, dy: event.deltaY * scale };
}

/**
 * Adds up pixel deltas and hands out whole notches, keeping the remainder,
 * so small trackpad and touch movements add up instead of being lost, and a
 * 100-pixel wheel notch scrolls one notch rather than a hundred.
 */
export class NotchAccumulator {
  private x = 0;
  private y = 0;

  constructor(private readonly pixelsPerNotch: number) {}

  add(dx: number, dy: number): { dx: number; dy: number } {
    this.x += dx;
    this.y += dy;
    const notchesX = Math.trunc(this.x / this.pixelsPerNotch);
    const notchesY = Math.trunc(this.y / this.pixelsPerNotch);
    this.x -= notchesX * this.pixelsPerNotch;
    this.y -= notchesY * this.pixelsPerNotch;
    return { dx: notchesX || 0, dy: notchesY || 0 };
  }
}

/**
 * Sends InputEvent bytes in FIFO order. Mouse moves that queue up while the
 * backend is busy collapse into the latest position, but never past a click,
 * scroll, or key event.
 */
class InputQueue {
  private chain = Promise.resolve();
  // The queued, not-yet-sent move; later moves overwrite it in place.
  private pendingMove: { event: InputEvent; tsUs: number } | null = null;

  constructor(private readonly send: (bytes: Uint8Array) => Promise<void>) {}

  private then(next: () => Promise<void>): void {
    this.chain = this.chain
      .then(next)
      .catch((error: unknown) => {
        console.error("Failed to send input event", error);
      });
  }

  enqueue(event: InputEvent): void {
    this.pendingMove = null;
    const bytes = encodeInputEvent(event, timestampUs());
    this.then(() => this.send(bytes));
  }

  enqueueMove(event: InputEvent): void {
    if (this.pendingMove) {
      this.pendingMove.event = event;
      this.pendingMove.tsUs = timestampUs();
      return;
    }
    const slot = { event, tsUs: timestampUs() };
    this.pendingMove = slot;
    this.then(() => {
      if (this.pendingMove === slot) this.pendingMove = null;
      return this.send(encodeInputEvent(slot.event, slot.tsUs));
    });
  }

  click(button: number): void {
    this.enqueue({ kind: "mouseButton", button, down: true });
    this.enqueue({ kind: "mouseButton", button, down: false });
  }

  press(code: number, modifiers = 0): void {
    this.enqueue({ kind: "key", code, down: true, modifiers });
    this.enqueue({ kind: "key", code, down: false, modifiers });
  }

  type(ch: string, modifiers = 0): void {
    this.enqueue({ kind: "char", ch, down: true, modifiers });
    this.enqueue({ kind: "char", ch, down: false, modifiers });
  }
}

/**
 * Capture canvas input and send raw InputEvent bytes in FIFO order. Mouse
 * moves that queue up while the backend is busy collapse into the latest
 * position, but never past a click, scroll, or key event. Touch works too:
 * see {@link attachTouch}.
 */
export function attachInput(
  canvas: HTMLCanvasElement,
  send: (bytes: Uint8Array) => Promise<void>,
): () => void {
  if (canvas.tabIndex < 0) canvas.tabIndex = 0;
  const queue = new InputQueue(send);
  const wheel = new NotchAccumulator(PIXELS_PER_NOTCH);

  const pointer = (event: MouseEvent) => mapPointer(event.clientX, event.clientY, canvas.getBoundingClientRect(), canvas.width, canvas.height);

  const onMouseMove = (event: MouseEvent): void => {
    const position = pointer(event);
    if (position) queue.enqueueMove({ kind: "mouseMove", ...position });
  };

  // Presses over the letterbox bars are ignored; releases always go through
  // so a drag that ends over a bar never leaves a button held remotely.
  const onMouseDown = (event: MouseEvent): void => {
    canvas.focus();
    if (!pointer(event)) return;
    const mapped = button(event.button);
    if (mapped !== null) queue.enqueue({ kind: "mouseButton", button: mapped, down: true });
  };

  const onMouseUp = (event: MouseEvent): void => {
    const mapped = button(event.button);
    if (mapped !== null) queue.enqueue({ kind: "mouseButton", button: mapped, down: false });
  };

  const onWheel = (event: WheelEvent): void => {
    if (!pointer(event)) return;
    const pixels = wheelPixels(event);
    const notches = wheel.add(pixels.dx, pixels.dy);
    if (notches.dx !== 0 || notches.dy !== 0) queue.enqueue({ kind: "scroll", ...notches });
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    event.preventDefault();
    if (isModifierKey(event.key)) return;
    const code = keyCode(event);
    if (code !== null) {
      const key = event.key || "";
      queue.enqueue(key.length > 0 && [...key].length === 1
        ? { kind: "char", ch: key, down: true, modifiers: modifiers(event) }
        : { kind: "key", code, down: true, modifiers: modifiers(event) });
    }
  };

  const onKeyUp = (event: KeyboardEvent): void => {
    event.preventDefault();
    if (isModifierKey(event.key)) return;
    const code = keyCode(event);
    if (code !== null) {
      const key = event.key || "";
      queue.enqueue(key.length > 0 && [...key].length === 1
        ? { kind: "char", ch: key, down: false, modifiers: modifiers(event) }
        : { kind: "key", code, down: false, modifiers: modifiers(event) });
    }
  };

  canvas.addEventListener("mousemove", onMouseMove);
  canvas.addEventListener("mousedown", onMouseDown);
  canvas.addEventListener("mouseup", onMouseUp);
  canvas.addEventListener("wheel", onWheel);
  canvas.addEventListener("keydown", onKeyDown);
  canvas.addEventListener("keyup", onKeyUp);
  const detachTouch = attachTouch(canvas, queue);

  return () => {
    canvas.removeEventListener("mousemove", onMouseMove);
    canvas.removeEventListener("mousedown", onMouseDown);
    canvas.removeEventListener("mouseup", onMouseUp);
    canvas.removeEventListener("wheel", onWheel);
    canvas.removeEventListener("keydown", onKeyDown);
    canvas.removeEventListener("keyup", onKeyUp);
    detachTouch();
  };
}

/** How far a finger may wander and still tap or long-press, in pixels. */
const TAP_SLOP = 10;
/** Holding a finger still this long right-clicks. */
export const LONG_PRESS_MS = 500;
/** A two-finger gesture that changes the finger distance by this much zooms. */
const PINCH_THRESHOLD = 0.08;
export const MAX_ZOOM = 5;

interface TouchPoint { x: number; y: number; startX: number; startY: number }

/**
 * Touch screens: a tap clicks, a long press right-clicks, a one-finger drag
 * drags with the left button, a two-finger drag scrolls, and a pinch zooms
 * the picture on this side (the remote screen is unchanged; a pinch back
 * below 1x resets it). Mouse and pen keep using the mouse handlers.
 */
function attachTouch(canvas: HTMLCanvasElement, queue: InputQueue): () => void {
  const touches = new Map<number, TouchPoint>();
  // press: one finger, not yet a tap, long press or drag. two: two fingers,
  // not yet a scroll or pinch. done: ignore until every finger is up.
  let gesture: "none" | "press" | "drag" | "two" | "scroll" | "pinch" | "done" = "none";
  let longPress: ReturnType<typeof setTimeout> | undefined;
  const scroller = new NotchAccumulator(TOUCH_PIXELS_PER_NOTCH);
  const zoom = { scale: 1, x: 0, y: 0 };
  let pinch = { scale: 1, x: 0, y: 0, distance: 1, cx: 0, cy: 0 };
  let last = { cx: 0, cy: 0 };

  if (canvas.style) {
    canvas.style.touchAction = "none";
    canvas.style.transformOrigin = "0 0";
  }

  const at = (clientX: number, clientY: number) => mapPointer(clientX, clientY, canvas.getBoundingClientRect(), canvas.width, canvas.height);
  const clearLongPress = () => { if (longPress !== undefined) { clearTimeout(longPress); longPress = undefined; } };

  // The canvas's untransformed top-left, in client pixels.
  const origin = () => {
    const parent = canvas.offsetParent?.getBoundingClientRect();
    return { left: (parent?.left ?? 0) + canvas.offsetLeft, top: (parent?.top ?? 0) + canvas.offsetTop };
  };
  const two = () => {
    const [a, b] = [...touches.values()];
    const base = origin();
    return { distance: Math.max(1, Math.hypot(a.x - b.x, a.y - b.y)), cx: (a.x + b.x) / 2 - base.left, cy: (a.y + b.y) / 2 - base.top };
  };
  const showZoom = () => {
    const width = canvas.offsetWidth;
    const height = canvas.offsetHeight;
    // Keep the zoomed picture covering the view.
    zoom.x = Math.min(0, Math.max(width - width * zoom.scale, zoom.x));
    zoom.y = Math.min(0, Math.max(height - height * zoom.scale, zoom.y));
    if (canvas.style) canvas.style.transform = zoom.scale === 1 ? "" : `translate(${zoom.x}px, ${zoom.y}px) scale(${zoom.scale})`;
  };

  const onPointerDown = (event: PointerEvent): void => {
    if (event.pointerType !== "touch") return;
    // No emulated mouse events, scrolling or browser zoom for this touch.
    event.preventDefault();
    canvas.setPointerCapture?.(event.pointerId);
    touches.set(event.pointerId, { x: event.clientX, y: event.clientY, startX: event.clientX, startY: event.clientY });
    if (touches.size === 1) {
      gesture = "press";
      const touch = touches.get(event.pointerId)!;
      longPress = setTimeout(() => {
        longPress = undefined;
        if (gesture !== "press") return;
        gesture = "done";
        const position = at(touch.startX, touch.startY);
        if (!position) return;
        queue.enqueueMove({ kind: "mouseMove", ...position });
        queue.click(2);
      }, LONG_PRESS_MS);
    } else if (touches.size === 2 && gesture !== "done") {
      clearLongPress();
      // A second finger ends a one-finger drag.
      if (gesture === "drag") queue.enqueue({ kind: "mouseButton", button: 1, down: false });
      gesture = "two";
      const start = two();
      pinch = { ...zoom, ...start };
      last = { cx: start.cx, cy: start.cy };
    }
  };

  const onPointerMove = (event: PointerEvent): void => {
    const touch = touches.get(event.pointerId);
    if (!touch) return;
    event.preventDefault();
    touch.x = event.clientX;
    touch.y = event.clientY;
    if (gesture === "press" && Math.hypot(touch.x - touch.startX, touch.y - touch.startY) > TAP_SLOP) {
      clearLongPress();
      const start = at(touch.startX, touch.startY);
      if (start) {
        gesture = "drag";
        queue.enqueueMove({ kind: "mouseMove", ...start });
        queue.enqueue({ kind: "mouseButton", button: 1, down: true });
      } else {
        gesture = "done";
      }
    }
    if (gesture === "drag") {
      const position = at(touch.x, touch.y);
      if (position) queue.enqueueMove({ kind: "mouseMove", ...position });
      return;
    }
    if (touches.size < 2 || (gesture !== "two" && gesture !== "scroll" && gesture !== "pinch")) return;
    const now = two();
    if (gesture === "two") {
      if (Math.abs(now.distance / pinch.distance - 1) > PINCH_THRESHOLD) gesture = "pinch";
      else if (Math.hypot(now.cx - pinch.cx, now.cy - pinch.cy) > TAP_SLOP) {
        // Count the movement from the start, not just from here.
        gesture = "scroll";
        last = { cx: pinch.cx, cy: pinch.cy };
      }
    }
    if (gesture === "scroll") {
      // Fingers moving up show what is further down, like a touch screen.
      const notches = scroller.add(last.cx - now.cx, last.cy - now.cy);
      if (notches.dx !== 0 || notches.dy !== 0) queue.enqueue({ kind: "scroll", ...notches });
    } else if (gesture === "pinch") {
      // Keep the point of the picture under the fingers' centre there.
      const pointX = (pinch.cx - pinch.x) / pinch.scale;
      const pointY = (pinch.cy - pinch.y) / pinch.scale;
      zoom.scale = Math.min(MAX_ZOOM, Math.max(1, pinch.scale * now.distance / pinch.distance));
      zoom.x = now.cx - pointX * zoom.scale;
      zoom.y = now.cy - pointY * zoom.scale;
      showZoom();
    }
    last = { cx: now.cx, cy: now.cy };
  };

  const onPointerEnd = (event: PointerEvent): void => {
    const touch = touches.get(event.pointerId);
    if (!touch) return;
    touches.delete(event.pointerId);
    if (gesture === "press" && touches.size === 0) {
      clearLongPress();
      const position = event.type === "pointerup" ? at(touch.startX, touch.startY) : null;
      if (position) {
        queue.enqueueMove({ kind: "mouseMove", ...position });
        queue.click(1);
      }
    } else if (gesture === "drag") {
      const position = at(touch.x, touch.y);
      if (position) queue.enqueueMove({ kind: "mouseMove", ...position });
      queue.enqueue({ kind: "mouseButton", button: 1, down: false });
      gesture = "done";
    }
    if (touches.size === 0) {
      gesture = "none";
      if (zoom.scale < 1.05) {
        zoom.scale = 1;
        showZoom();
      }
    } else if (gesture !== "none") {
      // Lifting one finger of two ends that gesture; start over once all are up.
      gesture = "done";
    }
  };

  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerup", onPointerEnd);
  canvas.addEventListener("pointercancel", onPointerEnd);

  return () => {
    clearLongPress();
    canvas.removeEventListener("pointerdown", onPointerDown);
    canvas.removeEventListener("pointermove", onPointerMove);
    canvas.removeEventListener("pointerup", onPointerEnd);
    canvas.removeEventListener("pointercancel", onPointerEnd);
    if (canvas.style) canvas.style.transform = "";
  };
}

/** Key codes for {@link KeyboardBridge.press}, as the host maps them. */
export const KEY = { backspace: 8, tab: 9, enter: 13, escape: 27, pageUp: 33, pageDown: 34, end: 35, home: 36, left: 37, up: 38, right: 39, down: 40, delete: 46 } as const;
/** Modifier bits for {@link KeyboardBridge.press} and {@link KeyboardBridge.setModifiers}. */
export const MODIFIER = { shift: 1, ctrl: 1 << 1, alt: 1 << 2, meta: 1 << 3 } as const;

export interface KeyboardBridge {
  /** Press and release a key, like Escape or an arrow. */
  press(code: number, modifiers?: number): void;
  /** Modifiers held for the next key or character typed; `onKeystroke` fires after it. */
  setModifiers(modifiers: number): void;
  detach(): void;
}

/** What the field holds while empty, so a backspace still has something to delete. */
const SENTINEL = " ";

/**
 * Type into the remote computer from a text field, for phone and tablet
 * keyboards, which don't send usable key events. The field keeps what was
 * typed since it was last cleared, and each change is sent as the
 * backspaces and characters that turn the old text into the new, so
 * autocorrect, suggestions and composed (IME) text reach the remote side too.
 */
export function attachKeyboard(
  field: HTMLInputElement | HTMLTextAreaElement,
  send: (bytes: Uint8Array) => Promise<void>,
  onKeystroke: () => void = () => {},
): KeyboardBridge {
  const queue = new InputQueue(send);
  let mirrored = SENTINEL;
  let composing = false;
  let held = 0;
  field.value = SENTINEL;

  const used = () => {
    if (held === 0) return;
    held = 0;
    onKeystroke();
  };
  const press = (code: number, modifiers = held) => {
    queue.press(code, modifiers);
    used();
  };
  const sync = () => {
    const before = [...mirrored];
    const after = [...field.value];
    let common = 0;
    while (common < before.length && common < after.length && before[common] === after[common]) common += 1;
    for (let index = common; index < before.length; index += 1) queue.press(KEY.backspace);
    for (const ch of after.slice(common)) {
      if (ch === "\n") queue.press(KEY.enter, held);
      else queue.type(ch, held);
      used();
    }
    mirrored = field.value;
    // Start over now and then, but never in the middle of a composition.
    if (!composing && (mirrored.length > 32 || !mirrored.startsWith(SENTINEL))) {
      field.value = SENTINEL;
      mirrored = SENTINEL;
    }
  };

  const onInput = () => sync();
  const onCompositionStart = () => { composing = true; };
  const onCompositionEnd = () => { composing = false; sync(); };
  // Keys that change no text: Enter in a single-line field, and what a
  // hardware keyboard attached to a tablet adds.
  const onKeyDown = (event: KeyboardEvent) => {
    const keys: Record<string, number> = { Enter: KEY.enter, Tab: KEY.tab, Escape: KEY.escape, ArrowLeft: KEY.left, ArrowUp: KEY.up, ArrowRight: KEY.right, ArrowDown: KEY.down, Home: KEY.home, End: KEY.end, PageUp: KEY.pageUp, PageDown: KEY.pageDown, Delete: KEY.delete };
    const code = keys[event.key];
    if (code === undefined || composing) return;
    event.preventDefault();
    press(code);
  };

  field.addEventListener("input", onInput);
  field.addEventListener("compositionstart", onCompositionStart);
  field.addEventListener("compositionend", onCompositionEnd);
  field.addEventListener("keydown", onKeyDown as EventListener);

  return {
    press,
    setModifiers: (modifiers: number) => { held = modifiers; },
    detach: () => {
      field.removeEventListener("input", onInput);
      field.removeEventListener("compositionstart", onCompositionStart);
      field.removeEventListener("compositionend", onCompositionEnd);
      field.removeEventListener("keydown", onKeyDown as EventListener);
    },
  };
}
