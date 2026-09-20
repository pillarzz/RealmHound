//! `Memo<K, V>` -- a small recompute-on-change cache.
//!
//! Caches a derived value and rebuilds it only when a caller-supplied *key*
//! (a cheap fingerprint of the inputs) changes. Used by immediate-mode panels
//! to avoid redoing expensive work (storage scans, aggregation, sorting) on
//! every frame when nothing changed.
//!
//! Keep keys small and cheap to compare. For inputs that are expensive to
//! compare (large `Vec`s, strings), key on a monotonic revision counter bumped
//! at each mutation site instead of the data itself.
//!
//! When the build closure must borrow the same struct that owns the `Memo`,
//! use [`Memo::update`] (returns an owned `bool`, so the memo's `&mut` borrow
//! ends before you read the value) then [`Memo::get`], relying on edition-2021
//! disjoint closure captures.

/// A cache that rebuilds its value only when its input key changes.
pub struct Memo<K, V> {
    key: Option<K>,
    value: Option<V>,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> Memo<K, V> {
    /// Create an empty memo. Nothing is cached until the first build.
    pub const fn new() -> Self {
        Self {
            key: None,
            value: None,
        }
    }

    /// The cached value, if one has been built.
    pub fn get(&self) -> Option<&V> {
        self.value.as_ref()
    }
}

impl<K: PartialEq, V> Memo<K, V> {
    /// Rebuild the value via `build` if `key` changed since the last build.
    /// Returns whether a rebuild happened. Pair with [`get`](Self::get); the
    /// split lets `build` borrow other fields of the memo's owner.
    pub fn update(&mut self, key: K, build: impl FnOnce() -> V) -> bool {
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        self.value = Some(build());
        self.key = Some(key);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn builds_on_first_use() {
        let mut memo: Memo<u64, u64> = Memo::new();
        assert_eq!(memo.get(), None);

        let rebuilt = memo.update(1, || 100);
        assert!(rebuilt);
        assert_eq!(memo.get(), Some(&100));
    }

    #[test]
    fn reuses_while_key_unchanged() {
        let builds = Cell::new(0);
        let mut memo: Memo<u64, u64> = Memo::new();

        for _ in 0..5 {
            memo.update(7, || {
                builds.set(builds.get() + 1);
                7 * 10
            });
        }
        assert_eq!(
            builds.get(),
            1,
            "should build exactly once for a stable key"
        );
        assert_eq!(memo.get(), Some(&70));
    }

    #[test]
    fn rebuilds_when_key_changes() {
        let builds = Cell::new(0);
        let mut memo: Memo<u64, u64> = Memo::new();

        let build = |k: u64| {
            builds.set(builds.get() + 1);
            k * 2
        };

        memo.update(1, || build(1));
        memo.update(1, || build(1));
        memo.update(2, || build(2));
        memo.update(2, || build(2));

        assert_eq!(builds.get(), 2, "one build per distinct key");
        assert_eq!(memo.get(), Some(&4));
    }
}
