//! Pruebas de integración: generan un video con FFmpeg y verifican lectura y motores.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use snipsy_audio::AudioQueue;
use snipsy_media::*;

fn sample_video() -> Option<PathBuf> {
    let path = std::env::temp_dir().join("snipsy_test_clip.mp4");
    if path.exists() {
        return Some(path);
    }
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error",
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=4",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=4",
            "-c:v", "libx264", "-g", "60", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&path)
        .status()
        .ok()?;
    status.success().then_some(path)
}

#[test]
fn rotated_phone_video_is_shown_upright() {
    let Some(src) = sample_video() else { return };
    let path = std::env::temp_dir().join("snipsy_test_rotated.mp4");
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-display_rotation:v:0", "90", "-i"])
        .arg(&src)
        .args(["-c", "copy"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("ffmpeg no soporta -display_rotation; se omite");
        return;
    }
    let info = MediaInfo::probe(&path).unwrap();
    assert_eq!(info.rotation, 270);
    assert_eq!((info.width, info.height), (720, 1280));
    let mut r = VideoReader::open(&path, (640, 640)).unwrap();
    let img = r.frame_at(0.0).unwrap().unwrap();
    assert_eq!((img.width, img.height), (360, 640));
}

#[test]
fn probe_reports_stream_info() {
    let Some(path) = sample_video() else { return };
    let info = MediaInfo::probe(&path).unwrap();
    assert!(info.has_video && info.has_audio);
    assert_eq!((info.width, info.height), (1280, 720));
    assert!((info.fps - 30.0).abs() < 0.01);
    assert!((info.duration - 4.0).abs() < 0.1, "duración {}", info.duration);
}

#[test]
fn video_reader_is_frame_accurate_forward_and_backward() {
    let Some(path) = sample_video() else { return };
    let mut r = VideoReader::open(&path, (640, 640)).unwrap();
    let a = r.frame_at(0.0).unwrap().unwrap();
    assert_eq!((a.width, a.height), (640, 360));
    // El mismo instante devuelve la misma imagen (cache), sin reconvertir.
    let a2 = r.frame_at(0.01).unwrap().unwrap();
    assert!(Arc::ptr_eq(&a.rgba, &a2.rgba));
    // Cuadros distintos dan imágenes distintas; ir atrás y volver da la misma.
    let f75 = r.frame_at(2.5).unwrap().unwrap();
    let f27 = r.frame_at(0.9).unwrap().unwrap();
    assert_ne!(f75.rgba, f27.rgba);
    let f75b = r.frame_at(2.5).unwrap().unwrap();
    assert_eq!(f75.rgba, f75b.rgba);
}

fn collect(engine: &VideoEngine, serial: u64, n: usize) -> Vec<VideoFrame> {
    let mut out = Vec::new();
    while out.len() < n {
        match engine.events().recv_timeout(Duration::from_secs(5)).expect("timeout") {
            VideoEvent::Frame(f) if f.serial == serial => out.push(f),
            VideoEvent::Frame(_) => {}
            VideoEvent::EndOfStream { serial: s } if s == serial => break,
            VideoEvent::EndOfStream { .. } => {}
            VideoEvent::Error(e) => panic!("{e}"),
        }
    }
    out
}

#[test]
fn video_engine_walks_timeline_with_gap() {
    let Some(path) = sample_video() else { return };
    let path = Arc::new(path);
    // Clip 1: cuadros 0-9 (fuente desde 1 s). Hueco 10-14. Clip 2: 15-19 (fuente desde 1 s).
    let tl = VideoTimeline {
        fps: 30.0,
        duration: 20,
        segments: vec![
            VideoSegment { clip: 1, path: path.clone(), layer: 0, start: 0, end: 10, source_in: 1.0, speed: 1.0 },
            VideoSegment { clip: 2, path: path.clone(), layer: 0, start: 15, end: 20, source_in: 1.0, speed: 1.0 },
        ],
    };
    let engine = VideoEngine::new((320, 320), Arc::new(|| {}));
    engine.update(1, 0, Some(Arc::new(tl)));
    let frames = collect(&engine, 1, 25);
    assert_eq!(frames.len(), 20, "termina al final del timeline");
    assert_eq!(frames.iter().map(|f| f.frame).collect::<Vec<_>>(), (0..20).collect::<Vec<_>>());
    assert!(frames[..10].iter().all(|f| f.layers.len() == 1 && f.layers[0].0 == 1));
    assert!(frames[10..15].iter().all(|f| f.layers.is_empty()), "hueco = negro");
    // Ambos clips empiezan en la misma posición de la fuente → misma imagen.
    assert_eq!(frames[0].layers[0].1.rgba, frames[15].layers[0].1.rgba);

    // Seek a mitad del primer clip.
    engine.update(2, 5, None);
    let frames = collect(&engine, 2, 1);
    assert_eq!(frames[0].frame, 5);
}

#[test]
fn video_engine_returns_all_visible_layers_bottom_to_top() {
    let Some(path) = sample_video() else { return };
    let path = Arc::new(path);
    let tl = VideoTimeline {
        fps: 30.0,
        duration: 10,
        segments: vec![
            VideoSegment { clip: 7, path: path.clone(), layer: 1, start: 0, end: 10, source_in: 2.0, speed: 1.0 },
            VideoSegment { clip: 3, path: path.clone(), layer: 0, start: 0, end: 10, source_in: 0.0, speed: 1.0 },
        ],
    };
    let engine = VideoEngine::new((320, 320), Arc::new(|| {}));
    engine.update(1, 0, Some(Arc::new(tl)));
    let f = collect(&engine, 1, 1).remove(0);
    assert_eq!(f.layers.iter().map(|l| l.0).collect::<Vec<_>>(), vec![3, 7]);
}

#[test]
fn audio_engine_mixes_and_fills_gaps_with_silence() {
    let Some(path) = sample_video() else { return };
    let queue = Arc::new(AudioQueue::new(48_000, 2));
    let engine = AudioEngine::new(queue.clone());
    let tl = AudioTimeline {
        duration: 2.0,
        segments: vec![AudioSegment { clip: 1, path: Arc::new(path), start: 0.5, end: 2.0, source_in: 0.0, speed: 1.0, gain: 1.0, envelope: None }],
    };
    queue.reset(1, 0.0);
    engine.update(1, 0.0, Some(Arc::new(tl)));
    let mut waited = 0;
    while queue.buffered_secs() < 0.45 && waited < 300 {
        std::thread::sleep(Duration::from_millis(10));
        waited += 1;
    }
    assert!(queue.buffered_secs() >= 0.45, "buffer {}", queue.buffered_secs());
}

#[test]
fn audio_reader_produces_signal_after_seek() {
    let Some(path) = sample_video() else { return };
    let mut r = AudioReader::open(&path, 48_000, 2).unwrap();
    let mut buf = vec![0.0f32; 4800 * 2];
    r.mix_into(2.0, &mut buf, 1.0).unwrap();
    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "el seno de 440 Hz debe sonar (pico {peak})");
}

