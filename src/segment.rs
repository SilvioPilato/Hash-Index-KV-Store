use std::io;
use std::{fs::read_dir, path::PathBuf, time::SystemTime};
pub struct Segment {
    pub segment_name: String,
    pub timestamp: u64,
}

impl Segment {
    pub fn new(segment_name: &str) -> Result<Self, std::time::SystemTimeError> {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_millis() as u64;
        Ok(Self {
            segment_name: segment_name.to_string(),
            timestamp,
        })
    }

    pub fn filename(&self) -> String {
        format!("{}_{}.db", self.segment_name, self.timestamp)
    }

    pub fn hint_filename(&self) -> String {
        format!("{}_{}.hint", self.segment_name, self.timestamp)
    }

    pub fn path(&self, dir: &str) -> PathBuf {
        PathBuf::from(dir).join(self.filename())
    }

    pub fn hint_path(&self, dir: &str) -> PathBuf {
        PathBuf::from(dir).join(self.hint_filename())
    }

    pub fn parse(filename: &str) -> Option<Self> {
        let stem = filename.strip_suffix(".db")?;
        let (name, ts) = stem.rsplit_once('_')?;
        let timestamp = ts.parse().ok()?;
        Some(Self {
            segment_name: name.to_string(),
            timestamp,
        })
    }
}

pub fn get_segments(dir: &str, db_name: &str) -> io::Result<Vec<Segment>> {
    let mut segments: Vec<Segment> = read_dir(dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            let segment = Segment::parse(&name)?;
            if segment.segment_name == db_name {
                Some(segment)
            } else {
                None
            }
        })
        .collect();
    segments.sort_by_key(|s| s.timestamp);
    Ok(segments)
}

pub fn get_last_segment(dir: &str, db_name: &str) -> io::Result<Option<Segment>> {
    Ok(get_segments(dir, db_name)?.into_iter().last())
}
