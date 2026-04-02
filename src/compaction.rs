use crate::sstable::SSTable;
use std::io;
pub trait CompactionStrategy: Send + Sync {
    /// Called after every memtable flush.
    fn add_sstable(&mut self, sst: SSTable);

    /// Called by the background worker. Runs at most one compaction step.
    /// Returns true if a compaction step ran.
    fn compact_if_needed(&mut self, db_path: &str, db_name: &str) -> io::Result<bool>;

    /// Full compaction of all on-disk files. Called by the COMPACT command
    /// (after LsmEngine has already flushed the live memtable).
    fn compact_all(&mut self, db_path: &str, db_name: &str) -> io::Result<()>;

    /// Files to check for a key lookup, in priority order (newest first).
    fn iter_for_key<'a>(&'a self, key: &str) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// Files whose key range overlaps [start, end], for range scans.
    fn iter_files_for_range<'a>(
        &'a self,
        start: &str,
        end: &str,
    ) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// All SSTables, for list_keys.
    fn iter_all<'a>(&'a self) -> Box<dyn Iterator<Item = &'a SSTable> + 'a>;

    /// Total SSTable file count (LsmEngine delegates segment_count() here).
    fn segment_count(&self) -> usize;
}
