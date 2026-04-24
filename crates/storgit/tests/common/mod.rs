//! Shared test helpers: id shorthand, put/get shortcuts, and the
//! per-layout factories invoked by `generic_test!` in `generic.rs`.

#![allow(dead_code)]

use storgit::layout::Layout;
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::{Parts, SubmoduleLayout};
use storgit::{Id, Store};

pub fn mkid(s: &str) -> Id {
    Id::new(s).unwrap()
}

pub fn put_data<L: Layout>(store: &mut Store<L>, id_str: &str, data: &[u8]) {
    store.put(&mkid(id_str), None, Some(data)).unwrap();
}

pub fn get_data<L: Layout>(store: &Store<L>, id_str: &str) -> Option<Vec<u8>> {
    store.get(&mkid(id_str)).unwrap().and_then(|e| e.data)
}

pub fn make_submodule_store() -> Store<SubmoduleLayout> {
    Store::<SubmoduleLayout>::open(Parts::default()).unwrap()
}

pub fn make_subdir_store() -> Store<SubdirLayout> {
    Store::<SubdirLayout>::open_subdir().unwrap()
}
