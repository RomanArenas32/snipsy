//! Transcripción real con Whisper. Requiere red la primera vez (descarga el modelo base):
//! `cargo test --release -p snipsy-subtitles -- --ignored --nocapture`

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossbeam_channel::unbounded;
use snipsy_core::{Asset, AssetId, Timeline};
use snipsy_media::MediaInfo;
use snipsy_subtitles::*;

fn spoken_clip() -> Option<PathBuf> {
    let dir = std::env::temp_dir();
    let path = dir.join("snipsy_voz.mp4");
    if path.exists() {
        return Some(path);
    }
    let aiff = dir.join("snipsy_voz.aiff");
    let said = Command::new("say")
        .args(["-v", "Paulina", "-o"])
        .arg(&aiff)
        .arg("Hola a todos. Hoy les presento nuestra nueva colección de verano, con descuentos de hasta el cincuenta por ciento.")
        .status()
        .is_ok_and(|s| s.success());
    if !said {
        return None;
    }
    // Video negro con la voz empezando en el segundo 1.
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=30:d=12", "-i"])
        .arg(&aiff)
        .args(["-filter_complex", "[1:a]adelay=1000|1000,apad[a]", "-map", "0:v", "-map", "[a]", "-shortest", "-c:v", "libx264", "-c:a", "aac"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(path)
}

#[test]
#[ignore]
fn transcribes_spanish_voice_into_timeline() {
    let _ = env_logger::builder().is_test(true).try_init();
    let Some(src) = spoken_clip() else { return };
    let info = MediaInfo::probe(&src).unwrap();
    let mut tl = Timeline::new();
    let a = tl.add_asset(Asset {
        id: AssetId(0),
        path: src,
        name: "voz".into(),
        duration: info.duration,
        fps: info.fps,
        width: info.width,
        height: info.height,
        has_video: true,
        has_audio: true,
        still: false,
        faces: None,
    });
    tl.width = 1080;
    tl.height = 1920;
    tl.append_asset(a);

    let (tx, rx) = unbounded();
    let settings = SubtitleSettings { model: Model::Base, language: Some("es".into()), style: CaptionStyle::Reel };
    let t = std::time::Instant::now();
    let captions = run(&tl, &settings, &tx, &Arc::new(AtomicBool::new(false))).unwrap();
    eprintln!("transcrito en {:.1} s ({} eventos)", t.elapsed().as_secs_f64(), rx.try_iter().count());
    for c in &captions {
        eprintln!("{:6.2} – {:6.2}  {}", c.start, c.end, c.text);
    }
    let all = captions.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ");
    for word in ["HOLA", "COLECCIÓN", "VERANO", "DESCUENTOS"] {
        assert!(all.contains(word), "falta «{word}» en: {all}");
    }
    // Empieza cuando empieza la voz (segundo 1), no antes.
    // (la voz real empieza en 1,00 s y la segunda frase en 2,11 s, según silencedetect)
    assert!((captions[0].start - 1.0).abs() < 0.2, "inicio {}", captions[0].start);
    let hoy = captions.iter().find(|c| c.text.starts_with("HOY")).unwrap();
    assert!((hoy.start - 2.11).abs() < 0.2, "«Hoy» en {}", hoy.start);
    assert!(captions.iter().all(|c| c.text.split(' ').count() <= 3));

    let look = CaptionLook::default();
    let n = apply(&mut tl, &captions, &look);
    let track = subtitle_track(&tl).unwrap();
    assert_eq!(tl.video_tracks[track].clips.len(), n);
    let srt = to_srt(&captions_from_timeline(&tl));
    assert!(srt.starts_with("1\n00:00:0"));
    // Volver a generar reemplaza en lugar de duplicar.
    apply(&mut tl, &captions, &look);
    assert_eq!(tl.video_tracks.len(), 2);

    // Reel con subtítulos, para revisarlo a ojo.
    let out = std::env::temp_dir().join("snipsy_reel_subtitulado.mp4");
    let settings = snipsy_export::ExportSettings {
        path: out.clone(),
        codec: snipsy_export::VideoCodec::H264,
        quality: snipsy_export::Quality::Medium,
        hardware: true,
    };
    snipsy_export::export_blocking(&tl, &settings).unwrap();
    eprintln!("reel: {}", out.display());
}
