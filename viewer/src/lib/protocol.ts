export const Tag = { Control: 0x01, Kex: 0x02, Video: 0x10, Input: 0x12 } as const;

export function wrap(tag: number, body: Uint8Array): Uint8Array {
  const out = new Uint8Array(1 + body.length);
  out[0] = tag;
  out.set(body, 1);
  return out;
}

export function unwrap(msg: Uint8Array): { tag: number; body: Uint8Array } {
  return { tag: msg[0], body: msg.subarray(1) };
}

export interface VideoHeader {
  seq: bigint;
  timestampUs: number;
  codec: number;
  keyframe: boolean;
  width: number;
  height: number;
  data: Uint8Array;
}

/**
 * Parses the 18-byte big-endian video header produced by the host-agent:
 *   u64 frame_seq   @0
 *   u32 timestamp_us@8
 *   u8  codec       @12 (1 = H264)
 *   u8  flags       @13 (bit0 = keyframe)
 *   u16 width       @14
 *   u16 height      @16
 *   data            @18..
 */
export function parseVideo(plain: Uint8Array): VideoHeader {
  const dv = new DataView(plain.buffer, plain.byteOffset, plain.byteLength);
  return {
    seq: dv.getBigUint64(0),
    timestampUs: dv.getUint32(8),
    codec: dv.getUint8(12),
    keyframe: (dv.getUint8(13) & 1) === 1,
    width: dv.getUint16(14),
    height: dv.getUint16(16),
    data: plain.subarray(18),
  };
}
