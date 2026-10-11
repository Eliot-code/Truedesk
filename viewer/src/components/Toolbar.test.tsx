// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import Toolbar, { monitorName } from "./Toolbar";

describe("monitorName", () => {
  it("turns Windows device paths into Display N", () => {
    expect(monitorName("\\\\.\\DISPLAY2", 1)).toBe("Display 2");
  });

  it("keeps readable names and falls back to the position", () => {
    expect(monitorName("Built-in Retina Display", 0)).toBe("Built-in Retina Display");
    expect(monitorName("  ", 2)).toBe("Display 3");
  });
});

describe("Toolbar", () => {
  afterEach(cleanup);

  it("shows a visible text label on each viewer control", () => {
    render(<Toolbar remoteId="123456789" onDisconnect={vi.fn()} />);
    for (const label of ["Fullscreen", "Clipboard", "Sound", "Files", "Record"]) {
      expect(screen.getByText(label)).toBeTruthy();
    }
  });

  it("calls onDisconnect when Disconnect is clicked", () => {
    const onDisconnect = vi.fn();
    render(<Toolbar remoteId="123456789" onDisconnect={onDisconnect} />);
    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(onDisconnect).toHaveBeenCalledOnce();
  });

  it("shows the remote ID and an MM:SS timer", () => {
    render(<Toolbar remoteId="987654321" onDisconnect={vi.fn()} />);
    expect(screen.getByText("987654321")).toBeTruthy();
    expect(screen.getByLabelText("Session timer").textContent).toMatch(/^\d{2}:\d{2}$/);
  });

  it("calls the recording handler for start and stop", () => {
    const onRecordingToggle = vi.fn();
    const { rerender } = render(<Toolbar remoteId="123" onDisconnect={vi.fn()} onRecordingToggle={onRecordingToggle} />);
    fireEvent.click(screen.getByRole("button", { name: "Record" }));
    rerender(<Toolbar remoteId="123" onDisconnect={vi.fn()} recordingActive onRecordingToggle={onRecordingToggle} />);
    fireEvent.click(screen.getByRole("button", { name: "Stop recording" }));
    expect(onRecordingToggle).toHaveBeenCalledTimes(2);
  });

  it("exposes toggle state through aria-pressed and titles", () => {
    const { rerender } = render(<Toolbar remoteId="123" onDisconnect={vi.fn()} clipboardSync audioMuted={false} />);
    const clipboard = screen.getByRole("button", { name: "Sync clipboard" });
    const audio = screen.getByRole("button", { name: "Mute audio" });
    expect(clipboard.getAttribute("aria-pressed")).toBe("true");
    expect(clipboard.title).toMatch(/is on/);
    expect(audio.getAttribute("aria-pressed")).toBe("false");
    rerender(<Toolbar remoteId="123" onDisconnect={vi.fn()} clipboardSync={false} audioMuted />);
    expect(clipboard.getAttribute("aria-pressed")).toBe("false");
    expect(clipboard.title).toMatch(/is off/);
    expect(audio.getAttribute("aria-pressed")).toBe("true");
    expect(audio.title).toMatch(/muted/);
  });

  it("hides controls that do nothing yet", () => {
    render(<Toolbar remoteId="123" onDisconnect={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Fit / Scale" })).toBeNull();
    expect(screen.queryByLabelText("Quality")).toBeNull();
    expect(screen.queryByRole("button", { name: "Keyboard" })).toBeNull();
  });

  it("colours the connection dot by status", () => {
    const { container, rerender } = render(<Toolbar remoteId="123" status="in_session" onDisconnect={vi.fn()} />);
    const dot = container.querySelector(".session-toolbar__connected-dot")!;
    expect(dot.getAttribute("data-status")).toBe("in_session");
    rerender(<Toolbar remoteId="123" status="connecting" onDisconnect={vi.fn()} />);
    expect(dot.getAttribute("data-status")).toBe("connecting");
    expect(screen.getByText("Reconnecting")).toBeTruthy();
  });
});
