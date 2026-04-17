import init, { start_popup } from "./pkg/valet_firefox.js";

await init({
  module_or_path: browser.runtime.getURL("pkg/valet_firefox_bg.wasm"),
});
start_popup();
