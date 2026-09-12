// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import Toolbar from "./Toolbar";

describe("Toolbar", () => {
  afterEach(cleanup);

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
});
