// Node 25+ ships its own Web Storage globals, and without --localstorage-file
// its `localStorage` is undefined. Vitest's jsdom environment doesn't replace
// a global Node already defines, so the specs saw that one instead of jsdom's
// (CI runs Node `lts/*`, 26 since October 2026). Put jsdom's storage back.
const dom = (globalThis as { jsdom?: { window: Window } }).jsdom;
if (dom) {
    for (const name of ['localStorage', 'sessionStorage'] as const) {
        Object.defineProperty(globalThis, name, {
            configurable: true,
            get: () => dom.window[name],
        });
    }
}
