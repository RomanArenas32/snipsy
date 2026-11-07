//! Utilidades de tiempo y timecode.

/// Formatea segundos como timecode `HH:MM:SS:FF` (non-drop-frame).
///
/// Para tasas fraccionarias (23.976, 29.97) se usa la tasa redondeada para
/// numerar los cuadros, igual que el timecode non-drop de los editores.
pub fn format_timecode(seconds: f64, fps: f64) -> String {
    let fps = sanitize_fps(fps);
    let frame = seconds_to_frame(seconds, fps);
    let nominal = fps.round().max(1.0) as u64;
    let ff = frame % nominal;
    let total_secs = frame / nominal;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        total_secs / 3600,
        (total_secs / 60) % 60,
        total_secs % 60,
        ff
    )
}

/// Índice de cuadro correspondiente a un instante (en segundos).
pub fn seconds_to_frame(seconds: f64, fps: f64) -> u64 {
    // Pequeño epsilon para que 1.0/fps * n no caiga en el cuadro anterior.
    (seconds.max(0.0) * sanitize_fps(fps) + 1e-6).floor() as u64
}

/// Duración de un cuadro en segundos.
pub fn frame_duration(fps: f64) -> f64 {
    1.0 / sanitize_fps(fps)
}

fn sanitize_fps(fps: f64) -> f64 {
    if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        30.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_basic_timecodes() {
        assert_eq!(format_timecode(0.0, 30.0), "00:00:00:00");
        assert_eq!(format_timecode(1.5, 30.0), "00:00:01:15");
        assert_eq!(format_timecode(3661.0, 25.0), "01:01:01:00");
    }

    #[test]
    fn frame_boundaries_are_stable() {
        for n in 0..1000u64 {
            let t = n as f64 / 30.0;
            assert_eq!(seconds_to_frame(t, 30.0), n);
        }
    }

    #[test]
    fn invalid_fps_falls_back() {
        assert_eq!(format_timecode(1.0, 0.0), "00:00:01:00");
        assert_eq!(format_timecode(1.0, f64::NAN), "00:00:01:00");
    }
}
