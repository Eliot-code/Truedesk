import { Tag, wrap, unwrap, parseVideo } from "./lib/protocol";
import { makeOpener, K_H2V } from "./lib/crypto";
import { makeDecoder } from "./lib/decode";

const canvas = document.querySelector<HTMLCanvasElement>("#screen")!;
const decode = makeDecoder(canvas);

async function main() {
  const open = await makeOpener(K_H2V);

  const ws = new WebSocket("ws://127.0.0.1:8080/ws");
  ws.binaryType = "arraybuffer";

  ws.onopen = () => {
    const connect = new TextEncoder().encode(JSON.stringify({ t: "connect", target: "abc", ver: 1 }));
    ws.send(wrap(Tag.Control, connect) as Uint8Array<ArrayBuffer>);
  };

  ws.onmessage = (ev) => {
    const { tag, body } = unwrap(new Uint8Array(ev.data));
    if (tag !== Tag.Video) return; // ignore control/kex/input frames for this PoC

    let plain: Uint8Array;
    try {
      plain = open(body);
    } catch (e) {
      console.error("AEAD open failed", e);
      return;
    }

    const v = parseVideo(plain);
    // decoder.decode() may error on the dummy 0xAB PoC payload — expected,
    // logged inside makeDecoder, not fatal.
    decode(v.data, v.keyframe, v.timestampUs);
  };

  ws.onerror = (e) => console.error("ws error", e);
  ws.onclose = () => console.log("ws closed");
}

main();
