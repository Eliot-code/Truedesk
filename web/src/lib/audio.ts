/**
 * Plays the host's Opus audio frames through WebCodecs and Web Audio. Each
 * frame is an independently decodable Opus packet (48 kHz, 20 ms); the decoder
 * turns it into PCM that is scheduled on an AudioContext so playback stays
 * gapless. If WebCodecs audio is unavailable the factory returns null and the
 * caller simply plays no sound (no crash).
 */

export interface AudioPlayer {
  /** Feed one Opus packet and its channel count. */
  push(payload: Uint8Array, channels: number): void;
  close(): void;
}

const FRAME_US = 20_000;

export function makeAudioPlayer(): AudioPlayer | null {
  const AudioCtx = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (typeof AudioDecoder === "undefined" || typeof EncodedAudioChunk === "undefined" || !AudioCtx) {
    return null;
  }

  const ctx = new AudioCtx({ sampleRate: 48_000 });
  let closed = false;
  let configured = false;
  let timestamp = 0;
  // Next time (seconds, in the context clock) a buffer may start.
  let playHead = 0;

  const decoder = new AudioDecoder({
    output: (data) => {
      if (closed) { data.close(); return; }
      try {
        const channels = data.numberOfChannels;
        const frames = data.numberOfFrames;
        const buffer = ctx.createBuffer(channels, frames, data.sampleRate);
        const plane = new Float32Array(frames);
        for (let channel = 0; channel < channels; channel += 1) {
          data.copyTo(plane, { planeIndex: channel, format: "f32-planar" });
          buffer.copyToChannel(plane, channel);
        }
        const source = ctx.createBufferSource();
        source.buffer = buffer;
        source.connect(ctx.destination);
        const now = ctx.currentTime;
        // A small lead absorbs jitter; if we fell behind, resync to now.
        if (playHead < now) playHead = now + 0.03;
        source.start(playHead);
        playHead += buffer.duration;
      } catch (error) {
        console.error("audio render error:", error);
      } finally {
        data.close();
      }
    },
    error: (error) => console.error("audio decode error:", error),
  });

  return {
    push: (payload, channels) => {
      if (closed) return;
      if (!configured) {
        try {
          decoder.configure({ codec: "opus", sampleRate: 48_000, numberOfChannels: channels || 2 });
          configured = true;
        } catch (error) {
          console.error("audio decoder configure failed:", error);
          return;
        }
      }
      void ctx.resume().catch(() => {});
      try {
        decoder.decode(new EncodedAudioChunk({ type: "key", timestamp, data: payload }));
        timestamp += FRAME_US;
      } catch (error) {
        console.error("audio decode failed:", error);
      }
    },
    close: () => {
      if (closed) return;
      closed = true;
      if (decoder.state !== "closed") {
        try { decoder.close(); } catch { /* already closing */ }
      }
      void ctx.close().catch(() => {});
    },
  };
}
