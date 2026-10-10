// Stands in for vitest in the browser: testApi.tsx only calls `vi.fn(implementation)`, and the
// harness never asserts on the calls, so the implementation itself will do.
export const vi = { fn: <F extends (...args: never[]) => unknown>(implementation?: F) => implementation ?? (() => undefined) };
