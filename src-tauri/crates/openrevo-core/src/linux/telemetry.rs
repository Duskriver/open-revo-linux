use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingStatus {
    Ok,
    Unavailable,
    Fault,
    WarmingUp,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadingMetadata {
    pub source: String,
    /// Time of the successful source read, not the enclosing snapshot or a cache hit.
    /// Unknown, faulty and incomplete differential readings have no sample time.
    pub sampled_at_ms: Option<u128>,
    pub status: ReadingStatus,
}
