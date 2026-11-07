//! Transcripción con Whisper (whisper.cpp, acelerado con Metal) con tiempos por palabra.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use whisper_rs::{DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::{Model, Word};

const LEAD: f64 = 0.15;

/// Frecuencia que espera Whisper.
pub const SAMPLE_RATE: u32 = 16_000;

/// Transcribe audio mono a 16 kHz. `language`: código ISO ("es", "en"…) o `None` para detectarlo.
/// `progress` recibe el porcentaje (0–100).
pub fn transcribe(
    model: &Path,
    kind: Model,
    audio: &[f32],
    language: Option<&str>,
    cancel: Arc<AtomicBool>,
    progress: impl FnMut(i32) + 'static,
) -> Result<Vec<Word>> {
    whisper_rs::install_logging_hooks();
    let mut cp = WhisperContextParameters::default();
    cp.use_gpu = true;
    // Alineación DTW: tiempos por palabra mucho más precisos que los de los tokens
    // (es incompatible con flash attention).
    cp.flash_attn = false;
    let preset = match kind {
        Model::Base => DtwModelPreset::Base,
        Model::Small => DtwModelPreset::Small,
        Model::LargeTurbo => DtwModelPreset::LargeV3Turbo,
    };
    cp.dtw_parameters = DtwParameters { mode: DtwMode::ModelPreset { model_preset: preset }, ..Default::default() };
    let ctx = WhisperContext::new_with_params(model, cp).map_err(|e| anyhow!("no se pudo cargar el modelo: {e}"))?;
    let mut state = ctx.create_state().map_err(|e| anyhow!("{e}"))?;

    let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 });
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    params.set_n_threads(threads as i32);
    params.set_language(Some(language.unwrap_or("auto")));
    params.set_translate(false);
    params.set_token_timestamps(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_progress_callback_safe(progress);
    // Cancelación. (No se usa `set_abort_callback_safe` de whisper-rs 0.16: su
    // trampolín interpreta mal el puntero y aborta al azar.)
    unsafe extern "C" fn should_abort(user_data: *mut std::ffi::c_void) -> bool {
        // SAFETY: `user_data` apunta al `AtomicBool` de `cancel`, vivo durante `state.full`.
        unsafe { (*(user_data as *const AtomicBool)).load(Ordering::Relaxed) }
    }
    // SAFETY: el `Arc` se mantiene vivo hasta que `state.full` termina.
    unsafe {
        params.set_abort_callback(Some(should_abort));
        params.set_abort_callback_user_data(Arc::as_ptr(&cancel) as *mut std::ffi::c_void);
    }

    state.full(params, audio).map_err(|e| anyhow!("error de transcripción: {e}"))?;
    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow!("cancelado"));
    }

    let eot = ctx.token_eot();
    // Los tokens son trozos de palabra: una palabra nueva empieza con espacio.
    // De cada palabra se toma el instante DTW de su primer token.
    struct Pending {
        bytes: Vec<u8>,
        start: f64,
        /// Fin según los tokens (poco preciso; solo como límite).
        end: f64,
    }
    let mut pending: Vec<Pending> = Vec::new();
    for segment in state.as_iter() {
        let (seg_start, seg_end) = (segment.start_timestamp() as f64 / 100.0, segment.end_timestamp() as f64 / 100.0);
        let mut first_in_segment = true;
        for i in 0..segment.n_tokens() {
            let Some(token) = segment.get_token(i) else { continue };
            if token.token_id() >= eot {
                continue; // tokens especiales ([_BEG_], marcas de tiempo…)
            }
            let Ok(piece) = token.to_bytes() else { continue };
            let data = token.token_data();
            let dtw = if data.t_dtw >= 0 { data.t_dtw as f64 / 100.0 } else { data.t0 as f64 / 100.0 };
            let t = dtw.clamp(seg_start, seg_end);
            let t1 = (data.t1 as f64 / 100.0).clamp(seg_start, seg_end);
            if piece.first() == Some(&b' ') || first_in_segment {
                pending.push(Pending { bytes: Vec::new(), start: t, end: t1 });
                first_in_segment = false;
            }
            let w = pending.last_mut().unwrap();
            w.bytes.extend_from_slice(piece);
            w.end = w.end.max(t1);
        }
    }

    let mut words: Vec<Word> = Vec::new();
    for i in 0..pending.len() {
        let p = &pending[i];
        let text = String::from_utf8_lossy(&p.bytes).trim().to_owned();
        if text.is_empty() {
            continue;
        }
        // Duración estimada por la longitud de la palabra, sin pasar a la siguiente.
        let natural = p.start + (0.08 * text.chars().count() as f64).clamp(0.25, 1.2);
        let next = pending.get(i + 1).map(|n| n.start);
        let mut end = match next {
            Some(n) if n - p.start < 1.5 => n,
            _ => natural,
        };
        // (El fin de segmento de Whisper suele quedarse corto: no se usa como límite.)
        end = end.min(natural.max(p.end.min(natural + 0.5)));
        if let Some(n) = next {
            end = end.min(n);
        }
        words.push(Word { start: p.start, end: end.max(p.start + 0.05), text });
    }
    // DTW marca el inicio de cada palabra con ~0,15–0,2 s de retraso: es mejor
    // que el subtítulo aparezca un poco antes que después.
    for w in &mut words {
        w.start = (w.start - LEAD).max(0.0);
    }
    for i in 1..words.len() {
        if words[i].start < words[i - 1].start {
            words[i].start = words[i - 1].start;
        }
    }
    Ok(words)
}
