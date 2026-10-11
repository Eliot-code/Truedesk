/**
 * Wires decrypted H.264 access units into a WebCodecs VideoDecoder, rendering
 * decoded frames to `canvas`. The host encodes real H.264 (openh264) and the
 * frames arrive already opened (AEAD) by the Rust backend.
 *
 * Returns a `decode` function for feeding access units and a `close` function
 * the caller MUST invoke on teardown so the VideoDecoder is released rather
 * than leaked across session unmount/remount.
 */
export interface Decoder {
  decode: (data: Uint8Array, keyframe: boolean, timestampUs: number) => void;
  close: () => void;
}

/** Assigning a canvas size clears it, so only touch it when the size changes. */
export function sizeCanvas(canvas: HTMLCanvasElement, width: number, height: number) {
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;
}

export function makeDecoder(canvas: HTMLCanvasElement): Decoder {
  const ctx = canvas.getContext("2d")!;
  const decoder = new VideoDecoder({
    output: (frame) => {
      sizeCanvas(canvas, frame.displayWidth, frame.displayHeight);
      ctx.drawImage(frame, 0, 0);
      frame.close();
    },
    error: (e) => {
      // Log without crashing; a malformed/unsupported frame must not kill the app.
      console.error("video decode error:", e);
    },
  });
  decoder.configure({ codec: "avc1.42E01E" }); // H.264 baseline

  return {
    decode: (data, keyframe, timestampUs) => {
      if (decoder.state === "closed") return;
      try {
        decoder.decode(
          new EncodedVideoChunk({
            type: keyframe ? "key" : "delta",
            timestamp: timestampUs,
            data,
          }),
        );
      } catch (e) {
        console.error("video decode() threw:", e);
      }
    },
    close: () => {
      if (decoder.state !== "closed") {
        try {
          decoder.close();
        } catch {
          // already closing/closed — ignore
        }
      }
    },
  };
}
