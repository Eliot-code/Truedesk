// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import ChatPanel, { type ChatEntry } from "./ChatPanel";

const messages: ChatEntry[] = [
  { from: "peer", text: "hi there", at: 1 },
  { from: "me", text: "hello back", at: 2 },
];

describe("ChatPanel", () => {
  afterEach(cleanup);

  it("renders the transcript", () => {
    render(<ChatPanel open messages={messages} onSend={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText("hi there")).toBeTruthy();
    expect(screen.getByText("hello back")).toBeTruthy();
  });

  it("sends trimmed text on Enter and clears the input", () => {
    const onSend = vi.fn();
    render(<ChatPanel open messages={[]} onSend={onSend} onClose={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Message" }) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "  spaced  " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSend).toHaveBeenCalledWith("spaced");
    expect(input.value).toBe("");
  });

  it("ignores empty or whitespace-only messages", () => {
    const onSend = vi.fn();
    render(<ChatPanel open messages={[]} onSend={onSend} onClose={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Message" });
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onSend).not.toHaveBeenCalled();
  });
});
