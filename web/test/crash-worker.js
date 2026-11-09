// Test only: the real worker, plus a way to raise a genuine uncaught error in it.
importScripts('worker.js');
self.addEventListener('message', event => {
  if (event.data === 'crash') setTimeout(() => { throw new Error('simulated worker crash'); }, 0);
});
