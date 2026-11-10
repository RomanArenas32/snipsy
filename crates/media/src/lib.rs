//! Lectura de medios con FFmpeg y motores de reproducción del timeline.
//!
//! - `VideoReader` / `AudioReader`: lectores síncronos de un archivo con seek preciso.
//! - `VideoEngine` / `AudioEngine`: hilos que recorren el timeline y producen
//!   cuadros (a la tasa del proyecto) y audio mezclado.

mod audio_engine;
mod audio_reader;
pub mod loudness;
mod probe;
pub mod proxy;
pub mod segments;
mod tempo;
mod video_engine;
mod video_reader;
mod writer;

pub use audio_engine::{AudioEngine, AudioSegment, AudioTimeline, Envelope, Mixer};
pub use audio_reader::AudioReader;
pub use tempo::Tempo;
pub use probe::MediaInfo;
pub use proxy::{ProxyEvent, ProxyQueue, ProxyRequest};
pub use video_engine::{VideoEngine, VideoEvent, VideoFrame, VideoSegment, VideoTimeline};
pub use video_reader::{Image, VideoReader};
pub use writer::{Mp4Writer, VideoCodec, WriterSettings};

use std::sync::{Arc, Once};

pub use ffmpeg_next as ffmpeg;

/// Callback para avisar a la interfaz que hay datos nuevos (p. ej. `ctx.request_repaint()`).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// Inicializa FFmpeg (idempotente).
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        ffmpeg::init().expect("no se pudo inicializar FFmpeg");
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Error);
    });
}

/// Tiempo de inicio del contenedor en segundos (algunos archivos no empiezan en 0).
pub(crate) fn container_start(ictx: &ffmpeg::format::context::Input) -> f64 {
    let start = unsafe { (*ictx.as_ptr()).start_time };
    if start == ffmpeg::ffi::AV_NOPTS_VALUE {
        0.0
    } else {
        start as f64 / ffmpeg::ffi::AV_TIME_BASE as f64
    }
}

/// Rotación de visualización declarada en el stream (grados en sentido horario:
/// 0, 90, 180 o 270). Los celulares graban en horizontal y marcan así el video vertical.
pub(crate) fn stream_rotation(stream: &ffmpeg::format::stream::Stream) -> u32 {
    use ffmpeg::ffi;
    unsafe {
        let par = (*stream.as_ptr()).codecpar;
        if par.is_null() {
            return 0;
        }
        let sd = ffi::av_packet_side_data_get(
            (*par).coded_side_data,
            (*par).nb_coded_side_data,
            ffi::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
        );
        if sd.is_null() || (*sd).data.is_null() {
            return 0;
        }
        // FFmpeg devuelve el ángulo antihorario; para mostrarlo hay que girar en sentido contrario.
        let ccw = ffi::av_display_rotation_get((*sd).data as *const i32);
        if !ccw.is_finite() {
            return 0;
        }
        (((-ccw).rem_euclid(360.0) / 90.0).round() as u32 % 4) * 90
    }
}

/// Ajusta las matrices de color de un escalador (BT.601/709, rango limitado/completo).
/// Por defecto swscale asume BT.601, lo que desplaza los colores de video HD (BT.709).
pub(crate) fn set_colorspace(
    ctx: &mut ffmpeg::software::scaling::Context,
    src_709: bool,
    src_full: bool,
    dst_709: bool,
    dst_full: bool,
) {
    use ffmpeg::ffi;
    let coeffs = |bt709: bool| unsafe { ffi::sws_getCoefficients(if bt709 { ffi::SWS_CS_ITU709 } else { ffi::SWS_CS_DEFAULT }) };
    unsafe {
        ffi::sws_setColorspaceDetails(
            ctx.as_mut_ptr(),
            coeffs(src_709),
            src_full as i32,
            coeffs(dst_709),
            dst_full as i32,
            0,
            1 << 16,
            1 << 16,
        );
    }
}

/// Seek al keyframe anterior a `target` (segundos relativos al inicio).
pub(crate) fn seek_input(ictx: &mut ffmpeg::format::context::Input, start: f64, target: f64) {
    let ts = ((target.max(0.0) + start) * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
    if let Err(e) = ictx.seek(ts, ..ts) {
        log::warn!("seek a {target:.3}s falló: {e}");
    }
}
