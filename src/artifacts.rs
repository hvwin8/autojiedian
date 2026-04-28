use std::fs;
use std::fs::File;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use serde::Serialize;

use crate::settings::ArtifactSettings;

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    enabled: bool,
    root_dir: PathBuf,
}

impl ArtifactStore {
    pub fn new(settings: &ArtifactSettings) -> Self {
        Self {
            enabled: settings.enabled,
            root_dir: PathBuf::from(&settings.dir),
        }
    }

    pub fn write_json<T: Serialize>(&self, relative_path: &str, value: &T) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let full_path = self.full_path(relative_path);
        self.ensure_parent(&full_path)?;
        let file = File::create(full_path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, value).map_err(io::Error::other)
    }

    pub fn write_json_lines<T: Serialize>(
        &self,
        relative_path: &str,
        values: &[T],
    ) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let full_path = self.full_path(relative_path);
        self.ensure_parent(&full_path)?;
        let file = File::create(full_path)?;
        let mut writer = BufWriter::new(file);
        for value in values {
            serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()
    }

    pub fn path(&self, relative_path: &str) -> PathBuf {
        self.full_path(relative_path)
    }

    fn full_path(&self, relative_path: &str) -> PathBuf {
        self.root_dir.join(relative_path)
    }

    fn ensure_parent(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct SampleRecord {
        value: String,
    }

    #[test]
    fn test_disabled_artifact_store_is_noop() {
        let store = ArtifactStore::new(&ArtifactSettings {
            enabled: false,
            dir: "artifacts-test".to_string(),
        });
        assert!(store
            .write_json(
                "noop.json",
                &SampleRecord {
                    value: "ok".to_string(),
                },
            )
            .is_ok());
    }
}
