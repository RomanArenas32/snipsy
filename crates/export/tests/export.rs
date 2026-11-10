//! Exporta un proyecto real (video + texto + formato vertical) y verifica el MP4.

use std::path::PathBuf;
use std::process::Command;

use snipsy_core::{Asset, AssetId, Edge, TextStyle, Timeline, Transform};
use snipsy_export::*;
use snipsy_media::{AudioReader, MediaInfo, VideoReader};

/// Video de color liso de 3 s con un tono de 440 Hz.
fn color_clip(name: &str, color: &str) -> Option<PathBuf> {
    let path = std::env::temp_dir().join(format!("snipsy_fx_{name}.mp4"));
    if path.exists() {
        return Some(path);
    }
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg(format!("color=c={color}:size=320x180:rate=30:duration=3"))
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=3",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(path)
}

fn add(tl: &mut Timeline, path: &PathBuf) -> AssetId {
    let info = MediaInfo::probe(path).unwrap();
    tl.add_asset(Asset {
        id: AssetId(0),
        path: path.clone(),
        name: "clip".into(),
        duration: info.duration,
        fps: info.fps,
        width: info.width,
        height: info.height,
        has_video: true,
        has_audio: true,
        still: false,
        faces: None,
    })
}

fn rms(path: &PathBuf, t: f64) -> f32 {
    let mut r = AudioReader::open(path, 48_000, 2).unwrap();
    let mut buf = vec![0.0f32; 4800 * 2];
    r.mix_into(t, &mut buf, 1.0).unwrap();
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
}

#[test]
fn exports_fades_dissolve_color_and_volume() {
    let (Some(red), Some(green)) = (color_clip("red", "0xE02020"), color_clip("green", "0x20C040")) else { return };
    if snipsy_render::headless_device().is_none() {
        eprintln!("sin GPU; se omite");
        return;
    }
    let mut tl = Timeline::new();
    let (ra, ga) = (add(&mut tl, &red), add(&mut tl, &green));
    // A (rojo): cuadros 0–45, con material sobrante después.
    let a = tl.append_asset(ra);
    assert!(tl.trim(a[0], Edge::End, 45));
    // B (verde): pegado a A en el cuadro 45, empezando 15 cuadros dentro del archivo.
    let b = tl.insert_asset(ga, 45, 0, 0);
    assert!(tl.trim(b[0], Edge::Start, 60));
    assert!(tl.move_clips(&[b[0]], -15, (0, 0)));
    assert_eq!(tl.video_tracks[0].clips[1].start, 45);
    for id in &b {
        tl.clip_mut(*id).unwrap().transition_in = 20;
    }
    let ca = tl.clip_mut(a[0]).unwrap();
    ca.fade_in = 15;
    tl.clip_mut(a[1]).unwrap().volume_db = -6.0;
    tl.clip_mut(b[0]).unwrap().color.saturation = 0.0;
    tl.clip_mut(b[1]).unwrap().fade_out = 15;
    assert_eq!(tl.duration(), 120);

    let path = std::env::temp_dir().join("snipsy_export_fx.mp4");
    let settings = ExportSettings { path: path.clone(), codec: VideoCodec::H264, quality: Quality::High, hardware: true };
    export_blocking(&tl, &settings).unwrap();

    let mut r = VideoReader::open(&path, (320, 180)).unwrap();
    let mut at = |frame: i64| {
        let img = r.frame_at(frame as f64 / 30.0).unwrap().unwrap();
        let i = (90 * img.width + 160) * 4;
        [img.rgba[i] as i32, img.rgba[i + 1] as i32, img.rgba[i + 2] as i32]
    };
    let near = |c: [i32; 3], e: [i32; 3], tol: i32| c.iter().zip(e).all(|(a, b)| (a - b).abs() <= tol);
    let black = at(0);
    assert!(black.iter().all(|v| *v < 20), "el fundido empieza en negro: {black:?}");
    let half = at(7);
    assert!((60..150).contains(&half[0]), "mitad del fundido: {half:?}");
    let red_px = at(25);
    assert!(near(red_px, [0xE0, 0x20, 0x20], 14), "rojo: {red_px:?}");
    // Verde sin saturación: gris (luma de 0x20C040 ≈ 150).
    let gray = at(80);
    assert!((gray[0] - gray[1]).abs() < 8 && (gray[1] - gray[2]).abs() < 8 && gray[0] > 110, "gris: {gray:?}");
    // A mitad de la disolvencia: mezcla de rojo y gris.
    let mid = at(45);
    let expect = [(0xE0 + gray[0]) / 2, (0x20 + gray[1]) / 2, (0x20 + gray[2]) / 2];
    assert!(near(mid, expect, 18), "disolvencia: {mid:?} ≈ {expect:?}");
    let early = at(36);
    assert!(near(early, red_px, 25), "inicio de la disolvencia casi rojo: {early:?}");

    // Audio: −6 dB ≈ la mitad de amplitud; el final se desvanece.
    let src = rms(&red, 0.8);
    let out = rms(&path, 0.8);
    assert!((out / src - 0.5).abs() < 0.06, "−6 dB: {out} / {src}");
    let full = rms(&path, 2.5);
    assert!((full / src - 1.0).abs() < 0.1, "B a volumen normal: {full} / {src}");
    let tail = rms(&path, 3.9);
    assert!(tail < src * 0.15, "fundido de salida: {tail}");
}

