//! Server log: the shared `nettest_proto::log::LineLog` pipeline under the names the server has
//! always used. The implementation moved to nettest-proto so the client monitor logs identically.

pub use nettest_proto::log::{LineLog as ServerLog, LogLine, LogOptions, LogTail};
