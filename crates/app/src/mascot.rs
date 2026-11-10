//! Snipi, la mascota de Snipsy: una bolita con orejas de tijera, dibujada con
//! formas simples (sin imágenes) y animada según su estado de ánimo.

use std::f32::consts::{PI, TAU};

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    /// Esperando: respira y parpadea.
    Idle,
    /// El usuario está escribiendo.
    Listening,
    /// Pensando la respuesta (cuando consulte a la IA).
    #[allow(dead_code)]
    Thinking,
    /// Trabajando en algo largo (subtítulos, exportación): las tijeras cortan.
    Working,
    Happy,
    Confused,
}

const BODY: Color32 = Color32::from_rgb(46, 196, 182);
const BODY_DARK: Color32 = Color32::from_rgb(28, 150, 140);
const BELLY: Color32 = Color32::from_rgb(170, 240, 228);
/// Hojas de tijera metálicas y tornillo coral.
const BLADE: Color32 = Color32::from_rgb(214, 222, 232);
const BLADE_DARK: Color32 = Color32::from_rgb(120, 132, 150);
const SCREW: Color32 = Color32::from_rgb(255, 107, 107);
const INK: Color32 = Color32::from_rgb(30, 34, 44);
const CHEEK: Color32 = Color32::from_rgba_premultiplied(120, 40, 60, 90);

fn ellipse(c: Pos2, rx: f32, ry: f32, rot: f32) -> Vec<Pos2> {
    let (s, co) = rot.sin_cos();
    (0..40)
        .map(|i| {
            let a = i as f32 / 40.0 * TAU;
            let (x, y) = (a.cos() * rx, a.sin() * ry);
            c + Vec2::new(x * co - y * s, x * s + y * co)
        })
        .collect()
}

fn arc(c: Pos2, r: Vec2, from: f32, to: f32) -> Vec<Pos2> {
    (0..=16).map(|i| {
        let a = from + (to - from) * i as f32 / 16.0;
        c + Vec2::new(a.cos() * r.x, a.sin() * r.y)
    })
    .collect()
}

/// Dibuja a Snipi en un recuadro de `size` y devuelve la respuesta (clic = saluda).
pub fn draw(ui: &mut Ui, size: f32, mood: Mood) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let t = ui.input(|i| i.time) as f32;
    // Siempre anima (suave): pedir el siguiente cuadro solo si se ve.
    if ui.is_rect_visible(rect) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    paint(ui.painter_at(rect.expand(4.0)), rect, mood, t, response.hovered());
    response
}

