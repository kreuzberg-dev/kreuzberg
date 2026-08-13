// Stub for the unresolvable "wasi_snapshot_preview1" import module in the
// wasm-bindgen Node test glue. See `env.js` in this directory for the full
// rationale. The engine tests never exercise the WASI-linked OCR C code, so
// every syscall can report clean absence: 0 for close/yield, WASI errno 8
// (EBADF) for the preopen probes libc runs at startup, and errno 63 (ENOSYS)
// for everything else. ~keep
const EBADF = 8;
const ENOSYS = 63;

module.exports = new Proxy(
  {
    fd_close: () => 0,
    sched_yield: () => 0,
    environ_get: () => 0,
    environ_sizes_get: () => 0,
    fd_prestat_get: () => EBADF,
    fd_prestat_dir_name: () => EBADF,
  },
  {
    get(target, prop) {
      if (prop in target) return target[prop];
      return () => ENOSYS;
    },
  },
);
