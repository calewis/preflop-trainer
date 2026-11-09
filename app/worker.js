// Runs the trainer wasm. Each message is one command; the reply is its JSON text.
const encoder = new TextEncoder();
const decoder = new TextDecoder();
let exports_ = null;
let panicText = '';

const ready = (async () => {
  const imports = {
    env: {
      tr_panic(ptr, len) {
        panicText = decoder.decode(new Uint8Array(exports_.memory.buffer, ptr, len));
      },
    },
  };
  const url = new URL('trainer.wasm', self.location.href);
  let instance;
  try {
    ({ instance } = await WebAssembly.instantiateStreaming(fetch(url), imports));
  } catch (_) {
    const bytes = await (await fetch(url)).arrayBuffer();
    ({ instance } = await WebAssembly.instantiate(bytes, imports));
  }
  exports_ = instance.exports;
  exports_.tr_init();
})();

function call(text) {
  const bytes = encoder.encode(text);
  const ptr = exports_.tr_alloc(bytes.length);
  new Uint8Array(exports_.memory.buffer, ptr, bytes.length).set(bytes);
  const len = exports_.tr_call(ptr, bytes.length);
  const out = exports_.tr_result_ptr();
  return decoder.decode(new Uint8Array(exports_.memory.buffer, out, len));
}

self.onmessage = async event => {
  const { id, args } = event.data;
  try {
    await ready;
    const started = performance.now();
    const text = call(args.join('\x1f'));
    self.postMessage({ id, text, ms: performance.now() - started });
  } catch (err) {
    self.postMessage({ id, fatal: panicText || String(err) });
  }
};
