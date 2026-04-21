import init, { start_background } from "./pkg/valet_browser.js";

await init({
  module_or_path: browser.runtime.getURL("pkg/valet_browser_bg.wasm"),
});
start_background();
