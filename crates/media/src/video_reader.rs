use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::ffmpeg::{self, format::Pixel, media, software::scaling};

/// Imagen RGBA lista para mostrar.
#[derive(Clone)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Arc<Vec<u8>>,
}

/// Lector síncrono de video: devuelve el cuadro visible en un instante dado.
///
/// Avanzar en orden es barato (solo decodifica); retroceder o saltar lejos hace
/// un seek al keyframe anterior y decodifica hasta el instante pedido.
pub struct VideoReader {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    index: usize,
    time_base: f64,
    start: f64,
    frame_dur: f64,
    rotation: u32,
    max_size: (u32, u32),
    packet: ffmpeg::Packet,
    /// Último cuadro con pts <= instante pedido, y el siguiente (ya decodificado).
    cur: Option<(f64, ffmpeg::frame::Video)>,
    next: Option<(f64, ffmpeg::frame::Video)>,
    spare: ffmpeg::frame::Video,
    eof: bool,
    scaler: Option<(scaling::Context, ScaleKey)>,
    high_quality: bool,
    cache: Option<(f64, Image)>,
}

const EPS: f64 = 1e-4;
/// Más allá de esto, es más rápido hacer seek que decodificar en orden.
const SEEK_AHEAD: f64 = 2.0;

impl VideoReader {
    pub fn open(path: &Path, max_size: (u32, u32)) -> Result<Self> {
        crate::init();
        let ictx = ffmpeg::format::input(path)?;
        let start = crate::container_start(&ictx);
        let stream = ictx
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| anyhow!("{} no tiene video", path.display()))?;
        let index = stream.index();
        let rotation = crate::stream_rotation(&stream);
        let time_base = f64::from(stream.time_base());
        let rate = stream.avg_frame_rate();
        let frame_dur = if rate.numerator() > 0 && rate.denominator() > 0 {
            1.0 / f64::from(rate)
        } else {
            1.0 / 30.0
        };
        let mut ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        ctx.set_threading(ffmpeg::codec::threading::Config::kind(ffmpeg::codec::threading::Type::Frame));
        let decoder = ctx.decoder().video()?;
        Ok(Self {
            ictx,
            decoder,
            index,
            time_base,
            start,
            frame_dur,
            rotation,
            max_size,
            packet: ffmpeg::Packet::empty(),
            cur: None,
            next: None,
            spare: ffmpeg::frame::Video::empty(),
            eof: false,
            scaler: None,
            high_quality: false,
            cache: None,
        })
    }

    /// Escalado de mayor calidad (más lento): para exportar.
    pub fn set_high_quality(&mut self, on: bool) {
        self.high_quality = on;
        self.scaler = None;
        self.cache = None;
    }

    /// Cambia el tamaño máximo de salida.
    pub fn set_max_size(&mut self, max_size: (u32, u32)) {
        if max_size != self.max_size {
            self.max_size = max_size;
            self.scaler = None;
            self.cache = None;
        }
    }

    /// Cuadro visible en el instante `t` (segundos del archivo).
    pub fn frame_at(&mut self, t: f64) -> Result<Option<Image>> {
        let fresh = self.cur.is_none() && self.next.is_none() && !self.eof;
        let need_seek = match (&self.cur, &self.next) {
            (Some((pts, _)), _) => t < pts - EPS || t > pts + SEEK_AHEAD,
            (None, Some((pts, _))) => t < pts - EPS || t > pts + SEEK_AHEAD,
            (None, None) => !(fresh && t < SEEK_AHEAD),
        };
        if need_seek {
            crate::seek_input(&mut self.ictx, self.start, t);
            self.decoder.flush();
            self.cur = None;
            self.next = None;
            self.eof = false;
        }

        loop {
            if self.next.is_none() {
                self.next = self.decode_one()?;
                if self.next.is_none() {
                    break; // fin del archivo: se mantiene el último cuadro
                }
            }
            let next_pts = self.next.as_ref().unwrap().0;
            if next_pts <= t + EPS {
                if let Some((_, old)) = self.cur.take() {
                    self.spare = old;
                }
                self.cur = self.next.take();
            } else {
                break;
            }
        }

        let chosen = self.cur.as_ref().or(self.next.as_ref());
        let Some((pts, frame)) = chosen else { return Ok(None) };
        let pts = *pts;
        if let Some((cached_pts, img)) = &self.cache {
            if *cached_pts == pts {
                return Ok(Some(img.clone()));
            }
        }
        let img = convert(frame, self.max_size, self.rotation, self.high_quality, &mut self.scaler)?;
        self.cache = Some((pts, img.clone()));
        Ok(Some(img))
    }

    fn decode_one(&mut self) -> Result<Option<(f64, ffmpeg::frame::Video)>> {
        let mut frame = std::mem::replace(&mut self.spare, ffmpeg::frame::Video::empty());
        loop {
            if self.decoder.receive_frame(&mut frame).is_ok() {
                let pts = frame
                    .timestamp()
                    .or(frame.pts())
                    .map(|p| p as f64 * self.time_base - self.start)
                    .unwrap_or_else(|| {
                        let last = self.next.as_ref().or(self.cur.as_ref()).map(|c| c.0).unwrap_or(-self.frame_dur);
                        last + self.frame_dur
                    });
                return Ok(Some((pts, frame)));
            }
            if self.eof {
                return Ok(None);
            }
            match self.packet.read(&mut self.ictx) {
                Ok(()) => {
                    if self.packet.stream() == self.index {
                        if let Err(e) = self.decoder.send_packet(&self.packet) {
                            log::debug!("paquete de video inválido: {e}");
                        }
                    }
                }
                Err(e) => {
                    if !matches!(e, ffmpeg::Error::Eof) {
                        log::warn!("error leyendo video: {e}");
                    }
                    let _ = self.decoder.send_eof();
                    self.eof = true;
                }
            }
        }
    }
}

