const wasmURL = browser.runtime.getURL("pkg/valet_firefox_bg.wasm");

console.log("wasm_loader.js");

WebAssembly.instantiateStreaming(fetch(wasmURL), {})
  .then((results) => {
    console.log("Wasm module loaded successfully");
    const add_one = results.instance.exports.add_one;
    console.log("Result of wasm function:", add_one(41));
  })
  .catch((err) => {
    console.error("Error loading WebAssembly module:", err);
  });
