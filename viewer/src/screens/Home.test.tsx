// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Home from "./Home";
import * as api from "../lib/api";
import { NotifyContext } from "../lib/notify";

vi.mock("../lib/api", async () => {
  const actual = await vi.importActual<typeof import("../lib/api")>("../lib/api");
  return { ...actual, getIdentity: vi.fn(), getConfig: vi.fn(), getUnattendedStatus: vi.fn(), listContacts: vi.fn(), listAllowed: vi.fn(), startHosting: vi.fn(), connect: vi.fn(), setUnattendedAccess: vi.fn(), setUnattended2fa: vi.fn(), confirmUnattended2fa: vi.fn(), rotatePassword: vi.fn(), addContact: vi.fn() };
});
describe("Home", () => {
  afterEach(cleanup);
  beforeEach(() => {
    vi.mocked(api.getIdentity).mockResolvedValue({ id: "123456789", password: "654321" });
    vi.mocked(api.getConfig).mockResolvedValue({ relay_url: "wss://relay", relay_urls: ["wss://relay"], recent_ids: [], clipboard_sync: false });
    vi.mocked(api.getUnattendedStatus).mockResolvedValue({ enabled: false, has_password: false, twofa_enabled: false });
    vi.mocked(api.listContacts).mockResolvedValue([]);
    vi.mocked(api.listAllowed).mockResolvedValue([]);
    vi.mocked(api.startHosting).mockResolvedValue(undefined); vi.mocked(api.connect).mockResolvedValue(undefined);
  });
  it("renders the identity returned by the backend", async () => { render(<Home status="online" onConnect={vi.fn()} incomingRequest={null} onDismissRequest={vi.fn()} />); expect(await screen.findByText("123456789")).toBeTruthy(); });
  it("reveals the session password on demand", async () => {
    render(<Home status="online" onConnect={vi.fn()} incomingRequest={null} onDismissRequest={vi.fn()} />);
    const field = await screen.findByLabelText("Session password") as HTMLInputElement;
    await waitFor(() => expect(field.value).toBe("654321"));
    expect(field.type).toBe("password");
    fireEvent.click(screen.getByRole("button", { name: "Show password" }));
    expect(field.type).toBe("text");
    fireEvent.click(screen.getByRole("button", { name: "Hide password" }));
    expect(field.type).toBe("password");
  });
  it("connects with the entered target ID and password", async () => {
    const onConnect = vi.fn().mockResolvedValue(undefined);
    const prompt = vi.spyOn(window, "prompt").mockReturnValue(null);
    render(<Home status="online" onConnect={onConnect} incomingRequest={null} onDismissRequest={vi.fn()} />);
    fireEvent.change(screen.getByPlaceholderText("000 000 000"), { target: { value: "987654321" } }); fireEvent.change(screen.getByPlaceholderText("Password"), { target: { value: "remote-secret" } }); fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(onConnect).toHaveBeenCalledWith("987654321", "remote-secret"));
    expect(prompt).not.toHaveBeenCalled();
  });
  it("passes an entered 2FA code when connecting", async () => {
    const onConnect = vi.fn().mockResolvedValue(undefined);
    render(<Home status="online" onConnect={onConnect} incomingRequest={null} onDismissRequest={vi.fn()} />);
    fireEvent.change(screen.getByPlaceholderText("000 000 000"), { target: { value: "987654321" } }); fireEvent.change(screen.getByPlaceholderText("Password"), { target: { value: "remote-secret" } }); fireEvent.change(screen.getByLabelText("2FA code (optional)"), { target: { value: "123456" } }); fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(onConnect).toHaveBeenCalledWith("987654321", "remote-secret", "123456"));
  });

  it("fills a contact's ID and focuses the password instead of connecting", async () => {
    vi.mocked(api.listContacts).mockResolvedValue([{ id: "111222333", alias: "Office", last_connected: null }]);
    const onConnect = vi.fn();
    render(<Home status="online" onConnect={onConnect} />);
    fireEvent.change(screen.getByPlaceholderText("Password"), { target: { value: "typed-for-someone-else" } });
    await screen.findByText("Office");
    // The first "Connect" is the form submit; pick the contact row's button.
    const contactConnect = screen.getAllByRole("button", { name: "Connect" }).find((button) => button.closest(".contact-row"))!;
    fireEvent.click(contactConnect);
    expect((screen.getByPlaceholderText("000 000 000") as HTMLInputElement).value).toBe("111222333");
    expect((screen.getByPlaceholderText("Password") as HTMLInputElement).value).toBe("");
    expect(document.activeElement).toBe(screen.getByPlaceholderText("Password"));
    expect(onConnect).not.toHaveBeenCalled();
  });

  it("rejects contact IDs that are not 9 digits", async () => {
    const notify = vi.fn();
    render(<NotifyContext.Provider value={notify}><Home status="online" onConnect={vi.fn()} /></NotifyContext.Provider>);
    fireEvent.change(screen.getByLabelText("Contact alias"), { target: { value: "Office" } });
    fireEvent.change(screen.getByLabelText("Contact device ID"), { target: { value: "1234" } });
    fireEvent.click(screen.getAllByRole("button", { name: "Add" })[0]);
    expect(notify).toHaveBeenCalledWith("Enter the 9-digit ID.", "error");
    expect(api.addContact).not.toHaveBeenCalled();
  });

  it("enables the unattended checkbox only with a password, and reverts it when saving fails", async () => {
    vi.mocked(api.setUnattendedAccess).mockRejectedValue("unattended access password must be at least 12 characters");
    const notify = vi.fn();
    render(<NotifyContext.Provider value={notify}><Home status="online" onConnect={vi.fn()} /></NotifyContext.Provider>);
    const enabled = screen.getByRole("checkbox", { name: "Enabled" }) as HTMLInputElement;
    expect(screen.getByPlaceholderText("Set a password (12+ characters)")).toBeTruthy();
    expect(enabled.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Unattended access password"), { target: { value: "short" } });
    expect(enabled.disabled).toBe(false);
    fireEvent.click(enabled);
    expect(enabled.checked).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save settings" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("Use at least 12 characters for the unattended access password.", "error"));
    expect(enabled.checked).toBe(false);
  });

  it("enables 2FA only after a code from the authenticator is confirmed", async () => {
    vi.mocked(api.getUnattendedStatus).mockResolvedValue({ enabled: true, has_password: true, twofa_enabled: false });
    vi.mocked(api.setUnattended2fa).mockResolvedValue({ uri: "otpauth://totp/OpenDesk:123456789?secret=JBSWY3DPEHPK3PXP&issuer=OpenDesk", secret: "JBSWY3DPEHPK3PXP", qr_width: 2, qr_modules: [true, false, false, true] });
    vi.mocked(api.confirmUnattended2fa).mockRejectedValueOnce("invalid 2FA code").mockResolvedValueOnce(undefined);
    const notify = vi.fn();
    render(<NotifyContext.Provider value={notify}><Home status="online" onConnect={vi.fn()} /></NotifyContext.Provider>);
    const toggle = screen.getByRole("checkbox", { name: "Require 2FA code" }) as HTMLInputElement;
    await waitFor(() => expect(toggle.disabled).toBe(false));
    fireEvent.click(toggle);

    expect(await screen.findByRole("img", { name: "QR code for your authenticator app" })).toBeTruthy();
    expect(screen.getByLabelText("Setup key").textContent).toBe("JBSW Y3DP EHPK 3PXP");
    expect(toggle.checked).toBe(false);

    fireEvent.change(screen.getByLabelText("Authenticator code"), { target: { value: "000000" } });
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith(expect.stringMatching(/didn't match/), "error"));
    expect(toggle.checked).toBe(false);

    fireEvent.change(screen.getByLabelText("Authenticator code"), { target: { value: "123456" } });
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(toggle.checked).toBe(true));
    expect(api.confirmUnattended2fa).toHaveBeenLastCalledWith("123456");
    expect(screen.queryByLabelText("Setup key")).toBeNull();
  });

  it("discards a pending 2FA setup on cancel", async () => {
    vi.mocked(api.getUnattendedStatus).mockResolvedValue({ enabled: true, has_password: true, twofa_enabled: false });
    vi.mocked(api.setUnattended2fa).mockResolvedValueOnce({ uri: "otpauth://x", secret: "ABCD", qr_width: 1, qr_modules: [true] }).mockResolvedValueOnce(null);
    render(<Home status="online" onConnect={vi.fn()} />);
    const toggle = screen.getByRole("checkbox", { name: "Require 2FA code" }) as HTMLInputElement;
    await waitFor(() => expect(toggle.disabled).toBe(false));
    fireEvent.click(toggle);
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByLabelText("Setup key")).toBeNull());
    expect(api.setUnattended2fa).toHaveBeenLastCalledWith(false);
    expect(toggle.checked).toBe(false);
  });

  it("keeps the 2FA toggle disabled until unattended access is on", async () => {
    render(<Home status="online" onConnect={vi.fn()} />);
    await screen.findByText("123456789");
    expect((screen.getByRole("checkbox", { name: "Require 2FA code" }) as HTMLInputElement).disabled).toBe(true);
  });

  it("confirms a rotated password", async () => {
    vi.mocked(api.rotatePassword).mockResolvedValue({ password: "999999" });
    const notify = vi.fn();
    render(<NotifyContext.Provider value={notify}><Home status="online" onConnect={vi.fn()} /></NotifyContext.Provider>);
    fireEvent.click(screen.getByRole("button", { name: "Rotate" }));
    await waitFor(() => expect(notify).toHaveBeenCalledWith("New password generated.", "info"));
  });
});
