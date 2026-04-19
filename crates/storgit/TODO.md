# storgit TODO

Known gaps and sharp edges before storgit is ready for real use. Ordered
roughly by priority. Resolved items are kept (struck through) for a few
iterations as a record of what landed and where to find it.

## Correctness / safety

- **Concurrent writers.** Nothing guards against two `Store` handles
  pointing at the same persisted BLOB. Last-writer-wins. Fine for the
  single-process valet case; document it.

## API gaps

- **Sync preserves config.** When multi-replica sync lands and
  `parent.git/config` starts holding remotes/refspecs, the parent-flush
  path (`flush_parent` -> `commit_parent_tree`) must not clobber it.
  Today the parent dir is mutated in place rather than rebuilt, so
  config is preserved by accident; lock that in with a test once sync
  is real.
- **Lazy `history`.** Returns `Vec<Entry>` with every payload eagerly
  decoded. For large histories, expose an iterator that yields entries
  on demand and lets callers stop early.
- **Typed errors.** `Error::Git(Box<dyn Error>)` and `Error::Other(String)`
  erase the shape of failures. Split into matchable variants (not found,
  corrupt tarball, underlying git error, io) once callers start
  branching on them.
- **`CommitId` ergonomics.** Exposes `pub [u8; 20]` but has no `Display`,
  `FromStr`, or hex helpers. Add them when callers start logging or
  persisting ids.

## Performance

- **Repeated `gix::open`.** Each `get` / `put` / `flush_parent` opens
  the parent and/or a submodule fresh. Cache handles on the `Store`
  once operations are stable.

## Minor

- **List ordering contract.** `Store::list` documents "arbitrary" order
  but actually returns BTreeMap key order (sorted by id). Tighten the
  doc or keep the looser contract intentionally.
- **Commit time fallback.** `current_signature` silently substitutes
  `seconds = 0` if the system clock predates the UNIX epoch. Harmless
  but worth a comment or a hard error.
