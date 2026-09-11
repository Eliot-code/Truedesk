import { Tag, wrap } from "./protocol";

export type InputEvent =
  | { kind: "mouseMove"; x: number; y: number }
  | { kind: "mouseButton"; button: number; down: boolean }
  | { kind: "scroll"; dx: number; dy: number }
  | { kind: "key"; code: number; down: boolean; modifiers: number };

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
  }
  return bytes;
}

function timestampUs(): number {
  return Math.floor(Date.now() * 1000);
}

function normalizedCoordinate(value: number, start: number, size: number): number {
  if (size <= 0) return 0;
  const normalized = ((value - start) / size) * 65535;
  return Math.max(0, Math.min(65535, Math.round(normalized)));
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

/** Capture canvas input, seal each event, and send it as a Tag.Input frame. */
export function attachInput(
  ws: WebSocket,
  canvas: HTMLCanvasElement,
  seal: (bytes: Uint8Array) => Promise<Uint8Array>,
): void {
  if (canvas.tabIndex < 0) canvas.tabIndex = 0;

  const send = async (event: InputEvent): Promise<void> => {
    const sealed = await seal(encodeInputEvent(event, timestampUs()));
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(wrap(Tag.Input, sealed) as Uint8Array<ArrayBuffer>);
    }
  };

  canvas.addEventListener("mousemove", (event) => {
    const rect = canvas.getBoundingClientRect();
    void send({
      kind: "mouseMove",
      x: normalizedCoordinate(event.clientX, rect.left, rect.width),
      y: normalizedCoordinate(event.clientY, rect.top, rect.height),
    });
  });

  canvas.addEventListener("mousedown", (event) => {
    canvas.focus();
    const mapped = button(event.button);
    if (mapped !== null) void send({ kind: "mouseButton", button: mapped, down: true });
  });

  canvas.addEventListener("mouseup", (event) => {
    const mapped = button(event.button);
    if (mapped !== null) void send({ kind: "mouseButton", button: mapped, down: false });
  });

  canvas.addEventListener("wheel", (event) => {
    void send({ kind: "scroll", dx: Math.trunc(event.deltaX), dy: Math.trunc(event.deltaY) });
  });

  canvas.addEventListener("keydown", (event) => {
    event.preventDefault();
    void send({ kind: "key", code: event.keyCode >>> 0, down: true, modifiers: modifiers(event) });
  });

  canvas.addEventListener("keyup", (event) => {
    event.preventDefault();
    void send({ kind: "key", code: event.keyCode >>> 0, down: false, modifiers: modifiers(event) });
  });
}
