use std::{collections::HashSet, fs, io};

use crate::{
    memtable::Memtable,
    sstable::{SSTable, get_sstables},
    storage_strategy::StorageStrategy,
};

struct Level {
    sstables: Vec<SSTable>,
    level_num: usize,
    max_bytes: u64,
    l0_threshold: usize,
    is_terminal: bool,
}

impl Level {
    pub fn append(&mut self, sst: SSTable) {
        self.sstables.push(sst);
    }

    fn total_bytes(&self) -> io::Result<u64> {
        let mut total = 0u64;
        for sst in &self.sstables {
            total += sst.size_on_disk()?
        }

        Ok(total)
    }

    fn needs_compaction(&self) -> io::Result<bool> {
        if self.level_num == 0 {
            return Ok(self.sstables.len() == self.l0_threshold);
        };
        Ok(self.total_bytes()? >= self.max_bytes)
    }

    fn overlapping_files<'a>(&'a self, min_key: &str, max_key: &str) -> Vec<&'a SSTable> {
        self.sstables
            .iter()
            .filter(|sst| {
                let Some((file_min, _)) = sst.get_min() else {
                    return false;
                };
                let Some((file_max, _)) = sst.get_max() else {
                    return false;
                };

                file_max.as_str() >= min_key && file_min.as_str() <= max_key
            })
            .collect()
    }

    pub fn key_range(&self) -> Option<(String, String)> {
        let min = self
            .sstables
            .iter()
            .filter_map(|sst| sst.get_min().as_ref())
            .map(|(key, _)| key)
            .min()?; // lexicographic min

        let max = self
            .sstables
            .iter()
            .filter_map(|sst| sst.get_max().as_ref())
            .map(|(key, _)| key)
            .max()?; // lexicographic max

        Some((min.clone(), max.clone()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &SSTable> + '_ {
        self.sstables.iter().rev()
    }

    pub fn squash(&mut self, db_path: &str, db_name: &str) -> io::Result<Option<SSTable>> {
        if self.sstables.is_empty() {
            return Ok(None);
        }
        let mut memtable = Memtable::new();
        for sst in self.iter() {
            for result in sst.iter()? {
                let record = result?;
                if memtable.entry(&record.key).is_some() {
                    continue;
                }
                if record.header.tombstone {
                    if self.is_terminal {
                        continue;
                    }
                    memtable.remove(record.key);
                } else {
                    memtable.insert(record.key, record.value);
                }
            }
            fs::remove_file(&sst.path)?;
        }
        self.sstables.clear();
        Ok(Some(SSTable::from_memtable(
            db_path,
            db_name,
            &memtable,
            Some(self.level_num),
        )?))
    }
}

pub struct Leveled {
    levels: Vec<Level>,
}

impl Leveled {
    pub fn new(num_levels: usize, l0_threshold: usize, l1_max_bytes: u64) -> Self {
        let mut levels = Vec::with_capacity(num_levels);
        for i in 0..num_levels {
            levels.push(Level {
                sstables: Vec::new(),
                level_num: i,
                max_bytes: l1_max_bytes * 10u64.pow(i.saturating_sub(1) as u32),
                l0_threshold,
                is_terminal: i == num_levels - 1,
            });
        }
        Leveled { levels }
    }

    pub fn load_from_dir(
        db_path: &str,
        db_name: &str,
        num_levels: usize,
        l0_threshold: usize,
        l1_max_bytes: u64,
    ) -> io::Result<Self> {
        let sstables = get_sstables(db_path, db_name)?;
        let mut strategy = Self::new(num_levels, l0_threshold, l1_max_bytes);
        for mut sst in sstables {
            sst.rebuild_index()?;
            let level = sst.level.unwrap_or(0);
            if level < strategy.levels.len() {
                strategy.levels[level].append(sst);
            } else {
                strategy.levels.last_mut().unwrap().append(sst);
            }
        }
        Ok(strategy)
    }

    fn compact_levels(
        &mut self,
        level_ids: (usize, usize),
        db_path: &str,
        db_name: &str,
    ) -> io::Result<()> {
        let (source_id, target_id) = level_ids;
        if source_id >= self.levels.len() || target_id >= self.levels.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "level index out of bounds: source={}, target={}, max={}",
                    source_id,
                    target_id,
                    self.levels.len() - 1
                ),
            ));
        }
        if source_id >= target_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "source level ({}) must be less than target level ({})",
                    source_id, target_id
                ),
            ));
        }
        let (left, right) = self.levels.split_at_mut(source_id + 1);
        let source = left.last_mut().unwrap();
        let target = &mut right[target_id - source_id - 1];

        match source.key_range() {
            Some((start, end)) => {
                let mut memtable = Memtable::new();
                let overlap = target.overlapping_files(&start, &end);
                let overlap_paths: HashSet<_> =
                    overlap.iter().map(|sst| sst.path.clone()).collect();
                for sst in source.iter() {
                    for result in sst.iter()? {
                        let record = result?;
                        if memtable.entry(&record.key).is_some() {
                            continue;
                        }
                        if record.header.tombstone {
                            if target.is_terminal {
                                continue; // drop tombstone
                            }
                            memtable.remove(record.key);
                        } else {
                            memtable.insert(record.key, record.value);
                        }
                    }
                }

                for sst in overlap {
                    for result in sst.iter()? {
                        let record = result?;
                        if memtable.entry(&record.key).is_some() {
                            continue;
                        }
                        if record.header.tombstone {
                            if target.is_terminal {
                                continue; // drop tombstone
                            }
                            memtable.remove(record.key);
                        } else {
                            memtable.insert(record.key, record.value);
                        }
                    }
                }

                let sstable =
                    SSTable::from_memtable(db_path, db_name, &memtable, Some(target.level_num))?;
                target
                    .sstables
                    .retain(|sst| !overlap_paths.contains(&sst.path));
                target.append(sstable);
                for path in &overlap_paths {
                    fs::remove_file(path)?;
                }
                for sst in source.iter() {
                    fs::remove_file(&sst.path)?;
                }

                source.sstables.clear();

                Ok(())
            }
            None => Ok(()),
        }
    }
}

