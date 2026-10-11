import { describe, expect, it } from "vitest";
import { friendlyError } from "./notify";

describe("friendlyError", () => {
  it("maps known backend errors to plain sentences", () => {
    expect(friendlyError("target ID must be 9 digits")).toBe("Enter the 9-digit ID.");
    expect(friendlyError("unattended access password must be at least 12 characters")).toBe("Use at least 12 characters for the unattended access password.");
    expect(friendlyError(new Error("invalid 2FA code"))).toMatch(/didn't match/);
  });

  it("passes other messages through as a sentence", () => {
    expect(friendlyError("relay unreachable")).toBe("Relay unreachable.");
    expect(friendlyError("The host declined the connection request.")).toBe("The host declined the connection request.");
    expect(friendlyError("")).toBe("Something went wrong.");
  });
});
