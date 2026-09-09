/**
 * Wires decrypted H.264 access units into WebCodecs VideoDecoder, rendering
 * decoded frames to `canvas`.
 *
 * PoC note: the host currently sends a dummy 0xAB payload (not valid H.264).
 * AEAD open() succeeding proves the crypto+transport path; VideoDecoder.decode()
 * is EXPECTED to error on this garbage payload until real H.264 capture/encode
 * lands (a later plan). We surface the error via console.error and the
 * decoder's error callback but do not let it crash the app.
 */
export function makeDecoder(canvas: HTMLCanvasElement) {
  const ctx = canvas.getContext("2d")!;
  const decoder = new VideoDecoder({
    output: (frame) => {
      canvas.width = frame.displayWidth;
      canvas.height = frame.displayHeight;
      ctx.drawImage(frame, 0, 0);
      frame.close();
    },
    error: (e) => {
      // Expected at PoC: dummy payload isn't valid H.264. Log, don't crash.
      console.error("decode error (expected with dummy PoC payload):", e);
    },
  });
  decoder.configure({ codec: "avc1.42E01E" }); // H.264 baseline

  return (data: Uint8Array, keyframe: boolean, timestampUs: number) => {
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
      console.error("decode() threw (expected with dummy PoC payload):", e);
    }
  };
}
