//! Render: compositor de capas en GPU (wgpu) y rasterizado de texto.
//!
//! El compositor no depende de la interfaz: el visor lo usa con el dispositivo
//! de egui y la exportación podrá usarlo con un dispositivo propio, sin ventana.

mod compositor;
mod lut;
mod scene;
mod text;

pub use compositor::{Compositor, Grade, Layer, LayerSource, LutSource, CANVAS_FORMAT};
pub use lut::Lut;
pub use scene::{load_image, RgbaImage, Scene, SceneLayer};
pub use text::{font_families, font_file_family, TextParams, TextRaster, TextRenderer};
pub use wgpu;

/// Crea un dispositivo wgpu sin ventana (exportación y pruebas).
pub fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster_block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster_block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// `block_on` mínimo para no depender de un runtime async.
fn pollster_block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}
