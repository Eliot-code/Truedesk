// @vitest-environment jsdom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { onFileOffer, onFileProgress, onFileDone, onFileError, respondFile, sendFile } = vi.hoisted(() => ({
  onFileOffer: vi.fn(), onFileProgress: vi.fn(), onFileDone: vi.fn(), onFileError: vi.fn(),
  respondFile: vi.fn(() => Promise.resolve()), sendFile: vi.fn(() => Promise.resolve(1)),
}));

vi.mock("../lib/api", () => ({ onFileOffer, onFileProgress, onFileDone, onFileError, respondFile, sendFile }));

import FileTransfer from "./FileTransfer";

describe("FileTransfer", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("accepts an offer and updates progress", async () => {
    let offerHandler!: (offer: { id: number; name: string; size: number; from: string }) => void;
    let progressHandler!: (progress: { id: number; transferred: number; total: number; direction: string }) => void;
    onFileOffer.mockImplementation((handler) => { offerHandler = handler; return Promise.resolve(vi.fn()); });
    onFileProgress.mockImplementation((handler) => { progressHandler = handler; return Promise.resolve(vi.fn()); });
    onFileDone.mockReturnValue(Promise.resolve(vi.fn()));
    onFileError.mockReturnValue(Promise.resolve(vi.fn()));
    const view = render(<FileTransfer />);

    await waitFor(() => expect(offerHandler).toBeTypeOf("function"));
    offerHandler({ id: 42, name: "report.pdf", size: 2048, from: "peer-7" });
    expect(await view.findByRole("dialog", { name: "Incoming file report.pdf" })).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(respondFile).toHaveBeenCalledWith(42, true));

    progressHandler({ id: 42, transferred: 1024, total: 2048, direction: "receive" });
    const progress = view.getByRole("progressbar", { name: "report.pdf progress" }) as HTMLProgressElement;
    await waitFor(() => expect(progress.value).toBe(1024));
  });
});
