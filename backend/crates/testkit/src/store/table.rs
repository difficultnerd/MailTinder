//! A generic in-memory table with versions and preconditions.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ports::store::{Keyed, Precondition, StoreError, Version, Versioned};

/// A single table: a mutex-guarded map from key to (record, version).
pub(crate) struct Table<K, R> {
    rows: Mutex<BTreeMap<K, (R, u64)>>,
    next_version: AtomicU64,
}

impl<K: Ord + Clone, R: Clone + Keyed<Key = K>> Table<K, R> {
    pub fn new() -> Self {
        Self {
            rows: Mutex::new(BTreeMap::new()),
            next_version: AtomicU64::new(0),
        }
    }

    pub fn get(&self, k: &K) -> Option<Versioned<R>> {
        let rows = self
            .rows
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        rows.get(k).map(|(r, v)| Versioned {
            record: r.clone(),
            version: Version(v.to_string()),
        })
    }

    pub fn put(&self, r: &R, pre: &Precondition) -> Result<Version, StoreError> {
        let mut rows = self
            .rows
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        let key = r.key();
        let existing = rows.get(&key).map(|(_, v)| v.to_string());
        match pre {
            Precondition::None => {}
            Precondition::MustNotExist => {
                if existing.is_some() {
                    return Err(StoreError::AlreadyExists);
                }
            }
            Precondition::MustExist => {
                if existing.is_none() {
                    return Err(StoreError::PreconditionFailed);
                }
            }
            Precondition::Matches(v) => {
                if existing.as_deref() != Some(v.0.as_str()) {
                    return Err(StoreError::PreconditionFailed);
                }
            }
        }
        let version = self.next_version.fetch_add(1, Ordering::SeqCst) + 1;
        rows.insert(key, (r.clone(), version));
        Ok(Version(version.to_string()))
    }

    pub fn delete(&self, k: &K, pre: &Precondition) -> Result<(), StoreError> {
        let mut rows = self
            .rows
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        let existing = rows.get(k).map(|(_, v)| v.to_string());
        match pre {
            Precondition::None => {}
            Precondition::MustNotExist => {
                if existing.is_some() {
                    return Err(StoreError::AlreadyExists);
                }
            }
            Precondition::MustExist => {
                if existing.is_none() {
                    return Err(StoreError::PreconditionFailed);
                }
            }
            Precondition::Matches(v) => {
                if existing.as_deref() != Some(v.0.as_str()) {
                    return Err(StoreError::PreconditionFailed);
                }
            }
        }
        rows.remove(k);
        Ok(())
    }

    /// Snapshot clone in key order.
    pub fn scan(&self) -> Vec<Versioned<R>> {
        let rows = self
            .rows
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        rows.iter()
            .map(|(_, (r, v))| Versioned {
                record: r.clone(),
                version: Version(v.to_string()),
            })
            .collect()
    }

    /// Remove rows that fail the predicate; returns the number removed.
    pub fn retain(&self, keep: impl Fn(&R) -> bool) -> u64 {
        let mut rows = self
            .rows
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        let before = rows.len();
        rows.retain(|_, (r, _)| keep(r));
        (before - rows.len()) as u64
    }
}

impl<K: Ord + Clone, R: Clone + Keyed<Key = K>> Default for Table<K, R> {
    fn default() -> Self {
        Self::new()
    }
}