fn sample_video() -> Option<PathBuf> {
    let path = std::env::temp_dir().join("snipsy_export_src.mp4");
    if path.exists() {
        return Some(path);
    }
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=0x2060E0:size=640x360:rate=30:duration=2",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(path)
}

#[test]
fn exports_vertical_reel_with_text_and_audio() {
    let Some(src) = sample_video() else { return };
    if snipsy_render::headless_device().is_none() {
        eprintln!("sin GPU; se omite");
        return;
    }
    let mut tl = Timeline::new();
    let info = MediaInfo::probe(&src).unwrap();
    let a = tl.add_asset(Asset {
        id: AssetId(0),
        path: src.clone(),
        name: "fuente".into(),
        duration: info.duration,
        fps: info.fps,
        width: info.width,
        height: info.height,
        has_video: true,
        has_audio: true,
        still: false,
        faces: None,
    });
    // Reel vertical: el video horizontal "rellena" el lienzo 9:16.
    tl.width = 360;
    tl.height = 640;
    let ids = tl.append_asset(a);
    let cover = snipsy_core::compose::cover_scale((640.0, 360.0), (360.0, 640.0));
    tl.clip_mut(ids[0]).unwrap().transform = Transform { scale: cover, ..Default::default() };
    // Texto blanco en la parte de arriba durante el primer segundo.
    let t = tl.insert_text(0, 1, 30, TextStyle { text: "HOLA".into(), size: 60.0, outline: None, ..Default::default() });
    tl.clip_mut(t).unwrap().transform.y = -250.0;

    let path = std::env::temp_dir().join("snipsy_export_reel.mp4");
    let settings = ExportSettings { path: path.clone(), codec: VideoCodec::H264, quality: Quality::Medium, hardware: true };
    export_blocking(&tl, &settings).unwrap();

    let out = MediaInfo::probe(&path).unwrap();
    assert_eq!((out.width, out.height), (360, 640));
    assert!(out.has_audio);
    assert!((out.duration - 2.0).abs() < 0.1, "duración {}", out.duration);
    assert!((out.fps - 30.0).abs() < 0.01);

    let mut r = VideoReader::open(&path, (360, 640)).unwrap();
    let img = r.frame_at(0.5).unwrap().unwrap();
    let px = |x: usize, y: usize| {
        let i = (y * img.width + x) * 4;
        [img.rgba[i] as i32, img.rgba[i + 1] as i32, img.rgba[i + 2] as i32]
    };
    // El video cubre todo el lienzo (sin barras negras), con su color azul.
    for (x, y) in [(5, 320), (354, 600), (180, 500)] {
        let c = px(x, y);
        assert!((c[0] - 0x20).abs() < 12 && (c[1] - 0x60).abs() < 12 && (c[2] - 0xE0).abs() < 12, "({x},{y}) = {c:?}");
    }
    // Hay texto blanco en la franja superior (y ≈ 70).
    let white = (100..260).filter(|&x| px(x, 70).iter().all(|&v| v > 200)).count();
    assert!(white > 10, "texto no encontrado ({white} px blancos)");
    // En el segundo 1.5 el texto ya no está.
    let img2 = r.frame_at(1.5).unwrap().unwrap();
    let i = (70 * img2.width + 180) * 4;
    assert!(img2.rgba[i] < 100, "el texto debería haber terminado");
}

