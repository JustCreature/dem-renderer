// Trunk initializer hook. Trunk loads + instantiates the wasm-bindgen module (running the
// #[wasm_bindgen(start)] `start` fn), then calls `onSuccess` with the module's JS exports.
// That is the right moment to spin up the SharedArrayBuffer-backed rayon thread pool: the
// wasm memory is shared, so the workers re-instantiate the module against it.
export default function initializer() {
  return {
    onStart: () => {},
    onProgress: () => {},
    onComplete: () => {},
    onSuccess: (wasm) => {
      const n = Math.max(1, (navigator.hardwareConcurrency || 4) - 1);
      // Exported via `pub use wasm_bindgen_rayon::init_thread_pool` (JS name initThreadPool).
      wasm.initThreadPool(n).then(
        () => console.log(`rayon thread pool ready (${n} workers)`),
        (e) => console.error("initThreadPool failed", e),
      );
    },
    onFailure: (err) => console.error("wasm init failed", err),
  };
}
