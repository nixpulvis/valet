(async () => {
  try {
    const src = browser.runtime.getURL("pkg/valet_firefox.js");
    console.debug("[valet] content script loading, module URL:", src);

    const mod = await import(src);
    console.debug("[valet] ES module loaded");

    const wasmUrl = browser.runtime.getURL("pkg/valet_firefox_bg.wasm");
    console.debug("[valet] WASM URL:", wasmUrl);

    await mod.default({ module_or_path: wasmUrl });
    console.debug("[valet] WASM initialized");

    mod.start_content();
  } catch (e) {
    console.warn("[valet] content script init failed:", e);
  }
})();
