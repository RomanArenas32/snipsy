//! Seguimiento de punta a punta: un rostro que cruza un video 16:9 se mantiene
//! centrado al exportar en 9:16. Necesita una foto con un rostro (`SNIPSY_FACE_IMAGE`).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use snipsy_core::{Asset, AssetId, Timeline};
use snipsy_media::{MediaInfo, VideoReader};

fn moving_face_video(photo: &str) -> PathBuf {
    let out = std::env::temp_dir().join("snipsy_moving_face.mp4");
    // Recorte alrededor de la cara de la foto, ampliado, cruzando de izquierda a derecha en 4 s.
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=0x406080:size=1280x720:rate=30:duration=4", "-i", photo,
            "-filter_complex", "[1]crop=110:110:190:60,scale=330:330[f];[0][f]overlay=x='100+t*200':y=150",
            "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok);
    out
}

fn face_x(path: &PathBuf, t: f64, size: (u32, u32)) -> Option<f32> {
    let mut r = VideoReader::open(path, size).unwrap();
    let img = r.frame_at(t).unwrap()?;
    let faces = snipsy_faces::Detector::new().detect(&img.rgba, img.width, img.height).unwrap();
    faces.iter().max_by(|a, b| (a.w * a.h).total_cmp(&(b.w * b.h))).map(|f| f.center().0)
}

#[test]
fn follows_face_when_reframing_to_vertical() {
    let Ok(photo) = std::env::var("SNIPSY_FACE_IMAGE") else {
        eprintln!("sin SNIPSY_FACE_IMAGE; se omite");
        return;
    };
    if snipsy_render::headless_device().is_none() {
        return;
    }
    let src = moving_face_video(&photo);
    let info = MediaInfo::probe(&src).unwrap();

    let t = std::time::Instant::now();
    let track = snipsy_faces::analyze(&src, info.duration, &AtomicBool::new(false), |_| {}).unwrap().expect("hay rostro");
    eprintln!("análisis de {:.0} s en {:.1} s ({} muestras)", info.duration, t.elapsed().as_secs_f64(), track.path.len());
    let (first, last) = (track.at(0.2).unwrap()[0], track.at(3.8).unwrap()[0]);
    assert!(first < 0.35 && last > 0.65, "la trayectoria va de izquierda a derecha: {first} → {last}");

    let mut tl = Timeline::new();
    let a = tl.add_asset(Asset {
        id: AssetId(0),
        path: src.clone(),
        name: "cara".into(),
        duration: info.duration,
        fps: info.fps,
        width: info.width,
        height: info.height,
        has_video: true,
        has_audio: false,
        still: false,
        faces: Some(track),
    });
    tl.append_asset(a);
    for (follow, name) in [(true, "con"), (false, "sin")] {
        let v = snipsy_core::reframe::reframe(&tl, 360, 640, follow);
        let out = std::env::temp_dir().join(format!("snipsy_follow_{name}.mp4"));
        let settings = snipsy_export::ExportSettings {
            path: out.clone(),
            codec: snipsy_export::VideoCodec::H264,
            quality: snipsy_export::Quality::High,
            hardware: true,
        };
        snipsy_export::export_blocking(&v, &settings).unwrap();
        let xs: Vec<Option<f32>> = [0.5, 2.0, 3.5].iter().map(|t| face_x(&out, *t, (360, 640))).collect();
        eprintln!("{name} seguimiento: centro del rostro en x = {xs:?}");
        if follow {
            for x in &xs {
                let x = x.expect("el rostro siempre en cuadro");
                assert!((x - 0.5).abs() < 0.12, "centrado: {xs:?}");
            }
        } else {
            // Recorte fijo al centro: al principio y al final la persona queda fuera.
            assert!(xs[0].is_none() && xs[2].is_none(), "{xs:?}");
        }
    }
}
