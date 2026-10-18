// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import Permissions from "./Permissions";
import * as api from "../lib/api";

vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  return { ...actual, permissionStatus: vi.fn(), openPermissionSettings: vi.fn() };
});

describe("Permissions", () => {
  afterEach(cleanup);

  it("shows each permission state and offers to skip while one is missing", async () => {
    vi.mocked(api.permissionStatus).mockResolvedValue({ screen_recording: true, accessibility: false });
    const onContinue = vi.fn();
    render(<Permissions onContinue={onContinue} />);
    expect(await screen.findByText("Granted")).toBeTruthy();
    expect(screen.getByText("Required")).toBeTruthy();
    expect(screen.getByText("1 of 2 granted")).toBeTruthy();
    expect(screen.getByText(/remove it with −, add it again with \+, then restart Truedesk/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Skip for now" }));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(onContinue).toHaveBeenCalledTimes(2);
  });

  it("enables Continue once both permissions are granted", async () => {
    vi.mocked(api.permissionStatus).mockResolvedValue({ screen_recording: true, accessibility: true });
    const onContinue = vi.fn();
    render(<Permissions onContinue={onContinue} />);
    const continueButton = await screen.findByRole("button", { name: "Continue" });
    expect(screen.queryByText(/remove it with −/)).toBeNull();
    await waitFor(() => expect(continueButton).toHaveProperty("disabled", false));
    fireEvent.click(continueButton);
    expect(onContinue).toHaveBeenCalledOnce();
  });
});
