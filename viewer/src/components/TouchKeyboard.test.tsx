// @vitest-environment jsdom
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import TouchKeyboard, { hasTouchScreen, type TouchKeyboardHandle } from "./TouchKeyboard";

/** The kind byte, key or character, and modifier byte of each sent event. */
const events = (send: ReturnType<typeof vi.fn>) => send.mock.calls.map(([bytes]) => {
  const data = bytes as Uint8Array;
  const value = new DataView(data.buffer).getUint32(9);
  return `${data[0] === 5 ? String.fromCodePoint(value) : value} ${data[13] ? "down" : "up"} ${data[14]}`;
});

describe("TouchKeyboard", () => {
  afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

  it("opens on show, then offers the keys phone keyboards lack", async () => {
    const send = vi.fn(() => Promise.resolve());
    const ref = createRef<TouchKeyboardHandle>();
    const view = render(<TouchKeyboard ref={ref} send={send} />);
    expect(view.queryByRole("toolbar", { name: "Special keys" })).toBeNull();
    act(() => ref.current!.show());
    const field = view.getByLabelText("Type on the remote computer");
    expect(document.activeElement).toBe(field);
    fireEvent.click(view.getByRole("button", { name: "Esc" }));
    fireEvent.click(view.getByRole("button", { name: "Left" }));
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    expect(events(send)).toEqual(["27 down 0", "27 up 0", "37 down 0", "37 up 0"]);
  });

  it("holds Ctrl for the next character only", async () => {
    const send = vi.fn(() => Promise.resolve());
    const ref = createRef<TouchKeyboardHandle>();
    const view = render(<TouchKeyboard ref={ref} send={send} />);
    act(() => ref.current!.show());
    const ctrl = view.getByRole("button", { name: "Ctrl" });
    fireEvent.click(ctrl);
    expect(ctrl.getAttribute("aria-pressed")).toBe("true");
    const field = view.getByLabelText("Type on the remote computer") as HTMLInputElement;
    fireEvent.input(field, { target: { value: " c" } });
    expect(ctrl.getAttribute("aria-pressed")).toBe("false");
    fireEvent.input(field, { target: { value: " cv" } });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    expect(events(send)).toEqual(["c down 2", "c up 2", "v down 0", "v up 0"]);
  });

  it("closes the keys when the keyboard goes away", () => {
    const ref = createRef<TouchKeyboardHandle>();
    const view = render(<TouchKeyboard ref={ref} send={() => Promise.resolve()} />);
    act(() => ref.current!.show());
    expect(view.getByRole("toolbar", { name: "Special keys" })).toBeTruthy();
    fireEvent.blur(view.getByLabelText("Type on the remote computer"));
    expect(view.queryByRole("toolbar", { name: "Special keys" })).toBeNull();
  });

  it("knows a touch screen when it sees one", () => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query === "(any-pointer: coarse)" }));
    expect(hasTouchScreen()).toBe(true);
    vi.stubGlobal("matchMedia", () => ({ matches: false }));
    expect(hasTouchScreen()).toBe(false);
  });
});