/// Medición: `cargo test --release -p snipsy-export -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_reel_1080x1920_10s() {
    let path = std::env::temp_dir().join("snipsy_bench_src.mp4");
    if !path.exists() {
        Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error",
                "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=10",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=10",
                "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
            .arg(&path)
            .status()
            .unwrap();
    }
    let info = MediaInfo::probe(&path).unwrap();
    let mut tl = Timeline::new();
    let a = tl.add_asset(Asset {
        id: AssetId(0),
        path,
        name: "bench".into(),
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
    let ids = tl.append_asset(a);
    let cover = snipsy_core::compose::cover_scale((1920.0, 1080.0), (1080.0, 1920.0));
    tl.clip_mut(ids[0]).unwrap().transform.scale = cover;
    tl.insert_text(0, 1, 300, TextStyle::default());
    for hardware in [true, false] {
        let out = std::env::temp_dir().join(format!("snipsy_bench_{hardware}.mp4"));
        let t = std::time::Instant::now();
        export_blocking(&tl, &ExportSettings { path: out.clone(), codec: VideoCodec::H264, quality: Quality::High, hardware }).unwrap();
        let secs = t.elapsed().as_secs_f64();
        let size = std::fs::metadata(&out).unwrap().len() as f64 / 1e6;
        eprintln!("hardware={hardware}: 10 s de video en {secs:.2} s ({:.1}x tiempo real), {size:.1} MB", 10.0 / secs);
    }
}

#[test]
fn exports_speed_change_keeping_pitch() {
    let path = std::env::temp_dir().join("snipsy_fx_rgb.mp4");
    if !path.exists() {
        let ok = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-filter_complex",
                "color=c=red:s=320x180:r=30:d=1[a];color=c=lime:s=320x180:r=30:d=1[b];color=c=blue:s=320x180:r=30:d=1[c];[a][b][c]concat=n=3[v];sine=frequency=440:duration=3[s]",
                "-map", "[v]", "-map", "[s]", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
    }
    if snipsy_render::headless_device().is_none() {
        return;
    }
    let mut tl = Timeline::new();
    let a = add(&mut tl, &path);
    let ids = tl.append_asset(a);
    assert!(tl.set_speed(ids[0], 2.0));
    assert_eq!(tl.duration(), 45);

    let out = std::env::temp_dir().join("snipsy_export_speed.mp4");
    export_blocking(&tl, &ExportSettings { path: out.clone(), codec: VideoCodec::H264, quality: Quality::High, hardware: true }).unwrap();
    let info = MediaInfo::probe(&out).unwrap();
    assert!((info.duration - 1.5).abs() < 0.1, "duración {}", info.duration);

    let mut r = VideoReader::open(&out, (320, 180)).unwrap();
    let mut at = |frame: i64| {
        let img = r.frame_at(frame as f64 / 30.0).unwrap().unwrap();
        let i = (90 * img.width + 160) * 4;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };
    let (red, green, blue) = (at(5), at(20), at(38));
    assert!(red[0] > 200 && red[1] < 50, "rojo: {red:?}");
    assert!(green[1] > 200 && green[0] < 50, "verde: {green:?}");
    assert!(blue[2] > 200 && blue[1] < 50, "azul: {blue:?}");

    // El audio dura lo mismo que el video y conserva el tono de 440 Hz.
    let mut ar = AudioReader::open(&out, 48_000, 2).unwrap();
    let mut buf = vec![0.0f32; 24_000 * 2];
    ar.mix_into(0.4, &mut buf, 1.0).unwrap();
    let left: Vec<f32> = buf.iter().step_by(2).copied().collect();
    let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    let hz = crossings as f64 / 2.0 / 0.5;
    assert!((hz - 440.0).abs() < 15.0, "tono: {hz} Hz");
    assert!(rms(&out, 1.2) > 0.03, "hay audio hasta el final");
}

#[test]
fn exports_brand_logo_and_call_to_action() {
    let Some(red) = color_clip("red", "0xE02020") else { return };
    if snipsy_render::headless_device().is_none() {
        eprintln!("sin GPU; se omite");
        return;
    }
    use snipsy_core::{BrandKit, Corner, TextPreset};
    // Logo verde de 100×50.
    let logo = std::env::temp_dir().join("snipsy_brand_logo.png");
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=0x00FF00:size=100x50", "-frames:v", "1"])
        .arg(&logo)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok);

    let mut tl = Timeline::new();
    let a = add(&mut tl, &red);
    tl.width = 640;
    tl.height = 360;
    tl.append_asset(a);
    let font = std::path::Path::new("/System/Library/Fonts/Supplemental/Courier New.ttf");
    let kit = BrandKit {
        logo_corner: Corner::TopRight,
        logo_size: 0.2,
        margin: 0.05,
        font: font.exists().then(|| "Courier New".to_owned()),
        font_file: font.exists().then(|| font.to_owned()),
        accent: [255, 214, 10, 255],
        ..Default::default()
    };
    kit.insert_preset(TextPreset::CallToAction, &mut tl, 30);
    let img = snipsy_render::load_image(&logo).unwrap();
    let logo_asset = tl.add_asset(Asset {
        id: AssetId(0),
        path: logo.clone(),
        name: "logo".into(),
        duration: 5.0,
        fps: 30.0,
        width: img.width as u32,
        height: img.height as u32,
        has_video: true,
        has_audio: false,
        still: true,
        faces: None,
    });
    kit.place_logo(&mut tl, logo_asset).unwrap();
    assert_eq!(tl.duration(), 120);

    let path = std::env::temp_dir().join("snipsy_export_brand.mp4");
    let settings = ExportSettings { path: path.clone(), codec: VideoCodec::H264, quality: Quality::High, hardware: true };
    export_blocking(&tl, &settings).unwrap();

    let mut r = VideoReader::open(&path, (640, 360)).unwrap();
    let mut frame = |n: i64| r.frame_at(n as f64 / 30.0).unwrap().unwrap();
    let px = |img: &snipsy_media::Image, x: usize, y: usize| {
        let i = (y * img.width + x) * 4;
        [img.rgba[i] as i32, img.rgba[i + 1] as i32, img.rgba[i + 2] as i32]
    };
    let green = |c: [i32; 3]| c[1] > 200 && c[0] < 60 && c[2] < 60;
    let yellow = |c: [i32; 3]| c[0] > 220 && c[1] > 180 && c[2] < 80;
    // Logo: 72×36 px, a 18 px de los bordes superior y derecho: x 550–622, y 18–54.
    for n in [5, 60, 115] {
        let f = frame(n);
        for (x, y) in [(554, 22), (586, 36), (618, 50)] {
            assert!(green(px(&f, x, y)), "logo en el cuadro {n} ({x}, {y}): {:?}", px(&f, x, y));
        }
        for (x, y) in [(586, 60), (630, 36), (544, 36), (586, 12)] {
            assert!(!green(px(&f, x, y)), "fuera del logo ({x}, {y})");
        }
    }
    // Llamada a la acción (fondo de acento) en el tercio inferior desde el cuadro 30.
    let count = |f: &snipsy_media::Image| {
        let mut n = 0;
        for y in 230..310 {
            for x in 150..490 {
                n += usize::from(yellow(px(f, x, y)));
            }
        }
        n
    };
    let before = frame(20);
    assert_eq!(count(&before), 0, "aún no aparece");
    let after = frame(70);
    let n = count(&after);
    assert!(n > 3000, "llamada a la acción visible: {n} px amarillos");
    let center = px(&after, 320, 180);
    assert!(center[0] > 180 && center[1] < 60, "el video sigue detrás: {center:?}");
}

