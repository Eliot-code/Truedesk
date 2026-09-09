import { describe, it, expect } from "vitest";
import { parseVideo, wrap, unwrap, Tag } from "./protocol";

describe("parseVideo", () => {
  it("parses the 18-byte header at the correct offsets", () => {
    const plain = new Uint8Array(18 + 4); // header + 4 bytes of payload
    const dv = new DataView(plain.buffer);
    dv.setBigUint64(0, 0x0102030405060708n); // seq
    dv.setUint32(8, 123456); // timestamp_us
    dv.setUint8(12, 1); // codec = H264
    dv.setUint8(13, 0b0000_0001); // flags: keyframe bit set
    dv.setUint16(14, 1920); // width
    dv.setUint16(16, 1080); // height
    plain.set([0xde, 0xad, 0xbe, 0xef], 18);

    const v = parseVideo(plain);

    expect(v.seq).toBe(0x0102030405060708n);
    expect(v.timestampUs).toBe(123456);
    expect(v.codec).toBe(1);
    expect(v.keyframe).toBe(true);
    expect(v.width).toBe(1920);
    expect(v.height).toBe(1080);
    expect(Array.from(v.data)).toEqual([0xde, 0xad, 0xbe, 0xef]);
  });

  it("clears keyframe when flags bit0 is unset", () => {
    const plain = new Uint8Array(18);
    plain[13] = 0b0000_0010; // some other flag bit, not bit0
    const v = parseVideo(plain);
    expect(v.keyframe).toBe(false);
  });
});

describe("wrap/unwrap", () => {
  it("round-trips tag + body", () => {
    const body = new Uint8Array([1, 2, 3]);
    const msg = wrap(Tag.Video, body);
    expect(msg[0]).toBe(Tag.Video);
    const { tag, body: out } = unwrap(msg);
    expect(tag).toBe(Tag.Video);
    expect(Array.from(out)).toEqual([1, 2, 3]);
  });
});
