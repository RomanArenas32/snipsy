//! Atajos en español: entiende órdenes frecuentes sin usar IA (gratis e instantáneo).
//!
//! «corta los silencios y pásalo a vertical», «quita del 0:10 al 0:15»,
//! «título "OFERTA 2x1" al inicio», «exporta en 9:16 y cuadrado», «deshaz»…
//! Lo que no entiende se devuelve como error para mandarlo a la IA.

use crate::actions::{Action, ColorChange, Command, Target};

/// Lo que el intérprete necesita saber del proyecto.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseContext {
    pub has_selection: bool,
    /// Duración del proyecto en segundos.
    pub duration: f64,
    pub follow_faces: bool,
}

/// Acciones entendidas, o el fragmento que no se entendió.
pub fn parse(input: &str, ctx: &ParseContext) -> Result<Vec<Action>, String> {
    let (quoted, rest) = extract_quoted(input);
    let text = normalize(&rest);
    if text.trim().is_empty() && quoted.is_none() {
        return Err(input.trim().to_owned());
    }
    let mut quoted = quoted;
    let mut actions: Vec<Action> = Vec::new();
    // Texto de la cláusula que produjo las últimas acciones (para unir «… y 0:20»).
    let mut last: Option<(String, usize)> = None;
    for piece in split_clauses(&text) {
        if piece.trim().is_empty() {
            continue;
        }
        // «exporta en vertical y cuadrado»: formatos sueltos se suman a la exportación.
        if let Some(Action::Export { formats: list }) = actions.last_mut() {
            let extra = formats(&piece);
            if !extra.is_empty() && matches!(clause(&piece, ctx, &mut None).as_deref(), Some([Action::Edit(Command::Format { .. })])) {
                for f in extra {
                    if !list.contains(&f) {
                        list.push(f);
                    }
                }
                continue;
            }
        }
        if let Some(found) = clause(&piece, ctx, &mut quoted) {
            last = Some((piece, actions.len()));
            actions.extend(found);
            continue;
        }
        // Quizá completa a la anterior: «quita de 0:10 y 0:20», «exporta en vertical y cuadrado».
        if let Some((prev, at)) = last.take() {
            let joined = format!("{prev} y {piece}");
            // Solo si el fragmento aporta algo (si no, «… y haz magia» se perdería en silencio).
            let found = clause(&joined, ctx, &mut quoted).filter(|f| f.as_slice() != &actions[at..]);
            if let Some(found) = found {
                actions.truncate(at);
                actions.extend(found);
                last = Some((joined, at));
                continue;
            }
        }
        return Err(piece.trim().to_owned());
    }
    if actions.is_empty() {
        return Err(input.trim().to_owned());
    }
    Ok(actions)
}

// ------------------------------------------------------------------ texto

