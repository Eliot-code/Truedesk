import { invoke } from "@tauri-apps/api/core";
import { Tag, wrap, unwrap } from "./protocol";

const KEX_MSG = 0x01;
const KEX_CONFIRM = 0x02;

/**
 * Routes and buffers inbound WebSocket frames so nothing is dropped between
 * awaits. A single persistent `message` listener feeds `ingest()` from the
 * moment the socket is created; consumers (`waitPaired`, `nextKex`, `onVideo`)
 * attach later and still receive frames that arrived before they were waiting.
 *
 * Frames are parsed by tag (and, for Tag.Kex, by their leading sub-tag byte):
 *   - Control `paired` resolves the paired-wait.
 *   - Tag.Kex frames are queued per sub-tag (or handed to a pending waiter),
 *     with the sub-tag byte stripped before delivery.
 *   - Tag.Video frames go to the video handler once attached, and are buffered
 *     until then so early frames survive the KEX handshake.
 */
export class FrameRouter {
  private kexQueues = new Map<number, Uint8Array[]>();
  private kexWaiters = new Map<number, (payload: Uint8Array) => void>();
  private pairedResolve: (() => void) | null = null;
  private pairedResolved = false;
  private videoBuffer: Uint8Array[] = [];
  private videoHandler: ((body: Uint8Array) => void) | null = null;

  /** Feed one raw WebSocket frame (full message, including the leading tag byte). */
  ingest(data: Uint8Array): void {
    const { tag, body } = unwrap(data);

    if (tag === Tag.Control) {
      try {
        const msg = JSON.parse(new TextDecoder().decode(body));
        if (msg.t === "paired") this.resolvePaired();
      } catch {
        // non-JSON / unrelated control frame — ignore
      }
      return;
    }

    if (tag === Tag.Kex) {
      const sub = body[0];
      const payload = body.subarray(1); // strip sub-tag byte for the backend
      const waiter = this.kexWaiters.get(sub);
      if (waiter) {
        this.kexWaiters.delete(sub);
        waiter(payload);
      } else {
        let q = this.kexQueues.get(sub);
        if (!q) {
          q = [];
          this.kexQueues.set(sub, q);
        }
        q.push(payload);
      }
      return;
    }

    if (tag === Tag.Video) {
      if (this.videoHandler) this.videoHandler(body);
      else this.videoBuffer.push(body);
      return;
    }
    // other tags (e.g. Input) are not consumed by the viewer
  }

  private resolvePaired(): void {
    this.pairedResolved = true;
    const r = this.pairedResolve;
    this.pairedResolve = null;
    if (r) r();
  }

  /** Resolves once a Control `paired` frame has been (or is) seen. */
  waitPaired(): Promise<void> {
    if (this.pairedResolved) return Promise.resolve();
    return new Promise((resolve) => {
      this.pairedResolve = resolve;
    });
  }

  /**
   * Yields the next Tag.Kex payload for `sub` (sub-tag stripped). Drains an
   * already-queued frame first; only waits if the queue is empty. This closes
   * the drop window — a frame delivered during a prior await is never lost.
   */
  nextKex(sub: number): Promise<Uint8Array> {
    const q = this.kexQueues.get(sub);
    if (q && q.length > 0) return Promise.resolve(q.shift()!);
    return new Promise((resolve) => {
      this.kexWaiters.set(sub, resolve);
    });
  }

  /** Attach the video handler and flush any frames buffered before now. */
  onVideo(handler: (body: Uint8Array) => void): void {
    this.videoHandler = handler;
    const buffered = this.videoBuffer;
    this.videoBuffer = [];
    for (const b of buffered) handler(b);
  }
}

/** Creates a FrameRouter and wires it to the socket's `message` events immediately. */
export function attachRouter(ws: WebSocket): FrameRouter {
  const router = new FrameRouter();
  ws.addEventListener("message", (ev: MessageEvent) => {
    router.ingest(new Uint8Array(ev.data));
  });
  return router;
}

/**
 * Drives the SPAKE2 password-authenticated handshake over the WebSocket.
 * All crypto lives in the Tauri backend; this only routes the wire messages.
 * Throws if the password is wrong (kex_verify / kex_finish reject).
 */
export async function runKex(
  ws: WebSocket,
  router: FrameRouter,
  password: string,
): Promise<void> {
  // 1. viewer starts, sends its spake2 msg (kex_start returns [0x01, ...msg] body)
  const startBody: number[] = await invoke("kex_start", { password });
  ws.send(wrap(Tag.Kex, new Uint8Array(startBody)) as Uint8Array<ArrayBuffer>);

  // 2. receive host's spake2 msg (sub-tag stripped), finish, send our confirm body
  const hostMsg = await router.nextKex(KEX_MSG);
  const confirmBody: number[] = await invoke("kex_finish", {
    peerMsg: Array.from(hostMsg),
  });
  ws.send(wrap(Tag.Kex, new Uint8Array(confirmBody)) as Uint8Array<ArrayBuffer>);

  // 3. receive host's confirm (sub-tag stripped), verify (throws on wrong password)
  const hostConfirm = await router.nextKex(KEX_CONFIRM);
  await invoke("kex_verify", { peerConfirm: Array.from(hostConfirm) });
}

/**
 * Opens (decrypts + authenticates) a sealed Tag.Video body via the backend.
 * Returns the plaintext (18-byte header + data). Throws on AEAD failure.
 */
export async function openFrame(sealed: Uint8Array): Promise<Uint8Array> {
  const plain: number[] = await invoke("open_frame", { sealed: Array.from(sealed) });
  return new Uint8Array(plain);
}
