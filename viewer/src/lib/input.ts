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

/** Capture canvas input and send raw InputEvent bytes in FIFO order. */
export function attachInput(
  canvas: HTMLCanvasElement,
  send: (bytes: Uint8Array) => Promise<void>,
): () => void {
  if (canvas.tabIndex < 0) canvas.tabIndex = 0;

  let sendChain = Promise.resolve();
  const enqueue = (event: InputEvent): void => {
    const bytes = encodeInputEvent(event, timestampUs());
    sendChain = sendChain
      .then(() => send(bytes))
      .catch((error: unknown) => {
        console.error("Failed to send input event", error);
      });
  };

  const onMouseMove = (event: MouseEvent): void => {
    const rect = canvas.getBoundingClientRect();
    enqueue({
      kind: "mouseMove",
      x: normalizedCoordinate(event.clientX, rect.left, rect.width),
      y: normalizedCoordinate(event.clientY, rect.top, rect.height),
    });
  };

  const onMouseDown = (event: MouseEvent): void => {
    canvas.focus();
    const mapped = button(event.button);
    if (mapped !== null) enqueue({ kind: "mouseButton", button: mapped, down: true });
  };

  const onMouseUp = (event: MouseEvent): void => {
    const mapped = button(event.button);
    if (mapped !== null) enqueue({ kind: "mouseButton", button: mapped, down: false });
  };

  const onWheel = (event: WheelEvent): void => {
    enqueue({ kind: "scroll", dx: Math.trunc(event.deltaX), dy: Math.trunc(event.deltaY) });
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    event.preventDefault();
    enqueue({ kind: "key", code: event.keyCode >>> 0, down: true, modifiers: modifiers(event) });
  };

  const onKeyUp = (event: KeyboardEvent): void => {
    event.preventDefault();
    enqueue({ kind: "key", code: event.keyCode >>> 0, down: false, modifiers: modifiers(event) });
  };

  canvas.addEventListener("mousemove", onMouseMove);
  canvas.addEventListener("mousedown", onMouseDown);
  canvas.addEventListener("mouseup", onMouseUp);
  canvas.addEventListener("wheel", onWheel);
  canvas.addEventListener("keydown", onKeyDown);
  canvas.addEventListener("keyup", onKeyUp);

  return () => {
    canvas.removeEventListener("mousemove", onMouseMove);
    canvas.removeEventListener("mousedown", onMouseDown);
    canvas.removeEventListener("mouseup", onMouseUp);
    canvas.removeEventListener("wheel", onWheel);
    canvas.removeEventListener("keydown", onKeyDown);
    canvas.removeEventListener("keyup", onKeyUp);
  };
}
