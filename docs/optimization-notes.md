# Optimization notes

Internal engineering notes for the `translate-dict-lsp` language server. Not user-facing.

## Current architecture (as of v0.2.0)

The dictionary is 674 JSON shards (`dict/{aa,ab,...}.json`, ~66 MB total) that are parsed lazily:

- **Forward lookup** (English → Chinese): one shard per query via `prefix_of()`, cached in a
  bounded LRU (`ShardCache`, cap 32). O(1)-ish, ~0.1 ms/hover.
- **Reverse query** (Chinese → English): a full scan. Three levers keep this fast and cheap:
  1. **Raw-byte prefilter** — UTF-8 is self-synchronizing, so a byte-substring search over the
     unparsed shard is an exact contains-test. Shards that cannot contain the query are never
     parsed (rare words now ~10-60 ms).
  2. **Parallel scan** — shards are chunked across up to 16 scoped threads; each thread parses only
     matched shards.
  3. **Result memoization** — `ReverseCache` (bounded LRU, 128) stores the last reverse results, so
     re-hovering the same word is ~0.3 ms instead of a re-scan.
- **Chinese word index** (`is_chinese_word` for FMM segmentation) is a lazily built ~6 MB bloom
  filter over all 2-3 char Chinese fragments. The previous `HashSet<String>` of ~2.57M fragments
  cost ~80 MB of RSS.
- **`malloc_trim(0)`** after a reverse scan returns per-thread glibc arena memory to the OS; without
  it, 16 worker threads' transient parse allocations inflate RSS to ~200 MB.

Measured on the full 674-shard dictionary (16-core, release):

| Scenario | Before (v0.1.0) | After (v0.2.0) |
|---|---|---|
| First Chinese hover (index build + scan) | ~2.4 s | ~0.7 s |
| New common Chinese word | ~1.0-1.3 s | ~200-300 ms |
| New rare Chinese word | ~1.2 s | ~10-60 ms |
| Repeat same Chinese word | ~1.2 s | ~0.3 ms |
| Steady heap (RssAnon) after hovers | ~85 MB | ~32 MB |

Note: total RSS after a hover reads ~100 MB even with a 32 MB heap, because the release binary
embeds the ~66 MB dictionary and those read-only pages become resident (shared, not per-process).

## Future ideas (tradeoffs)

1. **Precompute and embed the bloom bit set at build time** (`build.rs` → `include_bytes!`).
   The bloom currently rebuilds from all shards on the first Chinese hover (~0.7 s). Building it
   once in `build.rs` and embedding ~6 MB of bits removes that first-hover cost entirely. Highest
   ROI, low risk.
2. **Persistent reverse index** (Chinese phrase → word ids) would get common-word hovers well under
   100 ms, but costs ~40-60 MB resident memory — conflicting with the current low-memory goal.
   Only revisit if hover latency becomes a complaint again.
3. **Faster JSON parsing** (`simd-json` ~4-8x on the parse step) would speed up cold scans that
   still parse many shards (common words). Adds a dependency and a serde-compat shim; the parallel
   scan already masks most of this cost.
4. **Raise `ShardCache` capacity** (32 → e.g. 256) increases forward-lookup hit rate at the cost of
   ~26 MB per 32 shards of retained memory. Current cap is the deliberate tradeoff.
5. **Portability**: `malloc_trim` is glibc-specific (`cfg(target_env = "gnu")`). On musl/macOS the
   no-op branch runs; memory there relies on the system allocator. Not a release blocker.
