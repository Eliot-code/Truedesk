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

  it("shows each permission state and gates Continue", async () => {
    vi.mocked(api.permissionStatus).mockResolvedValue({ screen_recording: true, accessibility: false });
    render(<Permissions onContinue={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Continue" })).toHaveProperty("disabled", true);
    expect(await screen.findByText("Granted")).toBeTruthy();
    expect(screen.getByText("Required")).toBeTruthy();
    expect(screen.getByText("1 of 2 granted")).toBeTruthy();
  });

  it("enables Continue once both permissions are granted", async () => {
    vi.mocked(api.permissionStatus).mockResolvedValue({ screen_recording: true, accessibility: true });
    const onContinue = vi.fn();
    render(<Permissions onContinue={onContinue} />);
    const continueButton = await screen.findByRole("button", { name: "Continue" });
    await waitFor(() => expect(continueButton).toHaveProperty("disabled", false));
    fireEvent.click(continueButton);
    expect(onContinue).toHaveBeenCalledOnce();
  });
});