fn audio_file(name: &str, expr: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("snipsy_duck_{name}.m4a"));
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg(format!("aevalsrc={expr}:s=48000:d=5"))
        .args(["-c:a", "aac", "-b:a", "192k"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok);
    path
}

/// Amplitud de la frecuencia `f` en un tramo (Goertzel), para separar música y voz.
fn tone(path: &PathBuf, t: f64, f: f64) -> f64 {
    let mut r = AudioReader::open(path, 48_000, 1).unwrap();
    let mut buf = vec![0.0f32; 4800];
    r.mix_into(t, &mut buf, 1.0).unwrap();
    let w = 2.0 * std::f64::consts::PI * f / 48_000.0;
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for x in &buf {
        let s0 = *x as f64 + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2).sqrt() * 2.0 / buf.len() as f64
}

#[test]
fn ducks_music_under_voice() {
    use snipsy_core::{ducking, DuckSettings, TrackRef};
    if snipsy_render::headless_device().is_none() {
        eprintln!("sin GPU; se omite");
        return;
    }
    let voice = audio_file("voice", "0.5*sin(2*PI*440*t)*(between(t\\,1\\,2)+between(t\\,3\\,4))");
    let music = audio_file("music", "0.3*sin(2*PI*880*t)");
    let mut tl = Timeline::new();
    let add_audio = |tl: &mut Timeline, path: &PathBuf| {
        let info = MediaInfo::probe(path).unwrap();
        tl.add_asset(Asset {
            id: AssetId(0),
            path: path.clone(),
            name: "a".into(),
            duration: info.duration,
            fps: 30.0,
            width: 0,
            height: 0,
            has_video: false,
            has_audio: true,
            still: false,
            faces: None,
        })
    };
    let (v, m) = (add_audio(&mut tl, &voice), add_audio(&mut tl, &music));
    tl.insert_asset(v, 0, 0, 0);
    tl.insert_asset(m, 0, 0, 1);
    let track = TrackRef::audio(1);

    let s = DuckSettings::default();
    let levels = snipsy_media::loudness::levels_without(&tl, track);
    let regions = ducking::voice_regions(&levels, tl.fps, &s);
    // Dos frases (la pausa de 1 s es más larga que `hold`), con un cuadro de margen por el códec.
    assert_eq!(regions.len(), 2, "{regions:?}");
    for ((a, b), (ea, eb)) in regions.iter().zip([(30, 60), (90, 120)]) {
        assert!((a - ea).abs() <= 1 && (b - eb).abs() <= 1, "{regions:?}");
    }
    assert_eq!(ducking::apply_ducking(&mut tl, track, &regions, &s), 1);

    let path = std::env::temp_dir().join("snipsy_export_duck.mp4");
    let settings = ExportSettings { path: path.clone(), codec: VideoCodec::H264, quality: Quality::High, hardware: true };
    export_blocking(&tl, &settings).unwrap();

    let full = tone(&path, 0.4, 880.0);
    let ducked = tone(&path, 1.4, 880.0);
    // Entre el fin de la subida (2,6 s) y el inicio de la próxima bajada (2,75 s).
    let between = tone(&path, 2.62, 880.0);
    let ducked2 = tone(&path, 3.4, 880.0);
    eprintln!("música: {full:.3} → con voz {ducked:.3} → pausa {between:.3} → con voz {ducked2:.3}");
    assert!((full - 0.3).abs() < 0.03, "música a volumen normal: {full}");
    // −12 dB ≈ ×0,25.
    for d in [ducked, ducked2] {
        assert!((d / full - 0.251).abs() < 0.04, "música bajo la voz: {d} / {full}");
    }
    assert!((between / full - 1.0).abs() < 0.08, "vuelve a subir en la pausa: {between}");
    let v = tone(&path, 1.4, 440.0);
    assert!((v - 0.5).abs() < 0.05, "la voz no se toca: {v}");
}

