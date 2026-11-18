// Runs the trainer wasm. Each message is one command; the reply is its JSON text.
const encoder = new TextEncoder();
const decoder = new TextDecoder();
let exports_ = null;
let panicText = '';

// The page hands over the engine's bytes first, so every worker it starts runs the
// same build. A worker started without them (or given none) fetches the file.
let ready = null;

function start(bytes) {
  ready = (async () => {
    const imports = {
      env: {
        tr_panic(ptr, len) {
          panicText = decoder.decode(new Uint8Array(exports_.memory.buffer, ptr, len));
        },
      },
    };
    let instance;
    if (bytes) {
      ({ instance } = await WebAssembly.instantiate(bytes, imports));
    } else {
      const url = new URL('trainer.wasm', self.location.href);
      try {
        ({ instance } = await WebAssembly.instantiateStreaming(fetch(url), imports));
      } catch (_) {
        const fetched = await (await fetch(url)).arrayBuffer();
        ({ instance } = await WebAssembly.instantiate(fetched, imports));
      }
    }
    exports_ = instance.exports;
    exports_.tr_init();
  })();
}

function call(text) {
  const bytes = encoder.encode(text);
  const ptr = exports_.tr_alloc(bytes.length);
  new Uint8Array(exports_.memory.buffer, ptr, bytes.length).set(bytes);
  const len = exports_.tr_call(ptr, bytes.length);
  const out = exports_.tr_result_ptr();
  return decoder.decode(new Uint8Array(exports_.memory.buffer, out, len));
}

self.onmessage = async event => {
  if (event.data && 'wasm' in event.data) {
    if (!ready) start(event.data.wasm);
    return;
  }
  if (!ready) start(null);
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
