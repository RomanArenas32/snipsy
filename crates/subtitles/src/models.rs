//! Modelos de Whisper (formato ggml de whisper.cpp): se descargan una vez y se guardan.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, bail, Context, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Base,
    Small,
    LargeTurbo,
}

impl Model {
    pub const ALL: [Model; 3] = [Model::Small, Model::Base, Model::LargeTurbo];

    pub fn label(self) -> &'static str {
        match self {
            Model::Base => "Rápido (base · 148 MB)",
            Model::Small => "Equilibrado (small · 488 MB)",
            Model::LargeTurbo => "Máxima precisión (large-v3-turbo · 1,6 GB)",
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            Model::Base => "ggml-base.bin",
            Model::Small => "ggml-small.bin",
            Model::LargeTurbo => "ggml-large-v3-turbo.bin",
        }
    }

    pub fn url(self) -> String {
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", self.file_name())
    }

    pub fn path(self) -> PathBuf {
        models_dir().join(self.file_name())
    }

    pub fn is_downloaded(self) -> bool {
        self.path().exists()
    }
}

/// `~/Library/Application Support/Snipsy/models`
pub fn models_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Library/Application Support/Snipsy/models")
}

/// Descarga el modelo si no está. `progress(bytes, total)`. Se puede cancelar.
pub fn ensure(model: Model, cancel: &AtomicBool, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let path = model.path();
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(models_dir()).context("no se pudo crear la carpeta de modelos")?;
    let part = path.with_extension("part");
    let resp = ureq::get(&model.url()).call().map_err(|e| anyhow!("no se pudo descargar el modelo: {e}"))?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&part)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            bail!("cancelado");
        }
        let n = reader.read(&mut buf).context("error de red al descargar el modelo")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    file.flush()?;
    if total.is_some_and(|t| t != done) {
        let _ = std::fs::remove_file(&part);
        bail!("la descarga del modelo quedó incompleta");
    }
    std::fs::rename(&part, &path)?;
    Ok(path)
}
