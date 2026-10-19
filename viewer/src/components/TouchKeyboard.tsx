import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { attachKeyboard, KEY, MODIFIER, type KeyboardBridge } from "../lib/input";

export interface TouchKeyboardHandle {
  /** Open the on-screen keyboard. Call it straight from a tap handler: iOS only shows the keyboard for a focus a tap caused. */
  show(): void;
}

export interface TouchKeyboardProps {
  /** Sends encoded input events to the remote computer. */
  send: (bytes: Uint8Array) => Promise<void>;
}

/** Whether this device has a touch screen, so it may need the on-screen keyboard. */
export function hasTouchScreen(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(any-pointer: coarse)").matches;
}

const STICKY = [
  { label: "Ctrl", bit: MODIFIER.ctrl },
  { label: "Alt", bit: MODIFIER.alt },
  { label: "Win", bit: MODIFIER.meta },
];

const KEYS = [
  { label: "Esc", code: KEY.escape },
  { label: "Tab", code: KEY.tab },
  { label: "←", name: "Left", code: KEY.left },
  { label: "↑", name: "Up", code: KEY.up },
  { label: "↓", name: "Down", code: KEY.down },
  { label: "→", name: "Right", code: KEY.right },
];

/**
 * Typing on phones and tablets: a hidden field the on-screen keyboard types
 * into, mirrored to the remote computer, and a row of keys phone keyboards
 * lack. Ctrl, Alt and Win apply to the next key or character.
 */
const TouchKeyboard = forwardRef<TouchKeyboardHandle, TouchKeyboardProps>(function TouchKeyboard({ send }, ref) {
  const fieldRef = useRef<HTMLInputElement>(null);
  const bridgeRef = useRef<KeyboardBridge | null>(null);
  const [open, setOpen] = useState(false);
  const [held, setHeld] = useState(0);

  useImperativeHandle(ref, () => ({ show: () => fieldRef.current?.focus() }), []);

  useEffect(() => {
    const field = fieldRef.current;
    if (!field) return;
    const bridge = attachKeyboard(field, send, () => setHeld(0));
    bridgeRef.current = bridge;
    return () => { bridge.detach(); bridgeRef.current = null; };
  }, [send]);

  const toggle = (bit: number) => {
    const next = held ^ bit;
    setHeld(next);
    bridgeRef.current?.setModifiers(next);
  };

  // Keep the field focused (and the keyboard up) when a key here is tapped.
  const keepFocus = (event: React.PointerEvent | React.MouseEvent) => event.preventDefault();

  return (
    <>
      <input
        ref={fieldRef}
        className="touch-keyboard__field"
        aria-label="Type on the remote computer"
        autoCapitalize="off"
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        onFocus={() => setOpen(true)}
        onBlur={() => { setOpen(false); setHeld(0); bridgeRef.current?.setModifiers(0); }}
      />
      {open && (
        <div className="touch-keyboard__keys" role="toolbar" aria-label="Special keys">
          {STICKY.map(({ label, bit }) => (
            <button key={label} type="button" aria-pressed={(held & bit) !== 0} onPointerDown={keepFocus} onMouseDown={keepFocus} onClick={() => toggle(bit)}>{label}</button>
          ))}
          {KEYS.map(({ label, name, code }) => (
            <button key={label} type="button" aria-label={name ?? label} onPointerDown={keepFocus} onMouseDown={keepFocus} onClick={() => bridgeRef.current?.press(code)}>{label}</button>
          ))}
        </div>
      )}
    </>
  );
});

export default TouchKeyboard;