#[test]
fn detects_real_speech_for_ducking() {
    use snipsy_core::{ducking, DuckSettings, TrackRef};
    let aiff = std::env::temp_dir().join("snipsy_duck_speech.aiff");
    let ok = Command::new("say")
        .args(["-v", "Paulina", "-o"])
        .arg(&aiff)
        .arg("Hola a todos, bienvenidos a nuestra tienda. [[slnc 2000]] Hoy tenemos ofertas increíbles para ti.")
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("sin voz del sistema; se omite");
        return;
    }
    let info = MediaInfo::probe(&aiff).unwrap();
    let mut tl = Timeline::new();
    let a = tl.add_asset(Asset {
        id: AssetId(0),
        path: aiff.clone(),
        name: "voz".into(),
        duration: info.duration,
        fps: 30.0,
        width: 0,
        height: 0,
        has_video: false,
        has_audio: true,
        still: false,
        faces: None,
    });
    tl.insert_asset(a, 30, 0, 0);
    let levels = snipsy_media::loudness::levels_without(&tl, TrackRef::audio(1));
    let regions = ducking::voice_regions(&levels, tl.fps, &DuckSettings::default());
    eprintln!("{:.2} s de audio, tramos: {regions:?}", info.duration);
    // Dos frases separadas por 2 s de silencio; las pausas entre palabras no cortan el tramo.
    assert_eq!(regions.len(), 2, "{regions:?}");
    assert!((regions[0].0 - 30).abs() <= 6, "empieza con la voz: {regions:?}");
    let gap = regions[1].0 - regions[0].1;
    assert!((50..=70).contains(&gap), "silencio de ~2 s: {gap} cuadros");
}

