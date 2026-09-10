import { describe, it, expect } from "vitest";
import { FrameRouter } from "./kex";
import { Tag, wrap } from "./protocol";

const KEX_MSG = 0x01;
const KEX_CONFIRM = 0x02;

function kexFrame(sub: number, payload: number[]): Uint8Array {
  return wrap(Tag.Kex, new Uint8Array([sub, ...payload]));
}
function control(obj: unknown): Uint8Array {
  return wrap(Tag.Control, new TextEncoder().encode(JSON.stringify(obj)));
}

describe("FrameRouter", () => {
  it("delivers a KEX frame that arrived BEFORE the consumer waits (no drop)", async () => {
    const r = new FrameRouter();
    // Frame arrives during an IPC round-trip, before nextKex is called.
    r.ingest(kexFrame(KEX_MSG, [0xaa, 0xbb]));

    const payload = await r.nextKex(KEX_MSG); // consumer attaches late
    expect(Array.from(payload)).toEqual([0xaa, 0xbb]); // sub-tag stripped
  });

  it("delivers a KEX frame that arrives AFTER the consumer waits", async () => {
    const r = new FrameRouter();
    const pending = r.nextKex(KEX_CONFIRM);
    r.ingest(kexFrame(KEX_CONFIRM, [0x01, 0x02, 0x03]));
    expect(Array.from(await pending)).toEqual([0x01, 0x02, 0x03]);
  });

  it("keeps sub-tags in separate queues and drains FIFO", async () => {
    const r = new FrameRouter();
    r.ingest(kexFrame(KEX_MSG, [0x10]));
    r.ingest(kexFrame(KEX_CONFIRM, [0x20]));
    r.ingest(kexFrame(KEX_MSG, [0x11]));

    expect(Array.from(await r.nextKex(KEX_MSG))).toEqual([0x10]);
    expect(Array.from(await r.nextKex(KEX_CONFIRM))).toEqual([0x20]);
    expect(Array.from(await r.nextKex(KEX_MSG))).toEqual([0x11]);
  });

  it("resolves waitPaired whether the frame is early or late", async () => {
    const early = new FrameRouter();
    early.ingest(control({ t: "paired" }));
    await expect(early.waitPaired()).resolves.toBeUndefined();

    const late = new FrameRouter();
    const p = late.waitPaired();
    late.ingest(control({ t: "paired" }));
    await expect(p).resolves.toBeUndefined();
  });

  it("buffers Video frames that arrive before onVideo, then flushes them in order", () => {
    const r = new FrameRouter();
    r.ingest(wrap(Tag.Video, new Uint8Array([1])));
    r.ingest(wrap(Tag.Video, new Uint8Array([2])));

    const seen: number[] = [];
    r.onVideo((body) => seen.push(body[0]));
    expect(seen).toEqual([1, 2]); // early frames flushed

    r.ingest(wrap(Tag.Video, new Uint8Array([3])));
    expect(seen).toEqual([1, 2, 3]); // later frames delivered live
  });

  it("does not treat Video as KEX or vice versa", async () => {
    const r = new FrameRouter();
    r.ingest(wrap(Tag.Video, new Uint8Array([9])));
    r.ingest(kexFrame(KEX_MSG, [0x42]));

    expect(Array.from(await r.nextKex(KEX_MSG))).toEqual([0x42]);
    const seen: number[] = [];
    r.onVideo((b) => seen.push(b[0]));
    expect(seen).toEqual([9]);
  });
});