impl StorageStrategy for Leveled {
    fn add_sstable(&mut self, sst: SSTable) -> io::Result<()> {
        let l0 = self
            .levels
            .first_mut()
            .ok_or_else(|| io::Error::other("Leveled storage strategy levels not allocated"))?;
        l0.append(sst);
        Ok(())
    }

    fn compact_if_needed(&mut self, db_path: &str, db_name: &str) -> io::Result<bool> {
        for i in 0..self.levels.len() {
            if self.levels[i].needs_compaction()? {
                if self.levels[i].is_terminal {
                    if let Some(sst) = self.levels[i].squash(db_path, db_name)? {
                        self.levels[i].append(sst);
                    }
                } else {
                    self.compact_levels((i, i + 1), db_path, db_name)?;
                }
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn compact_all(&mut self, db_path: &str, db_name: &str) -> io::Result<()> {
        for i in 0..self.levels.len() {
            if self.levels[i].is_terminal {
                if let Some(sst) = self.levels[i].squash(db_path, db_name)? {
                    self.levels[i].append(sst);
                }
            } else {
                self.compact_levels((i, i + 1), db_path, db_name)?;
            }
        }
        Ok(())
    }

    fn iter_for_key<'a>(&'a self, key: &'a str) -> Box<dyn Iterator<Item = &'a SSTable> + 'a> {
        Box::new(
            self.levels
                .iter()
                .flat_map(|bucket| bucket.iter())
                .filter(move |sst| sst.bloom.might_contain(key)), // skip definite misses
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
        Box::new(self.levels.iter().flat_map(|bucket| bucket.iter()))
    }

    fn segment_count(&self) -> usize {
        self.levels.iter().flat_map(|bucket| bucket.iter()).count()
    }
}
