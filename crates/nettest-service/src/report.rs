//! Console output that is also appended to the capture file when running as the UAC child,
//! whose own console is invisible (see `elevate`).

use std::fs::File;
use std::io::Write;
use std::path::Path;

pub struct Report {
    capture: Option<File>,
}

impl Report {
    pub fn new(capture: Option<&Path>) -> Self {
        Self {
            capture: capture.and_then(|p| {
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(p)
                    .ok()
            }),
        }
    }

    pub fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        self.tee(s.as_ref());
    }

    pub fn err(&mut self, s: impl AsRef<str>) {
        eprintln!("{}", s.as_ref());
        self.tee(s.as_ref());
    }

    fn tee(&mut self, s: &str) {
        if let Some(f) = self.capture.as_mut() {
            let _ = writeln!(f, "{s}");
        }
    }
}
