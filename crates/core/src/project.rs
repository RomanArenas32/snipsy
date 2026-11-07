//! Guardar / abrir proyectos (`.snipsy`, JSON).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Timeline;

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    timeline: Timeline,
}

pub fn save(path: &Path, timeline: &Timeline) -> std::io::Result<()> {
    let file = ProjectFile { version: VERSION, timeline: timeline.clone() };
    let json = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

pub fn load(path: &Path) -> std::io::Result<Timeline> {
    let data = std::fs::read(path)?;
    let file: ProjectFile = serde_json::from_slice(&data).map_err(std::io::Error::other)?;
    if file.version > VERSION {
        return Err(std::io::Error::other(format!(
            "el proyecto es de una versión más nueva ({})",
            file.version
        )));
    }
    Ok(file.timeline)
}
