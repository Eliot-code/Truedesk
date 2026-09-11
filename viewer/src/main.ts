import { Tag, wrap, parseVideo } from "./lib/protocol";
import { runKex, openFrame, attachRouter } from "./lib/kex";
import { makeDecoder } from "./lib/decode";
import { attachInput } from "./lib/input";
import { invoke } from "@tauri-apps/api/core";

const canvas = document.querySelector<HTMLCanvasElement>("#screen")!;
const decode = makeDecoder(canvas);

async function main() {
  const password = "test"; // TODO: prompt the user; matches host arg 3
  const ws = new WebSocket("ws://127.0.0.1:8080/ws");
  ws.binaryType = "arraybuffer";

  // Install the persistent frame router BEFORE any await, so no frame that
  // arrives during the paired-wait or the KEX IPC round-trips is dropped.
  const router = attachRouter(ws);

  ws.onopen = () => {
    const connect = new TextEncoder().encode(
      JSON.stringify({ t: "connect", target: "abc", ver: 1 }),
    );
    ws.send(wrap(Tag.Control, connect) as Uint8Array<ArrayBuffer>);
  };

  ws.onerror = (e) => console.error("ws error", e);
  ws.onclose = () => console.log("ws closed");

  // wait for `paired`, then run KEX, then stream video
  await router.waitPaired();
  await runKex(ws, router, password); // throws on wrong password → nothing decoded

  // Only now decode video. Frames buffered during KEX are flushed here, in order.
  router.onVideo(async (body) => {
    try {
      const plain = await openFrame(body);
      const v = parseVideo(plain);
      // decoder.decode() may error on the dummy 0xAB PoC payload — expected,
      // logged inside makeDecoder, not fatal.
      decode(v.data, v.keyframe, v.timestampUs);
    } catch (e) {
      console.error("open/decode failed", e);
    }
  });

  attachInput(ws, canvas, (bytes) =>
    invoke<number[]>("seal_input", { bytes: Array.from(bytes) }).then(
      (sealed) => new Uint8Array(sealed),
    ),
  );
}

main();
