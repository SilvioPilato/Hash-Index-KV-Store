use std::{path::PathBuf, time::SystemTime};

pub struct Segment {
    pub segment_name: String,
    pub timestamp: u64,
}

impl Segment {
    pub fn new(segment_name: &str) -> Result<Self, std::time::SystemTimeError> {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs();
        Ok(Self {
            segment_name: segment_name.to_string(),
            timestamp,
        })
    }

    pub fn filename(&self) -> String {
        format!("{}_{}.db", self.segment_name, self.timestamp)
    }

    pub fn path(&self, dir: &str) -> PathBuf {
        PathBuf::from(dir).join(self.filename())
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