type ScaleKey = (Pixel, u32, u32, bool, bool);

fn convert(
    src: &ffmpeg::frame::Video,
    max_size: (u32, u32),
    rotation: u32,
    high_quality: bool,
    scaler: &mut Option<(scaling::Context, ScaleKey)>,
) -> Result<Image> {
    use ffmpeg::color;
    // HD y superior usa BT.709; sin etiqueta, se asume por la resolución (como los reproductores).
    let bt709 = match src.color_space() {
        color::Space::BT709 => true,
        color::Space::Unspecified => src.height() >= 720,
        _ => false,
    };
    let full = src.color_range() == color::Range::JPEG
        || matches!(src.format(), Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P);
    let key = (src.format(), src.width(), src.height(), bt709, full);
    // El límite aplica a la imagen ya girada.
    let max_size = if rotation % 180 == 90 { (max_size.1, max_size.0) } else { max_size };
    let (dw, dh) = fit(src.width(), src.height(), max_size);
    if scaler.as_ref().is_none_or(|(_, k)| *k != key) {
        let flags = if high_quality { scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND } else { scaling::Flags::BILINEAR };
        let mut ctx = scaling::Context::get(src.format(), src.width(), src.height(), Pixel::RGBA, dw, dh, flags)?;
        crate::set_colorspace(&mut ctx, bt709, full, bt709, true);
        *scaler = Some((ctx, key));
    }
    let (ctx, _) = scaler.as_mut().unwrap();
    let mut out = ffmpeg::frame::Video::empty();
    ctx.run(src, &mut out)?;

    // Copiar a un buffer compacto (el stride de FFmpeg puede tener relleno).
    let (w, h) = (out.width() as usize, out.height() as usize);
    let stride = out.stride(0);
    let data = out.data(0);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        rgba.extend_from_slice(&data[y * stride..y * stride + w * 4]);
    }
    let (rgba, w, h) = rotate(rgba, w, h, rotation);
    Ok(Image { width: w, height: h, rgba: Arc::new(rgba) })
}

/// Gira una imagen RGBA en sentido horario (0, 90, 180 o 270 grados).
fn rotate(src: Vec<u8>, w: usize, h: usize, rotation: u32) -> (Vec<u8>, usize, usize) {
    if rotation == 0 {
        return (src, w, h);
    }
    let px = |x: usize, y: usize| &src[(y * w + x) * 4..(y * w + x) * 4 + 4];
    let mut out = Vec::with_capacity(src.len());
    match rotation {
        90 => {
            for y in 0..w {
                for x in 0..h {
                    out.extend_from_slice(px(y, h - 1 - x));
                }
            }
            (out, h, w)
        }
        180 => {
            for y in 0..h {
                for x in 0..w {
                    out.extend_from_slice(px(w - 1 - x, h - 1 - y));
                }
            }
            (out, w, h)
        }
        270 => {
            for y in 0..w {
                for x in 0..h {
                    out.extend_from_slice(px(w - 1 - y, x));
                }
            }
            (out, h, w)
        }
        _ => (src, w, h),
    }
}

/// Tamaño que conserva la proporción y cabe en `max` (dimensiones pares).
pub(crate) fn fit(w: u32, h: u32, max: (u32, u32)) -> (u32, u32) {
    if w <= max.0 && h <= max.1 {
        return (w, h);
    }
    let scale = (max.0 as f64 / w as f64).min(max.1 as f64 / h as f64);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

#[cfg(test)]
mod tests {
    use super::{fit, rotate};

    #[test]
    fn rotate_moves_pixels_clockwise() {
        // 2x1: [A B] → girado 90° horario queda en columna: A arriba, B abajo.
        let src = vec![1, 1, 1, 1, 2, 2, 2, 2];
        let (out, w, h) = rotate(src.clone(), 2, 1, 90);
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![1, 1, 1, 1, 2, 2, 2, 2]);
        let (out, _, _) = rotate(src.clone(), 2, 1, 270);
        assert_eq!(out, vec![2, 2, 2, 2, 1, 1, 1, 1]);
        let (out, _, _) = rotate(src, 2, 1, 180);
        assert_eq!(out, vec![2, 2, 2, 2, 1, 1, 1, 1]);
    }

    #[test]
    fn fit_keeps_aspect_and_bounds() {
        assert_eq!(fit(1280, 720, (1920, 1080)), (1280, 720));
        assert_eq!(fit(3840, 2160, (1920, 1080)), (1920, 1080));
        assert_eq!(fit(1080, 1920, (1920, 1080)), (608, 1080));
    }
}
