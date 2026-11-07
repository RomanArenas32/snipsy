//! Agrupa palabras con tiempos en subtítulos legibles, y exporta a SRT.

/// Palabra reconocida, con su intervalo en segundos (tiempo del timeline).
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Caption {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Inicio (s) de cada palabra de `text`, en orden.
    pub words: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionStyle {
    /// Pocas palabras a la vez, grandes y en mayúsculas (Reels / TikTok).
    Reel,
    /// Frases completas en hasta dos líneas, abajo (YouTube, anuncios).
    Classic,
}

impl CaptionStyle {
    pub fn label(self) -> &'static str {
        match self {
            CaptionStyle::Reel => "Reel: pocas palabras, grandes",
            CaptionStyle::Classic => "Clásico: frases en dos líneas",
        }
    }

    fn limits(self) -> (usize, usize) {
        match self {
            CaptionStyle::Reel => (3, 18),
            CaptionStyle::Classic => (12, 64),
        }
    }
}

/// Silencio a partir del cual se empieza un subtítulo nuevo.
const PAUSE: f64 = 0.6;
/// Duración mínima en pantalla.
const MIN_SECS: f64 = 0.5;
/// Huecos menores que esto se rellenan alargando el subtítulo anterior (evita parpadeos).
const BRIDGE: f64 = 0.4;

fn ends_sentence(w: &str) -> bool {
    w.ends_with(['.', '?', '!', '…'])
}

fn clean(word: &str, style: CaptionStyle) -> String {
    let w = word.trim();
    match style {
        CaptionStyle::Classic => w.to_owned(),
        // En reels se quita la puntuación final y se pasa a mayúsculas.
        CaptionStyle::Reel => w.trim_end_matches([',', '.', ';', ':', '…']).to_uppercase(),
    }
}

/// Parte una frase larga en dos líneas por el espacio más cercano al centro.
fn two_lines(text: &str) -> String {
    if text.chars().count() <= 32 {
        return text.to_owned();
    }
    let mid = text.len() / 2;
    let split = text
        .match_indices(' ')
        .map(|(i, _)| i)
        .min_by_key(|i| (*i as i64 - mid as i64).abs());
    match split {
        Some(i) => format!("{}\n{}", &text[..i], &text[i + 1..]),
        None => text.to_owned(),
    }
}

pub fn group(words: &[Word], style: CaptionStyle) -> Vec<Caption> {
    let (max_words, max_chars) = style.limits();
    let mut out: Vec<Caption> = Vec::new();
    let mut cur: Vec<&Word> = Vec::new();
    let flush = |cur: &mut Vec<&Word>, out: &mut Vec<Caption>| {
        if cur.is_empty() {
            return;
        }
        let kept: Vec<(String, f64)> = cur
            .iter()
            .map(|w| (clean(&w.text, style), w.start))
            .filter(|(t, _)| !t.is_empty())
            .collect();
        if !kept.is_empty() {
            let mut text = kept.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" ");
            if style == CaptionStyle::Classic {
                text = two_lines(&text);
            }
            // Una «palabra» de Whisper puede contener espacios (raro): repetir su inicio.
            let words = kept.iter().flat_map(|(t, s)| std::iter::repeat_n(*s, t.split_whitespace().count())).collect();
            out.push(Caption { start: cur[0].start, end: cur[cur.len() - 1].end, text, words });
        }
        cur.clear();
    };
    for w in words.iter().filter(|w| !w.text.trim().is_empty()) {
        if let Some(last) = cur.last() {
            let chars: usize = cur.iter().map(|w| w.text.trim().chars().count() + 1).sum::<usize>() + w.text.trim().chars().count();
            let comma_break = last.text.ends_with(',') && cur.len() * 2 >= max_words;
            if cur.len() >= max_words || chars > max_chars || w.start - last.end > PAUSE || ends_sentence(&last.text) || comma_break {
                flush(&mut cur, &mut out);
            }
        }
        cur.push(w);
    }
    flush(&mut cur, &mut out);

    // Tiempos: duración mínima, rellenar huecos cortos y nunca solapar.
    for i in 0..out.len() {
        let next = out.get(i + 1).map(|c| c.start);
        let c = &mut out[i];
        c.end = c.end.max(c.start + MIN_SECS);
        if let Some(n) = next {
            if n - c.end < BRIDGE {
                c.end = n;
            }
            c.end = c.end.min(n);
        }
    }
    out.retain(|c| c.end > c.start);
    out
}

fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

pub fn to_srt(captions: &[Caption]) -> String {
    let mut s = String::new();
    for (i, c) in captions.iter().enumerate() {
        s += &format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(c.start), srt_time(c.end), c.text);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(spec: &[(&str, f64, f64)]) -> Vec<Word> {
        spec.iter().map(|(t, a, b)| Word { start: *a, end: *b, text: (*t).into() }).collect()
    }

    #[test]
    fn reel_groups_short_uppercase_chunks() {
        let w = words(&[
            ("Hola,", 0.0, 0.3),
            ("bienvenidos", 0.35, 0.9),
            ("a", 0.95, 1.0),
            ("nuestra", 1.05, 1.4),
            ("tienda.", 1.45, 1.9),
            ("Hoy", 3.0, 3.2),
            ("tenemos", 3.25, 3.6),
        ]);
        let c = group(&w, CaptionStyle::Reel);
        let texts: Vec<&str> = c.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["HOLA BIENVENIDOS", "A NUESTRA TIENDA", "HOY TENEMOS"]);
        // Hueco corto entre los dos primeros: se rellena. Pausa larga antes de «Hoy»: no.
        assert_eq!(c[0].end, c[1].start);
        assert_eq!(c[1].end, 1.9);
        assert_eq!(c[2].end, 3.6);
        assert_eq!(c[1].words, vec![0.95, 1.05, 1.45]);
    }

    #[test]
    fn classic_breaks_on_sentences_and_wraps() {
        let w = words(&[
            ("Este", 0.0, 0.2),
            ("es", 0.2, 0.3),
            ("un", 0.3, 0.4),
            ("ejemplo", 0.4, 0.8),
            ("bastante", 0.8, 1.2),
            ("largo", 1.2, 1.5),
            ("de", 1.5, 1.6),
            ("subtítulo.", 1.6, 2.2),
            ("Fin", 2.3, 2.5),
        ]);
        let c = group(&w, CaptionStyle::Classic);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].text, "Este es un ejemplo bastante\nlargo de subtítulo.");
        assert_eq!(c[1].text, "Fin");
        assert_eq!(c[1].end, 2.8, "duración mínima");
    }

    #[test]
    fn srt_format() {
        let s = to_srt(&[Caption { start: 61.5, end: 3725.004, text: "Hola".into(), words: vec![61.5] }]);
        assert_eq!(s, "1\n00:01:01,500 --> 01:02:05,004\nHola\n\n");
    }
}
