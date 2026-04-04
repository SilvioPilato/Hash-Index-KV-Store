use std::{fs, io};

use crate::{
    memtable::Memtable,
    sstable::{SSTable, get_sstables},
    storage_strategy::StorageStrategy,
};

// Minimum acceptable ratio between a new SSTable's size and a bucket's
// average SSTable size for the new SSTable to be assigned to that tier.
const MIN_TIER_COEFFICIENT: f64 = 0.5;
const MAX_TIER_COEFFICIENT: f64 = 1.5;

pub struct SizeTiered {
    sstables: Vec<Vec<SSTable>>,
    min_threshold: usize,
    max_threshold: usize,
}

impl SizeTiered {
    pub fn new(min_threshold: usize, max_threshold: usize) -> Self {
        SizeTiered {
            sstables: Vec::new(),
            min_threshold,
            max_threshold,
        }
    }

    pub fn load_from_dir(
        db_path: &str,
        db_name: &str,
        min_threshold: usize,
        max_threshold: usize,
    ) -> io::Result<Self> {
        let sstables = get_sstables(db_path, db_name)?;
        let mut strategy = Self::new(min_threshold, max_threshold);
        for mut sst in sstables {
            sst.rebuild_index()?;
            strategy.add_sstable(sst)?;
        }
        Ok(strategy)
    }
}

impl StorageStrategy for SizeTiered {
    fn add_sstable(&mut self, sst: SSTable) -> io::Result<()> {
        let new_sst_size = sst.size_on_disk()? as f64;
        let mut target_idx: Option<usize> = None;
        let mut best_match_diff = f64::MAX;

        for (i, bucket) in self.sstables.iter().enumerate() {
            let total = bucket
                .iter()
                .try_fold(0u64, |acc, table| table.size_on_disk().map(|s| acc + s))?
                as f64;

            let avg = if !bucket.is_empty() {
                total / bucket.len() as f64
            } else {
                0.0
            };
            let ratio = if new_sst_size != 0.0 {
                new_sst_size / avg
            } else {
                0.0
            };

            if (MIN_TIER_COEFFICIENT..=MAX_TIER_COEFFICIENT).contains(&ratio) {
                let ratio_diff = (avg - new_sst_size).abs();
                if ratio_diff < best_match_diff {
                    best_match_diff = ratio_diff;
                    target_idx = Some(i);
                }
            }
        }

        if let Some(i) = target_idx {
            self.sstables[i].push(sst);
        } else {
            self.sstables.push(vec![sst]);
        }
        Ok(())
    }

    fn compact_if_needed(&mut self, db_path: &str, db_name: &str) -> io::Result<bool> {
        for bucket in &mut self.sstables {
            if bucket.len() < self.min_threshold && bucket.len() < self.max_threshold {
                continue;
            }
            let mut memtable = Memtable::new();
            for segment in bucket.iter() {
                for result in segment.iter()? {
                    let record = result?;
                    // Skip if key already exists (we iterate newest-to-oldest, so first seen wins)
                    if memtable.entry(&record.key).is_some() {
                        continue;
                    }
                    if record.header.tombstone {
                        memtable.remove(record.key);
                    } else {
                        memtable.insert(record.key, record.value);
                    }
                }
                fs::remove_file(&segment.path)?;
            }

            memtable.drop_tombstones();

            let new_sst = SSTable::from_memtable(db_path, db_name, &memtable, None)?;
            *bucket = vec![new_sst];
            return Ok(true);
        }

        Ok(false)
    }

    fn compact_all(&mut self, db_path: &str, db_name: &str) -> std::io::Result<()> {
        let mut memtable = Memtable::new();
        for sst in self.iter_all() {
            for result in sst.iter()? {
                let record = result?;
                // Skip if key already exists (we iterate newest-to-oldest, so first seen wins)
                if memtable.entry(&record.key).is_some() {
                    continue;
                }
                if record.header.tombstone {
                    memtable.remove(record.key);
                } else {
                    memtable.insert(record.key, record.value);
                }
            }
            fs::remove_file(&sst.path)?;
        }
        memtable.drop_tombstones();

        self.sstables = vec![vec![SSTable::from_memtable(
            db_path, db_name, &memtable, None,
        )?]];
        Ok(())
    }

    fn iter_for_key<'a>(&'a self, key: &'a str) -> Box<dyn Iterator<Item = &'a SSTable> + 'a> {
        Box::new(
            self.sstables
                .iter()
                .flat_map(|bucket| bucket.iter())
                .rev() // newest-to-oldest
                .filter(|sst| sst.bloom.might_contain(key)), // skip definite misses
        )
    }

    fn iter_files_for_range<'a>(
        &'a self,
        start: &str,
        end: &str,
    ) -> Box<dyn Iterator<Item = &'a SSTable> + 'a> {
        let mut files = Vec::new();
        for sst in self.iter_all() {
            let Some(seg_min) = sst.get_min() else {
                continue;
            };
            let Some(seg_max) = sst.get_max() else {
                continue;
            };
            let (min, _) = seg_min;
            let (max, _) = seg_max;
            if max.as_str() < start || min.as_str() > end {
                continue;
            }
            files.push(sst);
        }
        Box::new(files.into_iter())
    }

    fn iter_all<'a>(&'a self) -> Box<dyn Iterator<Item = &'a crate::sstable::SSTable> + 'a> {
        Box::new(
            self.sstables.iter().flat_map(|bucket| bucket.iter()).rev(), // newest-to-oldest
        )
    }

    fn segment_count(&self) -> usize {
        self.sstables
            .iter()
            .flat_map(|bucket| bucket.iter())
            .count()
    }
}