/// Texto entre comillas (para títulos) y el resto.
fn extract_quoted(input: &str) -> (Option<String>, String) {
    for (open, close) in [('"', '"'), ('«', '»'), ('“', '”'), ('\'', '\'')] {
        if let Some(a) = input.find(open) {
            let after = a + open.len_utf8();
            if let Some(b) = input[after..].find(close) {
                let quoted = input[after..after + b].trim().to_owned();
                let rest = format!("{} titulo {}", &input[..a], &input[after + b + close.len_utf8()..]);
                return (Some(quoted), rest);
            }
        }
    }
    // «título: OFERTA» / «texto que diga OFERTA».
    let lower = input.to_lowercase();
    for marker in ["que diga ", "que dice ", "titulo:", "título:", "texto:"] {
        if let Some(i) = lower.find(marker) {
            let start = i + marker.len();
            let quoted = input.get(start..).unwrap_or("").trim();
            if !quoted.is_empty() {
                return (Some(quoted.to_owned()), format!("{} titulo", &input[..i]));
            }
        }
    }
    (None, input.to_owned())
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.to_lowercase().chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let c = match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            // Coma decimal: «1,5x».
            ',' if i > 0 && chars[i - 1].is_ascii_digit() && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()) => '.',
            c if c.is_alphanumeric() || matches!(c, ':' | '.' | ',' | ';' | '-') => *c,
            _ => ' ',
        };
        out.push(c);
    }
    // Frases con «y» que no separan órdenes.
    let out = out.replace("blanco y negro", "blanconegro");
    format!(" {} ", out.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn split_clauses(text: &str) -> Vec<String> {
    let mut s = text.to_owned();
    for sep in [" y luego ", " y despues ", " luego ", " despues ", " tambien ", " ademas ", " y ", ";"] {
        s = s.replace(sep, ",");
    }
    s.split(',').map(|p| format!(" {} ", p.trim())).collect()
}

fn words(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Alguna palabra empieza por `stem` (con una falta de ortografía si la raíz es larga).
fn has(text: &str, stems: &[&str]) -> bool {
    words(text).iter().any(|w| {
        stems.iter().any(|stem| {
            if w.starts_with(stem) {
                return true;
            }
            let n = stem.chars().count();
            // Solo palabras largas: con raíces cortas una falta confunde «salida» con «cálida».
            if n < 6 {
                return false;
            }
            let wc: Vec<char> = w.chars().collect();
            (n - 1..=n + 1).any(|len| len <= wc.len() && edit_distance(&wc[..len].iter().collect::<String>(), stem) <= 1)
        })
    })
}

/// Alguna palabra es exactamente una de `list`.
fn word(text: &str, list: &[&str]) -> bool {
    words(text).iter().any(|w| list.contains(w))
}

fn phrase(text: &str, list: &[&str]) -> bool {
    list.iter().any(|p| text.contains(&format!(" {p} ")) || text.contains(&format!(" {p}")))
}

// ------------------------------------------------------------------ números

fn word_number(w: &str) -> Option<f64> {
    Some(match w {
        "un" | "uno" | "una" => 1.0,
        "dos" => 2.0,
        "tres" => 3.0,
        "cuatro" => 4.0,
        "cinco" => 5.0,
        "seis" => 6.0,
        "siete" => 7.0,
        "ocho" => 8.0,
        "nueve" => 9.0,
        "diez" => 10.0,
        "quince" => 15.0,
        "veinte" => 20.0,
        "treinta" => 30.0,
        "medio" => 0.5,
        _ => return None,
    })
}

/// Tiempos (segundos) en orden de aparición: «0:10», «1:05.5», «10 s», «10 segundos», «2 minutos».
fn times(text: &str) -> Vec<f64> {
    let ws = words(text);
    let mut out = Vec::new();
    let unit = |w: Option<&&str>| -> Option<f64> {
        let w = *w?;
        if w.starts_with("seg") || w == "s" {
            Some(1.0)
        } else if w.starts_with("min") || w == "m" {
            Some(60.0)
        } else {
            None
        }
    };
    let mut i = 0;
    while i < ws.len() {
        let w = ws[i].trim_matches(|c: char| c == '.' || c == ',');
        if let Some((m, s)) = w.split_once(':') {
            if let (Ok(m), Ok(s)) = (m.parse::<f64>(), s.parse::<f64>()) {
                out.push(m * 60.0 + s);
            }
        } else if let Some(num) = w.strip_suffix("seg").or(w.strip_suffix('s')).and_then(|n| n.parse::<f64>().ok()) {
            out.push(num);
        } else if let Ok(n) = w.parse::<f64>() {
            // Sin unidad cuenta como segundos, salvo que la siguiente diga minutos.
            out.push(n * unit(ws.get(i + 1)).unwrap_or(1.0));
            if unit(ws.get(i + 1)).is_some() {
                i += 1;
            }
        } else if let (Some(n), Some(u)) = (word_number(w), unit(ws.get(i + 1))) {
            out.push(n * u);
            i += 1;
        }
        i += 1;
    }
    out
}

/// Formatos mencionados (ancho, alto).
fn formats(text: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut push = |f: (u32, u32)| {
        if !out.contains(&f) {
            out.push(f);
        }
    };
    for w in words(text) {
        match w {
            "9:16" | "9x16" => push((1080, 1920)),
            "16:9" | "16x9" => push((1920, 1080)),
            "1:1" | "1x1" => push((1080, 1080)),
            "4:5" | "4x5" => push((1080, 1350)),
            "4k" => push((3840, 2160)),
            _ => {}
        }
    }
    if has(text, &["vertical", "reel", "tiktok", "short", "historia", "stori"]) {
        push((1080, 1920));
    }
    if has(text, &["horizontal", "youtube", "apaisado"]) {
        push((1920, 1080));
    }
    if has(text, &["cuadrad"]) {
        push((1080, 1080));
    }
    if phrase(text, &["feed", "retrato"]) {
        push((1080, 1350));
    }
    out
}

/// Quita del texto los formatos tipo «9:16» para que no se lean como tiempos.
fn without_formats(text: &str) -> String {
    words(text)
        .into_iter()
        .filter(|w| !matches!(*w, "9:16" | "9x16" | "16:9" | "16x9" | "1:1" | "1x1" | "4:5" | "4x5"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn speed_value(text: &str) -> Option<f64> {
    for w in words(text) {
        let n = w.strip_suffix('x').or(w.strip_prefix('x')).and_then(|n| n.parse::<f64>().ok());
        if let Some(n) = n {
            return Some(n);
        }
    }
    if has(text, &["normal"]) {
        return Some(1.0);
    }
    if has(text, &["doble", "duplic"]) {
        return Some(2.0);
    }
    if has(text, &["triple"]) {
        return Some(3.0);
    }
    if has(text, &["mitad", "lent"]) {
        return Some(0.5);
    }
    if has(text, &["aceler", "rapid"]) {
        return Some(1.5);
    }
    None
}

// ------------------------------------------------------------------ órdenes

fn negative(text: &str) -> bool {
    word(text, &["no", "sin"]) || has(text, &["desactiv", "apaga", "quita", "deja de"])
}

fn clause(text: &str, ctx: &ParseContext, quoted: &mut Option<String>) -> Option<Vec<Action>> {
    let one = |c: Command| Some(vec![Action::Edit(c)]);
    let default_target = if ctx.has_selection { Target::Selected } else { Target::All };
    let target = if has(text, &["todo", "toda", "cada"]) {
        Target::All
    } else if phrase(text, &["este clip", "esta toma", "seleccion", "seleccionado", "seleccionados"]) {
        Target::Selected
    } else {
        default_target.clone()
    };

    if has(text, &["ayuda"]) || phrase(text, &["que puedes", "que sabes", "como funciona", "comandos"]) {
        return Some(vec![Action::Help]);
    }
    if has(text, &["deshac", "deshaz", "undo", "revierte", "revertir"]) {
        return Some(vec![Action::Undo]);
    }
    if has(text, &["rehac", "rehaz", "redo"]) {
        return Some(vec![Action::Redo]);
    }
    if word(text, &["play", "reproduce", "reproducir", "reproducelo"]) {
        return Some(vec![Action::Play]);
    }
    if word(text, &["pausa", "pausar", "pausalo", "deten", "detente"]) {
        return Some(vec![Action::Pause]);
    }
    // Títulos: con texto entre comillas o la palabra título/texto.
    if quoted.is_some() && has(text, &["titulo", "texto", "letrero", "rotulo", "escrib"]) || has(text, &["titulo", "letrero", "rotulo"]) {
        let t = without_formats(text);
        let at = if has(&t, &["inicio", "principio", "comienzo"]) { Some(0.0) } else { times(&t).first().copied() };
        let secs = times(&t).get(1).copied().filter(|_| has(&t, &["dur"])).unwrap_or(3.0);
        let text = quoted.take().unwrap_or_default();
        return one(Command::AddText { text, at, secs });
    }
    if has(text, &["subtitul", "transcrib", "caption"]) || word(text, &["subs"]) {
        return Some(vec![Action::Subtitles]);
    }
    if has(text, &["silencios", "pausas"]) && !word(text, &["silencia", "silenciar", "silencialo"]) {
        return Some(vec![Action::CutSilences]);
    }
    if has(text, &["ducking"]) || (has(text, &["musica"]) && has(text, &["baj"]) && has(text, &["habl", "voz"])) {
        return Some(vec![Action::Ducking]);
    }
    if has(text, &["marca", "branding"]) && !has(text, &["desmarca"]) {
        return Some(vec![Action::ApplyBrand]);
    }
    if has(text, &["logo"]) {
        return Some(vec![Action::PlaceLogo]);
    }
    if has(text, &["export", "renderiz", "render"]) {
        return Some(vec![Action::Export { formats: formats(text) }]);
    }
    if has(text, &["rostro", "cara", "persona"]) && has(text, &["sig", "segui", "rastre", "track"]) {
        let on = !negative(text);
        return one(Command::FollowFace { target, on });
    }
    let fmts = formats(text);
    if !fmts.is_empty() && !has(text, &["velocidad"]) {
        let (width, height) = fmts[0];
        return one(Command::Format { width, height, follow_faces: ctx.follow_faces });
    }

    // Color.
    let mut color = ColorChange::default();
    let less = has(text, &["menos", "baj", "quita", "reduc"]);
    let sign = if less { -1.0 } else { 1.0 };
    if word(text, &["blanconegro", "bn"]) || has(text, &["monocrom", "grises"]) {
        color.black_and_white = true;
    }
    if has(text, &["brillo", "ilumin", "aclar", "luminos"]) || phrase(text, &["mas claro", "mas clara"]) {
        color.exposure = 0.5 * sign;
    }
    if has(text, &["oscur"]) {
        color.exposure = -0.5;
    }
    if has(text, &["contrast"]) {
        color.contrast = 0.2 * sign;
    }
    if has(text, &["satur", "vivo", "vivos", "intens"]) {
        color.saturation = 0.3 * sign;
    }
    if has(text, &["calid", "calor", "anaranj"]) {
        color.temperature = 0.3;
    }
    if has(text, &["frio", "fria", "azulad"]) {
        color.temperature = -0.3;
    }
    if (has(text, &["color", "filtro"]) && has(text, &["normal", "original", "restable", "quita", "reset"])) || phrase(text, &["sin filtro"]) {
        color.reset = true;
    }
    if color != ColorChange::default() {
        return one(Command::Color { target, change: color });
    }

    // Velocidad.
    if has(text, &["velocidad", "aceler", "rapid", "lent", "doble"]) || words(text).iter().any(|w| w.len() >= 2 && (w.ends_with('x') || w.starts_with('x')) && speed_value(w).is_some()) {
        let speed = speed_value(text)?;
        return one(Command::Speed { target, speed });
    }

    // Volumen.
    if word(text, &["silencia", "silenciar", "silencialo", "mutea", "mute"]) || phrase(text, &["sin sonido", "quita el sonido", "quitale el sonido"]) {
        return one(Command::Volume { target, db: -60.0, relative: false });
    }
    if has(text, &["volumen", "sonido", "audio"]) {
        let ws = words(text);
        let db = ws.iter().enumerate().find_map(|(i, w)| {
            w.strip_suffix("db").filter(|n| !n.is_empty()).and_then(|n| n.parse::<f32>().ok()).or_else(|| {
                (ws.get(i + 1) == Some(&"db")).then(|| w.parse::<f32>().ok()).flatten()
            })
        });
        let up = has(text, &["sub", "mas", "alto", "aument"]);
        let down = has(text, &["baj", "menos", "reduc"]);
        return match (db, up, down) {
            (Some(d), true, _) => one(Command::Volume { target, db: d.abs(), relative: true }),
            (Some(d), _, true) => one(Command::Volume { target, db: -d.abs(), relative: true }),
            (Some(d), _, _) => one(Command::Volume { target, db: d, relative: false }),
            (None, true, _) => one(Command::Volume { target, db: 6.0, relative: true }),
            (None, _, true) => one(Command::Volume { target, db: -6.0, relative: true }),
            _ => None,
        };
    }

    // Fundidos: sin selección, de entrada al primer clip y de salida al último.
    if has(text, &["fundido", "fade", "desvanec", "aparec", "difumin"]) {
        let secs = times(text).first().copied().unwrap_or(0.5);
        let entry = has(text, &["entrada", "inicio", "principio", "aparec", "comienzo"]) && !has(text, &["desaparec"]);
        let exit = has(text, &["salida", "final", "fin", "desaparec", "termin"]);
        let (fade_in, fade_out) = match (entry, exit) {
            (true, false) => (true, false),
            (false, true) => (false, true),
            _ => (true, true),
        };
        let mut out = Vec::new();
        if ctx.has_selection || matches!(target, Target::All) && has(text, &["todo", "cada"]) {
            out.push(Action::Edit(Command::Fade { target, fade_in: fade_in.then_some(secs), fade_out: fade_out.then_some(secs) }));
        } else {
            if fade_in {
                out.push(Action::Edit(Command::Fade { target: Target::First, fade_in: Some(secs), fade_out: None }));
            }
            if fade_out {
                out.push(Action::Edit(Command::Fade { target: Target::Last, fade_in: None, fade_out: Some(secs) }));
            }
        }
        return Some(out);
    }

    // Cortes y borrados.
    let cut = has(text, &["quit", "borr", "elimin", "cort", "recort", "saca", "sacale"]);
    let split = has(text, &["divid", "part", "separ"]) || word(text, &["corta", "cortar", "cortalo", "split"]);
    if cut || split {
        let t = times(text);
        // «los primeros / últimos N segundos».
        if let Some(n) = t.first().copied() {
            if has(text, &["primer"]) {
                return one(Command::CutRange { from: 0.0, to: n });
            }
            if has(text, &["ultim"]) {
                return one(Command::CutRange { from: (ctx.duration - n).max(0.0), to: ctx.duration });
            }
        }
        if cut && t.len() >= 2 {
            let (from, to) = (t[0].min(t[1]), t[0].max(t[1]));
            return one(Command::CutRange { from, to });
        }
        if cut && (has(text, &["clip", "esto", "esta", "seleccion"]) || word(text, &["borra", "borralo", "eliminalo", "quitalo"])) && t.is_empty() {
            let target = if has(text, &["todo"]) { Target::All } else { Target::Selected };
            return one(Command::Delete { target, ripple: true });
        }
        if split || (cut && (t.len() == 1 || phrase(text, &["aqui", "en el cabezal", "aca"]))) {
            return one(Command::Split { at: t.first().copied() });
        }
        if cut && t.len() == 1 {
            return None;
        }
    }
    None
}

/// Texto de ayuda con ejemplos.
pub const HELP: &str = "Puedo hacer al momento (gratis):\n\
• «corta los silencios»\n\
• «quita del 0:10 al 0:15» · «quita los primeros 3 segundos»\n\
• «corta aquí» · «divide en 0:20»\n\
• «título \"OFERTA 2x1\" al inicio»\n\
• «pásalo a vertical» · «formato cuadrado»\n\
• «subtitula el video» · «aplica mi marca» · «pon el logo»\n\
• «baja la música cuando hablo» (ducking)\n\
• «blanco y negro» · «más brillo» · «más cálido» · «colores más vivos»\n\
• «velocidad 2x» · «cámara lenta» · «sube el volumen» · «silencia este clip»\n\
• «fundido de entrada y salida»\n\
• «sigue la cara» · «exporta en 9:16 y 1:1» · «deshaz»\n\
Puedes encadenar: «corta los silencios y pásalo a vertical».";

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<Action> {
        parse(s, &ParseContext { has_selection: false, duration: 60.0, follow_faces: true }).unwrap_or_else(|e| panic!("no entendió «{s}»: {e}"))
    }

    fn edit(s: &str) -> Command {
        match p(s).as_slice() {
            [Action::Edit(c)] => c.clone(),
            other => panic!("«{s}» → {other:?}"),
        }
    }

    #[test]
    fn understands_common_orders() {
        assert_eq!(p("Córtale los silencios"), vec![Action::CutSilences]);
        assert_eq!(p("quita las pausas"), vec![Action::CutSilences]);
        assert_eq!(p("subtitula el video"), vec![Action::Subtitles]);
        assert_eq!(p("aplica mi marca"), vec![Action::ApplyBrand]);
        assert_eq!(p("pon el logo"), vec![Action::PlaceLogo]);
        assert_eq!(p("baja la música cuando hablo"), vec![Action::Ducking]);
        assert_eq!(p("deshaz"), vec![Action::Undo]);
        assert_eq!(p("¿qué puedes hacer?"), vec![Action::Help]);
        assert_eq!(edit("pásalo a vertical"), Command::Format { width: 1080, height: 1920, follow_faces: true });
        assert_eq!(edit("formato 1:1"), Command::Format { width: 1080, height: 1080, follow_faces: true });
        assert_eq!(edit("quita del 0:10 al 0:15"), Command::CutRange { from: 10.0, to: 15.0 });
        assert_eq!(edit("borra de 20 a 25 segundos"), Command::CutRange { from: 20.0, to: 25.0 });
        assert_eq!(edit("quita los primeros 3 segundos"), Command::CutRange { from: 0.0, to: 3.0 });
        assert_eq!(edit("elimina los últimos dos segundos"), Command::CutRange { from: 58.0, to: 60.0 });
        assert_eq!(edit("corta aquí"), Command::Split { at: None });
        assert_eq!(edit("divide en 1:05.5"), Command::Split { at: Some(65.5) });
        assert_eq!(edit("borra este clip"), Command::Delete { target: Target::Selected, ripple: true });
        assert_eq!(edit("título \"OFERTA 2x1\" al inicio"), Command::AddText { text: "OFERTA 2x1".into(), at: Some(0.0), secs: 3.0 });
        assert_eq!(edit("pon un texto que diga Envío gratis"), Command::AddText { text: "Envío gratis".into(), at: None, secs: 3.0 });
        assert_eq!(edit("velocidad 1,5x"), Command::Speed { target: Target::All, speed: 1.5 });
        assert_eq!(edit("cámara lenta"), Command::Speed { target: Target::All, speed: 0.5 });
        assert_eq!(edit("sube el volumen 3 dB"), Command::Volume { target: Target::All, db: 3.0, relative: true });
        assert_eq!(edit("silencia este clip"), Command::Volume { target: Target::Selected, db: -60.0, relative: false });
        assert!(matches!(edit("ponlo en blanco y negro"), Command::Color { change: ColorChange { black_and_white: true, .. }, .. }));
        assert!(matches!(edit("un poco más de brillo"), Command::Color { change: ColorChange { exposure, .. }, .. } if exposure > 0.0));
        assert!(matches!(edit("menos brillo"), Command::Color { change: ColorChange { exposure, .. }, .. } if exposure < 0.0));
        assert_eq!(edit("sigue la cara"), Command::FollowFace { target: Target::All, on: true });
        assert_eq!(edit("no sigas el rostro"), Command::FollowFace { target: Target::All, on: false });
        assert_eq!(
            p("fundido de entrada y salida"),
            vec![
                Action::Edit(Command::Fade { target: Target::First, fade_in: Some(0.5), fade_out: None }),
                Action::Edit(Command::Fade { target: Target::Last, fade_in: None, fade_out: Some(0.5) }),
            ]
        );
    }

    #[test]
    fn chains_merges_and_tolerates_typos() {
        assert_eq!(
            p("corta los silencios y luego pásalo a vertical"),
            vec![Action::CutSilences, Action::Edit(Command::Format { width: 1080, height: 1920, follow_faces: true })]
        );
        assert_eq!(p("exporta en vertical y cuadrado"), vec![Action::Export { formats: vec![(1080, 1920), (1080, 1080)] }]);
        assert_eq!(p("exportalo en 9:16, 1:1 y 16:9"), vec![Action::Export { formats: vec![(1080, 1920), (1080, 1080), (1920, 1080)] }]);
        assert_eq!(edit("quita entre 0:10 y 0:20"), Command::CutRange { from: 10.0, to: 20.0 });
        assert_eq!(p("subtitulalo"), vec![Action::Subtitles]);
        assert_eq!(p("cortá los silensios"), vec![Action::CutSilences]);
        assert_eq!(p("sutitula"), vec![Action::Subtitles]);
        assert_eq!(edit("ponlo vertcal"), Command::Format { width: 1080, height: 1920, follow_faces: true });
    }

    #[test]
    fn unknown_goes_to_ai() {
        let ctx = ParseContext::default();
        assert!(parse("haz un resumen de 30 segundos con lo mejor", &ctx).is_err());
        assert!(parse("¿cómo se llama tu mamá?", &ctx).is_err());
        assert_eq!(parse("corta los silencios y haz magia", &ctx).unwrap_err(), "haz magia");
        assert!(parse("   ", &ctx).is_err());
    }
}
