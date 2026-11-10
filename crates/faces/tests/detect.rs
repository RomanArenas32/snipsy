//! Detección real con Vision. Con `SNIPSY_FACE_IMAGE=foto.jpg` usa esa foto; si no, se omite.

#[test]
fn detects_faces_in_photo() {
    let Ok(path) = std::env::var("SNIPSY_FACE_IMAGE") else {
        eprintln!("sin SNIPSY_FACE_IMAGE; se omite");
        return;
    };
    // Decodificar con ffmpeg a RGBA crudo.
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i", &path, "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .unwrap();
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=width,height", "-of", "csv=p=0", &path])
        .output()
        .unwrap();
    let dims = String::from_utf8_lossy(&probe.stdout);
    let mut it = dims.trim().split(',').map(|v| v.parse::<usize>().unwrap());
    let (w, h) = (it.next().unwrap(), it.next().unwrap());
    let det = snipsy_faces::Detector::new();
    let t = std::time::Instant::now();
    let faces = det.detect(&out.stdout, w, h).unwrap();
    eprintln!("{} rostro(s) en {:.0} ms", faces.len(), t.elapsed().as_secs_f64() * 1000.0);
    for f in &faces {
        let (cx, cy) = f.center();
        eprintln!("  centro ({:.0}, {:.0}) px · {:.0}×{:.0} · confianza {:.2}", cx * w as f32, cy * h as f32, f.w * w as f32, f.h * h as f32, f.confidence);
    }
    assert!(!faces.is_empty());
}
