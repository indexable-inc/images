// Vite's `?worker&inline` import: the module default-exports a constructor that
// spawns the worker (inlined as a blob, so the single-file build stays one HTML).
// Declared here because the site does not pull in `vite/client`'s ambient types.
declare module '*?worker&inline' {
  const workerConstructor: new () => Worker;
  export default workerConstructor;
}
