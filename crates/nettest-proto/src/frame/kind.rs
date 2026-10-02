//! Frame kinds. Numeric values are part of the wire contract; never renumber.

use super::FrameError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum Kind {
    Hello = 1,
    HelloAck = 2,
    Probe = 3,
    Echo = 4,
    Heartbeat = 5,
    HeartbeatAck = 6,
    TpStart = 7,
    TpData = 8,
    TpEnd = 9,
    TpResult = 10,
    Error = 11,
    Bye = 12,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Hello,
        Kind::HelloAck,
        Kind::Probe,
        Kind::Echo,
        Kind::Heartbeat,
        Kind::HeartbeatAck,
        Kind::TpStart,
        Kind::TpData,
        Kind::TpEnd,
        Kind::TpResult,
        Kind::Error,
        Kind::Bye,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Hello => "hello",
            Kind::HelloAck => "hello-ack",
            Kind::Probe => "probe",
            Kind::Echo => "echo",
            Kind::Heartbeat => "heartbeat",
            Kind::HeartbeatAck => "heartbeat-ack",
            Kind::TpStart => "tp-start",
            Kind::TpData => "tp-data",
            Kind::TpEnd => "tp-end",
            Kind::TpResult => "tp-result",
            Kind::Error => "error",
            Kind::Bye => "bye",
        }
    }
}

impl TryFrom<u8> for Kind {
    type Error = FrameError;
    fn try_from(v: u8) -> Result<Self, FrameError> {
        Kind::ALL
            .iter()
            .copied()
            .find(|k| *k as u8 == v)
            .ok_or(FrameError::BadKind(v))
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
