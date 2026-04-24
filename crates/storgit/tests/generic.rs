//! Tests exercising the generic [`storgit::Store`] surface. Each
//! assertion lives as a generic body in `common` and runs once per
//! layout. Layout-specific behaviour (persistence envelopes, on-disk
//! details) lives in that layout's own integration-test file.

mod common;

use storgit::Id;
use storgit::id;

// -- SubmoduleLayout instantiations ------------------------------------

#[test]
fn put_then_get_returns_latest_submodule() {
    common::put_then_get_returns_latest(common::make_submodule_store());
}

#[test]
fn get_missing_entry_returns_none_submodule() {
    common::get_missing_entry_returns_none(common::make_submodule_store());
}

#[test]
fn history_returns_all_versions_newest_first_submodule() {
    common::history_returns_all_versions_newest_first(common::make_submodule_store());
}

#[test]
fn list_names_live_entries_submodule() {
    common::list_names_live_entries(common::make_submodule_store());
}

#[test]
fn archive_removes_from_list_but_history_survives_submodule() {
    common::archive_removes_from_list_but_history_survives(common::make_submodule_store());
}

#[test]
fn re_put_after_archive_continues_history_submodule() {
    common::re_put_after_archive_continues_history(common::make_submodule_store());
}

#[test]
fn delete_drops_entry_and_history_submodule() {
    common::delete_drops_entry_and_history(common::make_submodule_store());
}

#[test]
fn re_put_after_delete_starts_fresh_history_submodule() {
    common::re_put_after_delete_starts_fresh_history(common::make_submodule_store());
}

#[test]
fn put_returns_matching_commit_id_for_latest_history_entry_submodule() {
    common::put_returns_matching_commit_id_for_latest_history_entry(common::make_submodule_store());
}

#[test]
fn empty_payload_roundtrips_submodule() {
    common::empty_payload_roundtrips(common::make_submodule_store());
}

#[test]
fn new_store_is_empty_and_writable_submodule() {
    common::new_store_is_empty_and_writable(common::make_submodule_store());
}

#[test]
fn put_rejects_both_sides_none_submodule() {
    common::put_rejects_both_sides_none(common::make_submodule_store());
}

#[test]
fn put_label_and_data_roundtrips_submodule() {
    common::put_label_and_data_roundtrips(common::make_submodule_store());
}

#[test]
fn put_none_slot_carries_prior_blob_forward_submodule() {
    common::put_none_slot_carries_prior_blob_forward(common::make_submodule_store());
}

#[test]
fn put_label_only_is_noop_when_label_matches_prior_submodule() {
    common::put_label_only_is_noop_when_label_matches_prior(common::make_submodule_store());
}

#[test]
fn put_none_on_fresh_entry_omits_slot_submodule() {
    common::put_none_on_fresh_entry_omits_slot(common::make_submodule_store());
}

#[test]
fn put_is_noop_when_tree_matches_head_submodule() {
    common::put_is_noop_when_tree_matches_head(common::make_submodule_store());
}

#[test]
fn label_cache_surfaces_via_label_and_list_labels_submodule() {
    common::label_cache_surfaces_via_label_and_list_labels(common::make_submodule_store());
}

#[test]
fn empty_label_is_not_indexed_but_still_in_history_submodule() {
    common::empty_label_is_not_indexed_but_still_in_history(common::make_submodule_store());
}

#[test]
fn archive_clears_label_from_cache_submodule() {
    common::archive_clears_label_from_cache(common::make_submodule_store());
}

// -- Id validation (layout-independent) --------------------------------

#[test]
fn id_rejects_empty() {
    assert_eq!(Id::new(""), Err(id::Error::Empty));
}

#[test]
fn id_rejects_slash_and_nul() {
    assert_eq!(Id::new("a/b"), Err(id::Error::BadChar('/')));
    assert_eq!(Id::new("a\0b"), Err(id::Error::BadChar('\0')));
}

#[test]
fn id_rejects_quote_and_backslash() {
    assert_eq!(Id::new("a\"b"), Err(id::Error::BadChar('"')));
    assert_eq!(Id::new("a\\b"), Err(id::Error::BadChar('\\')));
}

#[test]
fn id_rejects_control_chars() {
    assert_eq!(Id::new("a\nb"), Err(id::Error::BadChar('\n')));
    assert_eq!(Id::new("a\tb"), Err(id::Error::BadChar('\t')));
    assert_eq!(Id::new("a\rb"), Err(id::Error::BadChar('\r')));
    assert_eq!(Id::new("a\x01b"), Err(id::Error::BadChar('\x01')));
    assert_eq!(Id::new("a\x7fb"), Err(id::Error::BadChar('\x7f')));
}

#[test]
fn id_rejects_leading_dot() {
    assert_eq!(Id::new(".foo"), Err(id::Error::LeadingDot));
    assert_eq!(Id::new("."), Err(id::Error::LeadingDot));
    assert_eq!(Id::new(".."), Err(id::Error::LeadingDot));
}

#[test]
fn id_rejects_git_suffix() {
    assert_eq!(Id::new("foo.git"), Err(id::Error::GitSuffix));
}

#[test]
fn id_rejects_reserved_names() {
    assert_eq!(Id::new("index"), Err(id::Error::Reserved));
}

#[test]
fn id_rejects_too_long() {
    let long = "a".repeat(Id::MAX_LEN + 1);
    assert!(matches!(Id::new(long), Err(id::Error::TooLong { .. })));
}

#[test]
fn id_accepts_reasonable_strings() {
    Id::new("alpha").unwrap();
    Id::new("alpha-beta").unwrap();
    Id::new("user@example.com").unwrap();
    Id::new("01945e9b-3e3f-7b2a-b8ab-8a52c82d4c01").unwrap();
}
