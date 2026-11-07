//! LUTs 3D en formato `.cube` (Adobe / Resolve).

use std::path::Path;

/// LUT 3D cúbica: `size³` colores RGBA8, con el rojo variando más rápido.
#[derive(Clone, Debug)]
pub struct Lut {
    pub size: u32,
    pub rgba: Vec<u8>,
}

impl Lut {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("no se pudo leer {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut size = 0u32;
        let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
        let mut values: Vec<[f32; 3]> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let first = parts.next().unwrap_or_default();
            let nums = |parts: std::str::SplitWhitespace| -> Result<[f32; 3], String> {
                let v: Vec<f32> = parts.map(|p| p.parse::<f32>()).collect::<Result<_, _>>().map_err(|_| format!("línea inválida: {line}"))?;
                v.get(..3).map(|v| [v[0], v[1], v[2]]).ok_or_else(|| format!("línea inválida: {line}"))
            };
            match first {
                "TITLE" => {}
                "LUT_1D_SIZE" => return Err("las LUT 1D no están soportadas (solo 3D)".into()),
                "LUT_3D_SIZE" => {
                    size = parts.next().and_then(|n| n.parse().ok()).ok_or("LUT_3D_SIZE inválido")?;
                    if !(2..=128).contains(&size) {
                        return Err(format!("tamaño de LUT no soportado: {size}"));
                    }
                }
                "DOMAIN_MIN" => min = nums(parts)?,
                "DOMAIN_MAX" => max = nums(parts)?,
                _ if first.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') => {
                    values.push(nums(line.split_whitespace())?);
                }
                // Palabras clave desconocidas (p. ej. LUT_3D_INPUT_RANGE): se ignoran.
                _ => {}
            }
        }
        if size == 0 {
            return Err("falta LUT_3D_SIZE (¿es un archivo .cube 3D?)".into());
        }
        let n = (size * size * size) as usize;
        if values.len() != n {
            return Err(format!("la LUT tiene {} valores, se esperaban {n}", values.len()));
        }
        let mut rgba = Vec::with_capacity(n * 4);
        for v in values {
            for c in 0..3 {
                let x = (v[c] - min[c]) / (max[c] - min[c]).max(1e-6);
                rgba.push((x.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            rgba.push(255);
        }
        Ok(Self { size, rgba })
    }

    /// LUT neutra (no cambia los colores).
    pub fn identity(size: u32) -> Self {
        let mut rgba = Vec::with_capacity((size * size * size * 4) as usize);
        let s = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    rgba.extend([r, g, b].map(|v| (v as f32 / s * 255.0).round() as u8));
                    rgba.push(255);
                }
            }
        }
        Self { size, rgba }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cube_files() {
        let text = "TITLE \"test\"\n# comentario\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let lut = Lut::parse(text).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.rgba, Lut::identity(2).rgba);
        assert!(Lut::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(Lut::parse("LUT_1D_SIZE 16\n").is_err());
    }
}