fn write_clip(path: &std::path::Path, codec: VideoCodec, hardware: bool) -> String {
    let (w, h) = (320u32, 240u32);
    let mut writer = Mp4Writer::create(
        path,
        &WriterSettings {
            width: w,
            height: h,
            fps: 30.0,
            codec,
            bitrate: 4_000_000,
            hardware,
            keyframe_secs: 2.0,
            audio: Some((48_000, 2)),
            audio_bitrate: 192_000,
        },
    )
    .unwrap();
    let name = writer.encoder_name().to_owned();
    // Mitad izquierda naranja, mitad derecha azul.
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let c = if x < w / 2 { [230, 120, 30, 255] } else { [40, 90, 200, 255] };
            frame[i..i + 4].copy_from_slice(&c);
        }
    }
    let mut t = 0usize;
    for _ in 0..30 {
        writer.write_video(&frame).unwrap();
        // 1600 muestras por cuadro a 48 kHz / 30 fps.
        let audio: Vec<f32> = (0..1600)
            .flat_map(|_| {
                let v = (t as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                t += 1;
                [v, v]
            })
            .collect();
        writer.write_audio(&audio).unwrap();
    }
    writer.finish().unwrap();
    name
}

fn assert_roundtrip(path: &std::path::Path) {
    let info = MediaInfo::probe(path).unwrap();
    assert!(info.has_video && info.has_audio);
    assert_eq!((info.width, info.height), (320, 240));
    assert!((info.duration - 1.0).abs() < 0.1, "duración {}", info.duration);
    let mut r = VideoReader::open(path, (320, 240)).unwrap();
    let img = r.frame_at(0.5).unwrap().unwrap();
    let px = |x: usize, y: usize| {
        let i = (y * img.width + x) * 4;
        [img.rgba[i] as i32, img.rgba[i + 1] as i32, img.rgba[i + 2] as i32]
    };
    for (got, want) in [(px(60, 120), [230, 120, 30]), (px(260, 120), [40, 90, 200])] {
        let err = got.iter().zip(want).map(|(a, b)| (a - b).abs()).max().unwrap();
        assert!(err <= 8, "color {got:?} vs {want:?}");
    }
    let mut a = AudioReader::open(path, 48_000, 2).unwrap();
    let mut buf = vec![0.0f32; 9600];
    a.mix_into(0.3, &mut buf, 1.0).unwrap();
    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!((0.4..0.6).contains(&peak), "pico de audio {peak}");
}

