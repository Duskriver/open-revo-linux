//! Read-only raw event telemetry. Mapping keys to actions is a separate policy.
use super::vault::linux_events as wire;
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{ErrorKind, Read},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Default)]
pub struct EventSnapshot {
    pub available: bool,
    pub last_sequence: Option<u64>,
    pub last_code: Option<u32>,
    pub dropped: u64,
    pub error: Option<String>,
}

fn decode(frame: &[u8]) -> Result<(u64, u32), String> {
    if frame.len() != wire::FRAME_SIZE {
        return Err("WMI 事件帧长度不匹配".into());
    }
    let version = u32::from_le_bytes(
        frame[wire::VERSION_OFFSET..wire::VERSION_OFFSET + 4]
            .try_into()
            .unwrap(),
    );
    if version != wire::VERSION as u32 {
        return Err("WMI 事件协议版本不兼容，请配套更新驱动与应用".into());
    }
    Ok((
        u64::from_le_bytes(
            frame[wire::SEQUENCE_OFFSET..wire::SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap(),
        ),
        u32::from_le_bytes(
            frame[wire::CODE_OFFSET..wire::CODE_OFFSET + 4]
                .try_into()
                .unwrap(),
        ),
    ))
}

#[derive(Debug, Clone, Copy)]
pub struct RawEvent {
    pub code: u32,
}

pub struct WmiEvents {
    path: PathBuf,
    file: Option<File>,
    state: EventSnapshot,
    pending: Vec<RawEvent>,
    retry_at: Option<Instant>,
}
impl Default for WmiEvents {
    fn default() -> Self {
        Self::new(PathBuf::from("/dev/openrevo_events"))
    }
}
impl WmiEvents {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            state: EventSnapshot::default(),
            pending: Vec::new(),
            retry_at: None,
        }
    }
    pub fn snapshot(&self) -> EventSnapshot {
        self.state.clone()
    }
    fn accept(&mut self, frame: &[u8]) -> Result<(), String> {
        let (sequence, code) = decode(frame)?;
        if let Some(last) = self.state.last_sequence {
            // Wrapping subtraction also handles the sequence counter wrapping.
            let distance = sequence.wrapping_sub(last);
            if distance == 0 || distance > u64::MAX / 2 {
                return Err("WMI 事件序号倒退或重复".into());
            }
            self.state.dropped = self.state.dropped.saturating_add(distance - 1);
        }
        self.state.last_sequence = Some(sequence);
        self.state.last_code = Some(code);
        Ok(())
    }
    fn read_pending(&mut self) -> Result<(), String> {
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&self.path)
                    .map_err(|e| {
                        format!("WMI 事件通道不可用：{e}；设备可能未提供此 GUID 或已由系统驱动接管")
                    })?,
            );
            self.state.last_sequence = None;
            self.state.last_code = None;
        }
        // Bound work per runtime tick even if firmware continuously emits events.
        for _ in 0..wire::QUEUE_SIZE {
            let mut frame = [0; wire::FRAME_SIZE];
            match self.file.as_mut().unwrap().read(&mut frame) {
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(format!("WMI 事件读取失败：{e}")),
                Ok(n) => {
                    self.accept(&frame[..n])?;
                    self.pending.push(RawEvent {
                        code: self.state.last_code.unwrap(),
                    });
                }
            }
        }
        Ok(())
    }
    pub fn reset(&mut self) {
        self.file = None;
        self.pending.clear();
        self.retry_at = None;
        self.state.available = false;
    }
    pub fn poll(&mut self) -> Vec<RawEvent> {
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return Vec::new();
        }
        self.pending.clear();
        let dropped = self.state.dropped;
        let result = self.read_pending();
        self.state.available = result.is_ok();
        self.state.error = result.err();
        if !self.state.available {
            self.file = None;
            self.pending.clear();
            self.retry_at = Some(Instant::now() + Duration::from_secs(2));
        }
        // A gap or malformed batch cannot be replayed as user intent.
        if self.state.dropped != dropped {
            self.pending.clear();
        }
        std::mem::take(&mut self.pending)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(sequence: u64, code: u32) -> [u8; wire::FRAME_SIZE] {
        let mut frame = [0; wire::FRAME_SIZE];
        frame[wire::SEQUENCE_OFFSET..wire::SEQUENCE_OFFSET + 8]
            .copy_from_slice(&sequence.to_le_bytes());
        frame[wire::CODE_OFFSET..wire::CODE_OFFSET + 4].copy_from_slice(&code.to_le_bytes());
        frame[wire::VERSION_OFFSET..wire::VERSION_OFFSET + 4]
            .copy_from_slice(&(wire::VERSION as u32).to_le_bytes());
        frame
    }
    #[test]
    fn malformed_or_incompatible_frames_are_rejected() {
        assert!(decode(&[]).is_err());
        assert!(decode(&frame(1, 42)[..wire::FRAME_SIZE - 1]).is_err());
        let mut bytes = frame(1, 42);
        bytes[wire::VERSION_OFFSET] += 1;
        assert!(decode(&bytes).is_err());
    }
    #[test]
    fn raw_unknown_events_and_sequence_gaps_are_preserved() {
        let mut reader = WmiEvents::default();
        reader.accept(&frame(7, u32::MAX)).unwrap();
        reader.accept(&frame(10, 42)).unwrap();
        assert_eq!(reader.snapshot().dropped, 2);
        assert_eq!(reader.snapshot().last_code, Some(42));
        assert!(reader.accept(&frame(10, 3)).is_err());
        assert!(reader.accept(&frame(9, 3)).is_err());
        assert_eq!(reader.snapshot().last_code, Some(42));
    }
    #[test]
    fn sequence_wrap_is_not_reported_as_loss() {
        let mut reader = WmiEvents::default();
        reader.accept(&frame(u64::MAX, 1)).unwrap();
        reader.accept(&frame(0, 2)).unwrap();
        assert_eq!(reader.snapshot().dropped, 0);
    }
    #[test]
    fn missing_device_never_blocks_or_claims_support() {
        let dir = tempfile::tempdir().unwrap();
        let mut reader = WmiEvents::new(dir.path().join("missing"));
        reader.poll();
        assert!(!reader.snapshot().available);
        assert!(reader.snapshot().error.is_some());
    }
}
