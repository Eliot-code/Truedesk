// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeDecoder, sizeCanvas } from "./decode";

describe("sizeCanvas", () => {
  it("leaves the canvas alone when the size is unchanged", () => {
    const canvas = document.createElement("canvas");
    canvas.width = 640;
    canvas.height = 480;
    const setWidth = vi.spyOn(canvas, "width", "set");
    const setHeight = vi.spyOn(canvas, "height", "set");

    sizeCanvas(canvas, 640, 480);

    expect(setWidth).not.toHaveBeenCalled();
    expect(setHeight).not.toHaveBeenCalled();
  });

  it("resizes when the remote screen size changes", () => {
    const canvas = document.createElement("canvas");
    sizeCanvas(canvas, 1920, 1080);
    expect(canvas.width).toBe(1920);
    expect(canvas.height).toBe(1080);
  });
});

class FakeDecoder {
  static instances: FakeDecoder[] = [];
  static rejectHardware = false;
  state: "unconfigured" | "configured" | "closed" = "unconfigured";
  decodeQueueSize = 0;
  configs: VideoDecoderConfig[] = [];
  chunks: { type: string; timestamp: number; data: Uint8Array }[] = [];
  resets = 0;
  constructor(public init: VideoDecoderInit) {
    FakeDecoder.instances.push(this);
  }
  configure(config: VideoDecoderConfig) {
    if (FakeDecoder.rejectHardware && config.hardwareAcceleration) throw new TypeError("unsupported");
    this.configs.push(config);
    this.state = "configured";
  }
  decode(chunk: { type: string; timestamp: number; data: Uint8Array }) {
    if (this.state !== "configured") throw new Error("not configured");
    this.chunks.push(chunk);
  }
  reset() { this.resets++; this.state = "unconfigured"; }
  close() { this.state = "closed"; }
  /** Simulate WebCodecs failing: the decoder closes, then reports the error. */
  fail() { this.state = "closed"; this.init.error(new DOMException("bad frame", "EncodingError")); }
}

class FakeChunk {
  type: string; timestamp: number; data: Uint8Array;
  constructor(init: { type: string; timestamp: number; data: Uint8Array }) {
    this.type = init.type;
    this.timestamp = init.timestamp;
    this.data = init.data;
  }
}

const SPS_KEYFRAME = [0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x28, 0, 0, 0, 1, 0x65, 0x88];

function frame(seq: number, keyframe: boolean, payload: number[] = keyframe ? SPS_KEYFRAME : [0, 0, 0, 1, 0x41, 0x9a]) {
  const bytes = new Uint8Array(18 + payload.length);
  const view = new DataView(bytes.buffer);
  view.setUint32(4, seq);
  view.setUint32(8, seq * 1000);
  view.setUint8(13, keyframe ? 1 : 0);
  view.setUint16(14, 1920);
  view.setUint16(16, 1080);
  bytes.set(payload, 18);
  return bytes.buffer;
}

