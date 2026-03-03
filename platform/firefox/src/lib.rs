use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn add_one(a: i32) -> i32 {
    return a + 1;
}

#[wasm_bindgen(start)]
pub fn main() {
    println!("hit");
}
