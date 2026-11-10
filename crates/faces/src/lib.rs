//! Detección de rostros con Vision (el framework de Apple, en el Mac, sin internet)
//! y trayectoria suavizada del rostro principal para reencuadrar videos.

mod track;

pub use track::{main_face, smooth_path, FaceSample};

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use snipsy_core::follow::FaceTrack;

/// Muestras por segundo al analizar un video.
pub const SAMPLES_PER_SEC: f64 = 5.0;
/// Tamaño máximo de los cuadros analizados (Vision no necesita más).
const ANALYSIS_SIZE: (u32, u32) = (640, 640);

/// Analiza un video y devuelve la trayectoria suavizada del rostro principal
/// (`None` si nunca aparece nadie). `progress` recibe 0–1.
pub fn analyze(path: &Path, duration: f64, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> anyhow::Result<Option<FaceTrack>> {
    let interval = 1.0 / SAMPLES_PER_SEC;
    let n = ((duration / interval).ceil() as usize).max(1);
    let mut reader = snipsy_media::VideoReader::open(path, ANALYSIS_SIZE)?;
    let detector = Detector::new();
    let mut samples = Vec::with_capacity(n);
    let mut previous = None;
    for i in 0..n {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("cancelado");
        }
        let t = (i as f64 * interval).min((duration - 0.01).max(0.0));
        let sample = match reader.frame_at(t)? {
            Some(img) => {
                let faces = detector.detect(&img.rgba, img.width, img.height)?;
                main_face(&faces, previous)
            }
            None => None,
        };
        if sample.is_some() {
            previous = sample;
        }
        samples.push(sample);
        if i % 5 == 0 {
            progress(i as f32 / n as f32);
        }
    }
    progress(1.0);
    Ok(smooth_path(&samples, interval).map(|path| FaceTrack { interval, path }))
}

/// Rostro detectado, en coordenadas normalizadas (0–1) con origen arriba a la izquierda.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub confidence: f32,
}

impl Face {
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[cfg(target_os = "macos")]
mod vision {
    use anyhow::{anyhow, Result};
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::AnyThread;
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{
        CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    };
    use objc2_foundation::{NSArray, NSDictionary};
    use objc2_vision::{VNDetectFaceRectanglesRequest, VNImageRequestHandler, VNRequest};

    use super::Face;

    /// Detector reutilizable (una petición de Vision por hilo).
    pub struct Detector {
        request: Retained<VNDetectFaceRectanglesRequest>,
    }

    impl Detector {
        pub fn new() -> Self {
            Self { request: unsafe { VNDetectFaceRectanglesRequest::new() } }
        }

        /// Rostros en una imagen RGBA compacta de `width × height`.
        pub fn detect(&self, rgba: &[u8], width: usize, height: usize) -> Result<Vec<Face>> {
            anyhow::ensure!(rgba.len() >= width * height * 4 && width > 0 && height > 0, "imagen inválida");
            autoreleasepool(|_| {
                let data = CFData::from_bytes(&rgba[..width * height * 4]);
                let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or_else(|| anyhow!("CGDataProvider"))?;
                let space = CGColorSpace::new_device_rgb().ok_or_else(|| anyhow!("CGColorSpace"))?;
                // RGBX: el canal alfa se ignora.
                let info = CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0);
                let image = unsafe {
                    CGImage::new(
                        width,
                        height,
                        8,
                        32,
                        width * 4,
                        Some(&space),
                        info,
                        Some(&provider),
                        std::ptr::null(),
                        false,
                        CGColorRenderingIntent::RenderingIntentDefault,
                    )
                }
                .ok_or_else(|| anyhow!("CGImage"))?;
                let handler = unsafe {
                    VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &image, &NSDictionary::new())
                };
                let request: &VNRequest = &self.request;
                handler
                    .performRequests_error(&NSArray::from_slice(&[request]))
                    .map_err(|e| anyhow!("Vision: {}", e.localizedDescription()))?;
                let mut faces = Vec::new();
                if let Some(results) = unsafe { self.request.results() } {
                    for obs in results.iter() {
                        let b = unsafe { obs.boundingBox() };
                        let confidence = unsafe { obs.confidence() };
                        // Vision usa origen abajo a la izquierda.
                        faces.push(Face {
                            x: b.origin.x as f32,
                            y: (1.0 - b.origin.y - b.size.height) as f32,
                            w: b.size.width as f32,
                            h: b.size.height as f32,
                            confidence,
                        });
                    }
                }
                Ok(faces)
            })
        }
    }

    impl Default for Detector {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(target_os = "macos")]
pub use vision::Detector;

/// En otros sistemas no hay detector: no se encuentran rostros.
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct Detector;

#[cfg(not(target_os = "macos"))]
impl Detector {
    pub fn new() -> Self {
        Self
    }
    pub fn detect(&self, _rgba: &[u8], _width: usize, _height: usize) -> anyhow::Result<Vec<Face>> {
        Ok(Vec::new())
    }
}