describe("makeDecoder", () => {
  let clock = 0;
  let requestKeyframe: ReturnType<typeof vi.fn<() => void>>;
  let drawImage: ReturnType<typeof vi.fn>;
  let canvas: HTMLCanvasElement;
  let getContext: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    FakeDecoder.instances = [];
    FakeDecoder.rejectHardware = false;
    vi.stubGlobal("VideoDecoder", FakeDecoder);
    vi.stubGlobal("EncodedVideoChunk", FakeChunk);
    vi.spyOn(console, "error").mockImplementation(() => {});
    clock = 0;
    requestKeyframe = vi.fn<() => void>();
    drawImage = vi.fn();
    canvas = document.createElement("canvas");
    getContext = vi.fn(() => ({ drawImage }));
    canvas.getContext = getContext as unknown as HTMLCanvasElement["getContext"];
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  const make = () => makeDecoder(canvas, { requestKeyframe, now: () => clock });
  const current = () => FakeDecoder.instances[FakeDecoder.instances.length - 1];

  it("configures for low latency with hardware decoding and a 4K-capable level", () => {
    make();
    expect(current().configs[0]).toEqual({ codec: "avc1.42E034", optimizeForLatency: true, hardwareAcceleration: "prefer-hardware" });
    expect(getContext).toHaveBeenCalledWith("2d", { alpha: false, desynchronized: true });
  });

  it("falls back to a plain config when the hardware preference is rejected", () => {
    FakeDecoder.rejectHardware = true;
    make();
    expect(current().configs[0]).toEqual({ codec: "avc1.42E034", optimizeForLatency: true });
  });

  it("decodes keyframes and following deltas, sizing the canvas from the header", () => {
    const decoder = make();
    decoder.push(frame(1, true));
    decoder.push(frame(2, false));
    expect(current().chunks.map((chunk) => [chunk.type, chunk.timestamp])).toEqual([["key", 1000], ["delta", 2000]]);
    expect(Array.from(current().chunks[1].data)).toEqual([0, 0, 0, 1, 0x41, 0x9a]);
    expect(canvas.width).toBe(1920);
    expect(canvas.height).toBe(1080);
    expect(requestKeyframe).not.toHaveBeenCalled();
  });

  it("reconfigures with the stream's own codec string from the SPS", () => {
    make().push(frame(1, true));
    expect(current().configs[current().configs.length - 1]).toMatchObject({ codec: "avc1.42C028" });
  });

  it("draws decoded frames and releases them", () => {
    make();
    const videoFrame = { displayWidth: 800, displayHeight: 600, close: vi.fn() };
    current().init.output(videoFrame as unknown as VideoFrame);
    expect(drawImage).toHaveBeenCalledWith(videoFrame, 0, 0);
    expect(videoFrame.close).toHaveBeenCalled();
    expect(canvas.width).toBe(800);
  });

  it("asks for a keyframe, rate limited, while waiting for one", () => {
    const decoder = make();
    decoder.push(frame(1, false));
    clock = 100;
    decoder.push(frame(2, false));
    expect(current().chunks).toHaveLength(0);
    expect(requestKeyframe).toHaveBeenCalledTimes(1);
    clock = 600;
    decoder.push(frame(3, false));
    expect(requestKeyframe).toHaveBeenCalledTimes(2);
  });

  it("skips deltas after a sequence gap and asks for a keyframe", () => {
    const decoder = make();
    decoder.push(frame(1, true));
    decoder.push(frame(3, false));
    decoder.push(frame(4, false));
    expect(current().chunks).toHaveLength(1);
    expect(requestKeyframe).toHaveBeenCalledTimes(1);
    decoder.push(frame(5, true));
    decoder.push(frame(6, false));
    expect(current().chunks.map((chunk) => chunk.type)).toEqual(["key", "key", "delta"]);
  });

  it("drops deltas when the decode queue backs up, then flushes it on the next keyframe", () => {
    const decoder = make();
    decoder.push(frame(1, true));
    current().decodeQueueSize = 3;
    decoder.push(frame(2, false));
    expect(current().chunks).toHaveLength(1);
    expect(requestKeyframe).toHaveBeenCalledTimes(1);
    decoder.push(frame(3, true));
    expect(current().resets).toBe(1);
    expect(current().chunks.map((chunk) => chunk.type)).toEqual(["key", "key"]);
  });

  it("recreates the decoder after an error and resumes from the next keyframe", () => {
    const decoder = make();
    decoder.push(frame(1, true));
    const first = current();
    first.init.output({ displayWidth: 1920, displayHeight: 1080, close: vi.fn() } as unknown as VideoFrame);
    first.fail();

    expect(FakeDecoder.instances).toHaveLength(2);
    expect(requestKeyframe).toHaveBeenCalledTimes(1);
    // It produced frames, so the hardware preference stays.
    expect(current().configs[0]).toMatchObject({ hardwareAcceleration: "prefer-hardware", codec: "avc1.42C028" });

    decoder.push(frame(2, false));
    expect(current().chunks).toHaveLength(0);
    decoder.push(frame(3, true));
    decoder.push(frame(4, false));
    expect(current().chunks.map((chunk) => chunk.type)).toEqual(["key", "delta"]);
  });

  it("drops the hardware preference when a decoder fails before any output", () => {
    make().push(frame(1, true));
    current().fail();
    expect(current().configs[0]).toEqual({ codec: "avc1.42C028", optimizeForLatency: true });
  });

  it("ignores errors and output from a replaced decoder", () => {
    make().push(frame(1, true));
    const first = current();
    first.fail();
    first.init.error(new DOMException("late", "EncodingError"));
    const late = { displayWidth: 1, displayHeight: 1, close: vi.fn() };
    first.init.output(late as unknown as VideoFrame);
    expect(FakeDecoder.instances).toHaveLength(2);
    expect(drawImage).not.toHaveBeenCalled();
    expect(late.close).toHaveBeenCalled();
  });

  it("recovers when decode() throws synchronously", () => {
    const decoder = make();
    decoder.push(frame(1, true));
    current().decode = () => { throw new Error("boom"); };
    decoder.push(frame(2, false));
    expect(FakeDecoder.instances).toHaveLength(2);
    expect(FakeDecoder.instances[0].state).toBe("closed");
    expect(requestKeyframe).toHaveBeenCalledTimes(1);
  });

  it("closes the decoder and ignores later frames and errors", () => {
    const decoder = make();
    decoder.close();
    expect(current().state).toBe("closed");
    decoder.push(frame(1, true));
    current().init.error(new DOMException("after close", "EncodingError"));
    expect(current().chunks).toHaveLength(0);
    expect(FakeDecoder.instances).toHaveLength(1);
  });

  it("ignores messages shorter than the header", () => {
    make().push(new Uint8Array(5));
    expect(current().chunks).toHaveLength(0);
    expect(requestKeyframe).not.toHaveBeenCalled();
  });
});