#[test]
fn writer_roundtrip_software_h264() {
    let path = std::env::temp_dir().join("snipsy_writer_sw.mp4");
    let name = write_clip(&path, VideoCodec::H264, false);
    assert_eq!(name, "libx264");
    assert_roundtrip(&path);
}

#[test]
fn writer_roundtrip_hardware_hevc() {
    let path = std::env::temp_dir().join("snipsy_writer_hw.mp4");
    let name = write_clip(&path, VideoCodec::Hevc, true);
    eprintln!("codificador usado: {name}");
    assert_roundtrip(&path);
}

/// Video 4K de 2 s: rojo el primer segundo, azul el segundo.
fn clip_4k() -> Option<std::path::PathBuf> {
    let path = std::env::temp_dir().join("snipsy_4k_src.mp4");
    if path.exists() {
        return Some(path);
    }
    let ok = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=0xE02020:size=3840x2160:rate=30:duration=1",
            "-f", "lavfi", "-i", "color=c=0x2040E0:size=3840x2160:rate=30:duration=1",
            "-filter_complex", "[0:v][1:v]concat=n=2:v=1[v]", "-map", "[v]",
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(path)
}

#[test]
fn proxy_keeps_timing_at_lower_resolution() {
    use snipsy_media::proxy;
    let Some(src) = clip_4k() else { return };
    let info = MediaInfo::probe(&src).unwrap();
    assert!(proxy::needs_proxy(info.width, info.height));
    let dir = std::env::temp_dir().join(format!("snipsy_proxies_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    // A través de la cola, como en la app.
    let queue = snipsy_media::ProxyQueue::new(dir.clone(), std::sync::Arc::new(|| {}));
    queue.request(snipsy_media::ProxyRequest {
        asset: 7,
        path: src.clone(),
        width: info.width,
        height: info.height,
        fps: info.fps,
        duration: info.duration,
    });
    let mut progressed = false;
    let proxy = loop {
        match queue.events().recv_timeout(std::time::Duration::from_secs(120)).expect("el proxy no terminó") {
            snipsy_media::ProxyEvent::Progress { asset, .. } => {
                assert_eq!(asset, 7);
                progressed = true;
            }
            snipsy_media::ProxyEvent::Done { asset: 7, proxy, secs } => {
                eprintln!("proxy en {secs:.2} s");
                break proxy;
            }
            other => panic!("{other:?}"),
        }
    };
    assert!(progressed);
    assert_eq!(proxy::existing_proxy_in(&dir, &src), Some(proxy.clone()));
    assert!(!dir.read_dir().unwrap().any(|e| e.unwrap().path().to_string_lossy().contains(".part")), "sin temporales");

    let p = MediaInfo::probe(&proxy).unwrap();
    assert_eq!((p.width, p.height), (1280, 720));
    assert!((p.duration - info.duration).abs() < 0.05, "misma duración: {} vs {}", p.duration, info.duration);
    let size = std::fs::metadata(&proxy).unwrap().len();
    assert!(size < std::fs::metadata(&src).unwrap().len() * 4, "liviano: {size} bytes");

    // Mismo contenido en el mismo instante (el corte rojo → azul en 1,0 s).
    let mut r = VideoReader::open(&proxy, (1280, 720)).unwrap();
    let mut at = |t: f64| {
        let img = r.frame_at(t).unwrap().unwrap();
        let i = (360 * img.width + 640) * 4;
        [img.rgba[i] as i32, img.rgba[i + 1] as i32, img.rgba[i + 2] as i32]
    };
    let red = |c: [i32; 3]| c[0] > 180 && c[2] < 80;
    let blue = |c: [i32; 3]| c[2] > 180 && c[0] < 80;
    assert!(red(at(0.0)) && red(at(0.95)), "{:?}", at(0.95));
    assert!(blue(at(1.0)) && blue(at(1.9)), "{:?}", at(1.0));
    // Saltar hacia atrás también cae en el cuadro justo.
    assert!(red(at(0.5)));

    // La cola reconoce el proxy existente sin volver a crearlo.
    queue.request(snipsy_media::ProxyRequest { asset: 8, path: src.clone(), width: info.width, height: info.height, fps: info.fps, duration: info.duration });
    match queue.events().recv_timeout(std::time::Duration::from_secs(10)).unwrap() {
        snipsy_media::ProxyEvent::Done { asset: 8, secs, .. } => assert_eq!(secs, 0.0),
        other => panic!("{other:?}"),
    }
    drop(queue);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Compara la vista previa leyendo un 4K real (con detalle y movimiento) contra su proxy.
/// `cargo test -p snipsy-media --release --test decode bench_proxy -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_proxy_vs_original() {
    use std::time::Instant;
    let src = std::env::temp_dir().join("snipsy_4k_bench.mp4");
    if !src.exists() {
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=3840x2160:rate=30:duration=6",
                "-c:v", "libx264", "-preset", "veryfast", "-b:v", "45M", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok);
    }
    let info = MediaInfo::probe(&src).unwrap();
    let dir = std::env::temp_dir().join("snipsy_bench_proxies");
    let dest = snipsy_media::proxy::proxy_path_in(&dir, &src).unwrap();
    if !dest.exists() {
        let t = Instant::now();
        let size = snipsy_media::proxy::proxy_size(info.width, info.height);
        snipsy_media::proxy::make_proxy(&src, &dest, size, info.fps, info.duration, &Default::default(), |_| {}).unwrap();
        eprintln!("crear proxy de {:.0} s de 4K: {:.1} s", info.duration, t.elapsed().as_secs_f64());
    }
    for (name, path) in [("original 4K", &src), ("proxy 720p", &dest)] {
        let mut r = VideoReader::open(path, (1920, 1080)).unwrap();
        let t = Instant::now();
        for n in 0..150 {
            r.frame_at(n as f64 / 30.0).unwrap().unwrap();
        }
        let fps = 150.0 / t.elapsed().as_secs_f64();
        // Saltos al azar (como al mover el cabezal).
        let t = Instant::now();
        for i in 0..20 {
            let at = ((i * 7919) % 170) as f64 / 30.0;
            r.frame_at(at).unwrap().unwrap();
        }
        let seek_ms = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
        eprintln!("{name}: reproducción {fps:.0} cuadros/s · salto promedio {seek_ms:.0} ms");
    }
}