#[test]
fn exports_reframed_formats() {
    let Some(red) = color_clip("red", "0xE02020") else { return };
    if snipsy_render::headless_device().is_none() {
        eprintln!("sin GPU; se omite");
        return;
    }
    use snipsy_core::{BrandKit, Corner};
    let logo = std::env::temp_dir().join("snipsy_brand_logo.png");
    assert!(Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=0x00FF00:size=100x50", "-frames:v", "1"])
        .arg(&logo)
        .status()
        .is_ok_and(|s| s.success()));
    // Anuncio 16:9 (640×360): video a pantalla completa, logo arriba a la derecha y un título blanco.
    let mut tl = Timeline::new();
    let a = add(&mut tl, &red);
    tl.width = 640;
    tl.height = 360;
    tl.append_asset(a);
    tl.insert_text(0, 1, 90, TextStyle { text: "OFERTA".into(), size: 60.0, outline: None, ..Default::default() });
    let l = tl.add_asset(Asset {
        id: AssetId(0),
        path: logo.clone(),
        name: "logo".into(),
        duration: 5.0,
        fps: 30.0,
        width: 100,
        height: 50,
        has_video: true,
        has_audio: false,
        still: true,
        faces: None,
    });
    BrandKit { logo_corner: Corner::TopRight, logo_size: 0.2, margin: 0.05, ..Default::default() }.place_logo(&mut tl, l).unwrap();

    for (w, h) in [(360u32, 640u32), (360, 360)] {
        let v = snipsy_core::reframe::reframe(&tl, w, h, false);
        let path = std::env::temp_dir().join(format!("snipsy_reframe_{w}x{h}.mp4"));
        let settings = ExportSettings { path: path.clone(), codec: VideoCodec::H264, quality: Quality::High, hardware: true };
        export_blocking(&v, &settings).unwrap();
        let info = MediaInfo::probe(&path).unwrap();
        assert_eq!((info.width, info.height), (w, h));
        let mut r = VideoReader::open(&path, (w, h)).unwrap();
        let img = r.frame_at(0.5).unwrap().unwrap();
        let px = |x: u32, y: u32| {
            let i = (y as usize * img.width + x as usize) * 4;
            [img.rgba[i] as i32, img.rgba[i + 1] as i32, img.rgba[i + 2] as i32]
        };
        let red_px = |c: [i32; 3]| c[0] > 180 && c[1] < 70 && c[2] < 70;
        let green = |c: [i32; 3]| c[1] > 200 && c[0] < 60;
        let white = |c: [i32; 3]| c.iter().all(|v| *v > 200);
        // El video llena todo el cuadro (sin franjas negras arriba ni abajo).
        for (x, y) in [(w / 2, 4), (w / 2, h - 4), (4, h / 2), (w - 4, h / 2)] {
            assert!(red_px(px(x, y)) || green(px(x, y)), "{w}x{h}: borde ({x}, {y}) = {:?}", px(x, y));
        }
        // Logo: 72×36 (20 % de 360) a 18 px de arriba y de la derecha.
        assert!(green(px(w - 18 - 36, 18 + 18)), "{w}x{h}: logo {:?}", px(w - 54, 36));
        assert!(!green(px(w - 10, 36)) && !green(px(w - 54, 60)), "{w}x{h}: tamaño del logo");
        // El título sigue en el centro.
        let whites = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| white(px(x, y))).collect::<Vec<_>>();
        assert!(whites.len() > 200, "{w}x{h}: texto visible");
        let cx = whites.iter().map(|p| p.0 as f64).sum::<f64>() / whites.len() as f64;
        let cy = whites.iter().map(|p| p.1 as f64).sum::<f64>() / whites.len() as f64;
        assert!((cx - w as f64 / 2.0).abs() < 8.0 && (cy - h as f64 / 2.0).abs() < 8.0, "{w}x{h}: centro del texto ({cx:.0}, {cy:.0})");
    }
}
