//! The `monitor.toml` the Alerts and Monitor tabs edit: one in-memory `MonitorConfig` loaded
//! from disk (encoding-aware, like the monitor itself) and written back as a whole.
//!
//! Invariant: a file that failed to parse is never overwritten. `load_error` stays set, which
//! blocks `save` until the user reloads a fixed file or explicitly discards it.

use std::path::PathBuf;

use crate::monitor::config::{self, Encoding, MonitorConfig};

pub struct MonitorDraft {
    pub path: PathBuf,
    pub cfg: MonitorConfig,
    pub load_error: Option<String>,
    /// Encoding the file was found in (`None` when it did not exist).
    pub encoding: Option<Encoding>,
    pub dirty: bool,
}

impl MonitorDraft {
    pub fn load(path: PathBuf) -> Self {
        let mut d = Self {
            path,
            cfg: MonitorConfig::default(),
            load_error: None,
            encoding: None,
            dirty: false,
        };
        d.reload();
        d
    }

    /// Re-read the file at `path`; a missing file is an empty draft, not an error.
    pub fn reload(&mut self) {
        self.cfg = MonitorConfig::default();
        self.load_error = None;
        self.encoding = None;
        self.dirty = false;
        if !self.path.exists() {
            return;
        }
        match config::load(&self.path) {
            Ok((cfg, enc)) => {
                self.cfg = cfg;
                self.encoding = Some(enc);
            }
            Err(e) => self.load_error = Some(e),
        }
    }

    /// Forget a file that would not parse and start from an empty draft (the next save
    /// overwrites it).
    pub fn discard_load_error(&mut self) {
        self.load_error = None;
        self.cfg = MonitorConfig::default();
        self.dirty = true;
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Write the draft with private permissions (it may hold tokens).
    pub fn save(&mut self) -> Result<(), String> {
        if let Some(e) = &self.load_error {
            return Err(format!(
                "not saved: {} could not be parsed ({e}); fix it, or press D to discard it",
                self.path.display()
            ));
        }
        let text = config::to_toml(&self.cfg)?;
        nettest_service::files::write_private(&self.path, text.as_bytes())
            .map_err(|e| format!("write {}: {e}", self.path.display()))?;
        self.dirty = false;
        self.encoding = Some(Encoding::Utf8);
        Ok(())
    }
}