fn paint(p: egui::Painter, rect: Rect, mood: Mood, t: f32, hovered: bool) {
    let k = rect.width() / 120.0;
    // Respiración y saltitos.
    let bob = match mood {
        Mood::Happy => -(t * 7.0).sin().abs() * 7.0,
        Mood::Working => (t * 9.0).sin() * 1.5,
        Mood::Confused => 0.0,
        _ => (t * 2.2).sin() * 2.0,
    } * k;
    let tilt = match mood {
        Mood::Confused => 0.12,
        Mood::Thinking => (t * 1.5).sin() * 0.05,
        _ => 0.0,
    };
    let c = rect.center() + Vec2::new(0.0, 20.0 * k + bob);
    let squash = 1.0 + (t * 2.2).cos() * 0.015;

    // Sombra.
    p.add(Shape::convex_polygon(
        ellipse(Pos2::new(rect.center().x, rect.bottom() - 6.0 * k), 34.0 * k - bob * 0.5, 5.0 * k, 0.0),
        Color32::from_black_alpha(70),
        Stroke::NONE,
    ));

    // Orejas de tijera (detrás de la cabeza): dos hojas que se cruzan.
    let snip = match mood {
        Mood::Working => (t * 12.0).sin().abs() * 0.45,
        Mood::Happy => (t * 7.0).sin().abs() * 0.25,
        Mood::Listening => 0.15,
        _ => 0.05 + ((t * 0.7).sin().max(0.0).powi(8)) * 0.3,
    };
    // Las hojas se cruzan en X sobre la cabeza; las colas quedan detrás de ella.
    let pivot = c + Vec2::new(tilt * 20.0 * k, -40.0 * k);
    for side in [-1.0f32, 1.0] {
        let angle = -PI / 2.0 - side * (0.38 + snip) + tilt;
        let d = Vec2::new(angle.cos(), angle.sin());
        let n = Vec2::new(-d.y, d.x) * side;
        let blade = vec![
            pivot - d * 16.0 * k + n * 3.5 * k,
            pivot + n * 5.0 * k,
            pivot + d * 22.0 * k + n * 3.0 * k,
            pivot + d * 36.0 * k,
            pivot - n * 2.5 * k,
            pivot - d * 16.0 * k - n * 2.5 * k,
        ];
        p.add(Shape::convex_polygon(blade, BLADE, Stroke::new(1.5 * k, BLADE_DARK)));
        // Filo brillante.
        p.line_segment([pivot + n * 3.0 * k, pivot + d * 30.0 * k + n * 0.8 * k], Stroke::new(1.3 * k, Color32::WHITE));
    }
    p.add(Shape::convex_polygon(ellipse(pivot, 4.5 * k, 4.5 * k, 0.0), SCREW, Stroke::new(1.2 * k, BLADE_DARK)));

    // Cuerpo.
    p.add(Shape::convex_polygon(ellipse(c, 42.0 * k / squash, 38.0 * k * squash, tilt), BODY, Stroke::new(2.0 * k, BODY_DARK)));
    p.add(Shape::convex_polygon(ellipse(c + Vec2::new(0.0, 16.0 * k), 25.0 * k, 17.0 * k, tilt), BELLY, Stroke::NONE));
    // Bracitos.
    let wave = if mood == Mood::Happy || hovered { (t * 10.0).sin() * 0.5 } else { 0.0 };
    for side in [-1.0f32, 1.0] {
        let a = PI / 2.0 - side * (1.2 + if side > 0.0 { wave } else { 0.0 });
        let base = c + Vec2::new(side * 38.0 * k, 6.0 * k);
        p.add(Shape::convex_polygon(ellipse(base + Vec2::new(a.cos(), a.sin()) * 5.0 * k, 9.0 * k, 5.5 * k, a), BODY, Stroke::new(1.5 * k, BODY_DARK)));
    }

    // Cara.
    let face = c + Vec2::new(tilt * 30.0 * k, -6.0 * k);
    let blink = (t % 4.3) < 0.13 || ((t + 0.25) % 4.3) < 0.08 && mood == Mood::Idle;
    let look = match mood {
        Mood::Thinking => Vec2::new(0.5, -0.8),
        Mood::Listening => Vec2::new(0.0, 0.6),
        Mood::Confused => Vec2::new(-0.4, -0.2),
        _ => Vec2::new((t * 0.6).sin() * 0.5, (t * 0.4).cos() * 0.2),
    };
    for side in [-1.0f32, 1.0] {
        let eye = face + Vec2::new(side * 14.0 * k, 0.0);
        let happy = mood == Mood::Happy;
        if happy {
            // Ojos «^ ^».
            p.add(Shape::line(arc(eye + Vec2::new(0.0, 3.0 * k), Vec2::new(7.0 * k, 6.0 * k), PI, TAU), Stroke::new(2.8 * k, INK)));
        } else if blink {
            p.line_segment([eye - Vec2::new(7.0 * k, 0.0), eye + Vec2::new(7.0 * k, 0.0)], Stroke::new(2.5 * k, INK));
        } else {
            let small = mood == Mood::Confused && side > 0.0;
            let (rx, ry) = if small { (7.0, 8.0) } else { (9.0, 11.0) };
            p.add(Shape::convex_polygon(ellipse(eye, rx * k, ry * k, 0.0), Color32::WHITE, Stroke::new(1.5 * k, INK)));
            let pupil = eye + look * 3.5 * k;
            p.add(Shape::convex_polygon(ellipse(pupil, 5.0 * k, 5.0 * k, 0.0), INK, Stroke::NONE));
            p.add(Shape::convex_polygon(ellipse(pupil + Vec2::new(-1.6, -1.8) * k, 1.6 * k, 1.6 * k, 0.0), Color32::WHITE, Stroke::NONE));
        }
        p.add(Shape::convex_polygon(ellipse(eye + Vec2::new(side * 6.0 * k, 12.0 * k), 5.0 * k, 3.5 * k, 0.0), CHEEK, Stroke::NONE));
    }
    if mood == Mood::Confused {
        // Ceja levantada.
        let e = face + Vec2::new(-14.0 * k, -15.0 * k);
        p.line_segment([e - Vec2::new(7.0, -2.0) * k, e + Vec2::new(7.0, -3.0) * k], Stroke::new(2.2 * k, INK));
    }

    // Boca.
    let m = face + Vec2::new(0.0, 15.0 * k);
    match mood {
        Mood::Happy => {
            let mut pts = arc(m, Vec2::new(8.0 * k, 7.0 * k), 0.0, PI);
            pts.push(m + Vec2::new(8.0 * k, 0.0));
            p.add(Shape::convex_polygon(pts, Color32::from_rgb(120, 30, 50), Stroke::new(1.5 * k, INK)));
        }
        Mood::Thinking | Mood::Working => {
            p.add(Shape::convex_polygon(ellipse(m + Vec2::new(0.0, 2.0 * k), 3.0 * k, 3.5 * k, 0.0), Color32::from_rgb(120, 30, 50), Stroke::new(1.2 * k, INK)));
        }
        Mood::Confused => {
            let pts: Vec<Pos2> = (0..=12).map(|i| {
                let x = -8.0 + i as f32 * 16.0 / 12.0;
                m + Vec2::new(x, (x * 0.8).sin() * 2.0) * k
            })
            .collect();
            p.add(Shape::line(pts, Stroke::new(2.0 * k, INK)));
        }
        Mood::Idle | Mood::Listening => {
            p.add(Shape::line(arc(m - Vec2::new(0.0, 3.0 * k), Vec2::new(7.0 * k, 5.0 * k), 0.2, PI - 0.2), Stroke::new(2.2 * k, INK)));
        }
    }

    // Extras.
    let top = rect.center_top() + Vec2::new(30.0 * k, 10.0 * k);
    match mood {
        Mood::Thinking => {
            for i in 0..3 {
                let up = ((t * 4.0 - i as f32 * 0.6).sin().max(0.0)) * 4.0 * k;
                p.add(Shape::convex_polygon(ellipse(top + Vec2::new(i as f32 * 9.0 * k, -up), 3.0 * k, 3.0 * k, 0.0), Color32::from_gray(220), Stroke::NONE));
            }
        }
        Mood::Confused => {
            p.text(top + Vec2::new(8.0 * k, 0.0), egui::Align2::CENTER_CENTER, "?", egui::FontId::proportional(22.0 * k), Color32::from_rgb(255, 210, 90));
        }
        Mood::Happy => {
            for i in 0..2 {
                let a = t * 3.0 + i as f32 * PI;
                let s = top + Vec2::new(-60.0 * k * i as f32 + 4.0 * k, 6.0 * k) + Vec2::new(a.cos(), a.sin()) * 3.0 * k;
                p.text(s, egui::Align2::CENTER_CENTER, "✦", egui::FontId::proportional(13.0 * k), Color32::from_rgb(255, 220, 120));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dibuja a Snipi sin ventana, encima de `img`: egui teselá las formas y aquí se
    /// rasterizan los triángulos en CPU (las letras se omiten).
    fn rasterize(img: &mut image::RgbaImage, mascot: f32, top: f32, mood: Mood, t: f64) {
        let size = img.width() as f32;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(size))),
            time: Some(t),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(top);
                    draw(ui, mascot, mood);
                });
            });
        });
        out.textures_delta.clear();
        let prims = ctx.tessellate(out.shapes, 1.0);
        for prim in prims {
            let egui::epaint::Primitive::Mesh(mesh) = prim.primitive else { continue };
            for tri in mesh.indices.chunks(3) {
                let v: Vec<_> = tri.iter().map(|i| mesh.vertices[*i as usize]).collect();
                // Las letras usan la textura de fuentes: sin ella saldrían como rectángulos.
                if v.iter().any(|v| v.uv != egui::epaint::WHITE_UV) {
                    continue;
                }
                let (a, b, c) = (v[0].pos, v[1].pos, v[2].pos);
                let area = (b - a).x * (c - a).y - (b - a).y * (c - a).x;
                if area.abs() < 1e-6 {
                    continue;
                }
                let (x0, x1) = (a.x.min(b.x).min(c.x).floor().max(0.0) as u32, a.x.max(b.x).max(c.x).ceil().min(size - 1.0) as u32);
                let (y0, y1) = (a.y.min(b.y).min(c.y).floor().max(0.0) as u32, a.y.max(b.y).max(c.y).ceil().min(size - 1.0) as u32);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((b - p).x * (c - p).y - (b - p).y * (c - p).x) / area;
                        let w1 = ((c - p).x * (a - p).y - (c - p).y * (a - p).x) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        // Color premultiplicado interpolado, compuesto sobre lo que ya hay.
                        let col = |k: usize| v[0].color[k] as f32 * w0 + v[1].color[k] as f32 * w1 + v[2].color[k] as f32 * w2;
                        let alpha = col(3) / 255.0;
                        let px = img.get_pixel_mut(x, y);
                        let dst_a = px.0[3] as f32 / 255.0;
                        for k in 0..3 {
                            // Destino recto → premultiplicado, mezcla, y de vuelta a recto.
                            let d = px.0[k] as f32 * dst_a;
                            let out_a = alpha + dst_a * (1.0 - alpha);
                            let v = (col(k) + d * (1.0 - alpha)) / out_a.max(1e-6);
                            px.0[k] = v.clamp(0.0, 255.0) as u8;
                        }
                        px.0[3] = ((alpha + dst_a * (1.0 - alpha)) * 255.0).clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }

    #[test]
    fn draws_every_mood() {
        let dir = std::env::var("SNIPI_PNG").ok();
        for (i, mood) in [Mood::Idle, Mood::Listening, Mood::Thinking, Mood::Working, Mood::Happy, Mood::Confused].into_iter().enumerate() {
            let mut img = image::RgbaImage::from_pixel(240, 240, image::Rgba([27, 27, 27, 255]));
            rasterize(&mut img, 220.0, 10.0, mood, 1.0);
            // El cuerpo turquesa ocupa buena parte del dibujo.
            let body = img.pixels().filter(|p| p.0[0] < 80 && p.0[1] > 170 && p.0[2] > 160).count();
            assert!(body > 5_000, "{mood:?}: {body} píxeles de cuerpo");
            if let Some(dir) = &dir {
                img.save(format!("{dir}/snipi_{i}_{mood:?}.png")).unwrap();
            }
        }
    }

    /// Ícono de la app (1024×1024, cuadrado redondeado de macOS) con Snipi contenta.
    /// Se genera con `SNIPSY_ICON_OUT=archivo.png cargo test -p snipsy app_icon`.
    #[test]
    fn app_icon() {
        let Ok(out) = std::env::var("SNIPSY_ICON_OUT") else { return };
        // Se dibuja al doble y se reduce: bordes suaves.
        let big = 2048u32;
        let s = big as f32 / 1024.0;
        let (inset, radius) = (100.0 * s, 185.0 * s);
        let mut img = image::RgbaImage::from_pixel(big, big, image::Rgba([0, 0, 0, 0]));
        let lo = big as f32 - inset;
        for (x, y, px) in img.enumerate_pixels_mut() {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distancia a la esquina redondeada.
            let cx = fx.clamp(inset + radius, lo - radius);
            let cy = fy.clamp(inset + radius, lo - radius);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            if fx < inset || fx > lo || fy < inset || fy > lo || d > radius {
                continue;
            }
            // Degradado de noche azul a violeta, con un brillo arriba.
            let t = (fy - inset) / (lo - inset);
            let glow = (1.0 - ((fx - big as f32 / 2.0).powi(2) + (fy - inset - 260.0 * s).powi(2)).sqrt() / (520.0 * s)).max(0.0) * 40.0;
            let mix = |a: f32, b: f32| (a + (b - a) * t + glow).clamp(0.0, 255.0) as u8;
            *px = image::Rgba([mix(34.0, 62.0), mix(44.0, 30.0), mix(84.0, 96.0), 255]);
        }
        rasterize(&mut img, 720.0 * s, 170.0 * s, Mood::Happy, 0.11);
        let icon = image::imageops::resize(&img, 1024, 1024, image::imageops::FilterType::Lanczos3);
        icon.save(&out).unwrap();
    }
}
