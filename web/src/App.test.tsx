// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { FakeCore, FakeSocket } from "./lib/fakes";

// jsdom has no canvas or WebCodecs; the session screen's decoder is not under test.
vi.mock("../../viewer/src/lib/decode", () => ({ makeDecoder: () => ({ push: vi.fn(), close: vi.fn() }) }));

function fill(id: string, password: string, code = "") {
  fireEvent.change(screen.getByLabelText("Device ID"), { target: { value: id } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: password } });
  fireEvent.change(screen.getByLabelText("Two-factor code"), { target: { value: code } });
  fireEvent.click(screen.getByRole("button", { name: "Connect" }));
}

beforeEach(() => {
  FakeSocket.all = [];
  localStorage.clear();
});
afterEach(cleanup);

describe("App", () => {
  it("checks the form before connecting", () => {
    const loadCore = vi.fn();
    render(<App loadCore={loadCore} unsupported={null} />);
    fill("123", "pw");
    expect(screen.getByRole("alert").textContent).toBe("Enter the 9-digit ID.");
    fill("123 456 789", "pw", "12");
    expect(screen.getByRole("alert").textContent).toBe("The two-factor code has 6 digits.");
    expect(loadCore).not.toHaveBeenCalled();
  });

  it("runs a session and shows why it ended", async () => {
    const core = new FakeCore();
    core.setup = (viewer) => {
      viewer.onReceive = ([first]) => first === 1 ? [{ kind: "authenticated" }] : [{ kind: "ended", text: "session rejected by host: denied" }];
    };
    core.decide = () => ({ retry: false, delayMs: 0, message: "The host declined the connection request." });
    render(<App loadCore={async () => core} openSocket={(url) => new FakeSocket(url)} unsupported={null} />);
    await act(async () => fill("123-456-789", "secret", "123456"));
    expect(core.viewers[0].password).toBe("secret");
    expect(core.viewers[0].authCode).toBe("123456");
    const socket = FakeSocket.all[0];
    act(() => socket.open());
    act(() => socket.message([1]));
    expect(screen.getByText(/to accept/).textContent).toContain("123 456 789");
    act(() => socket.message([2]));
    expect(screen.getByRole("alert").textContent).toBe("The host declined the connection request.");
  });

  it("disconnects from the toolbar", async () => {
    const core = new FakeCore();
    render(<App loadCore={async () => core} openSocket={(url) => new FakeSocket(url)} unsupported={null} />);
    await act(async () => fill("123456789", "pw"));
    act(() => FakeSocket.all[0].open());
    fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(FakeSocket.all[0].sent.at(-1)).toEqual([0x01, 0xbb]);
    expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("blocks browsers without WebCodecs", () => {
    render(<App loadCore={vi.fn()} unsupported="no WebCodecs here" />);
    expect(screen.getByRole("alert").textContent).toBe("no WebCodecs here");
    expect((screen.getByRole("button", { name: "Connect" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
