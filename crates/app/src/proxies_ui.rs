//! Proxies en la interfaz: estado de cada medio, cola de fondo y el interruptor global.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Color32, RichText, Ui};
use snipsy_core::{Asset, Timeline};
use snipsy_media::proxy::{self, proxies_dir};
use snipsy_media::{Notify, ProxyEvent, ProxyQueue, ProxyRequest};

#[derive(Clone, Debug, PartialEq)]
pub enum ProxyState {
    Queued,
    Working(f32),
    Ready(PathBuf),
    Failed(String),
}

pub struct Proxies {
    queue: ProxyQueue,
    dir: PathBuf,
    /// Estado por archivo original (los ids de medios cambian entre proyectos; las rutas no).
    states: HashMap<PathBuf, ProxyState>,
    /// Usar los proxies en la vista previa.
    pub enabled: bool,
    /// Crear proxies automáticamente al importar videos de más de Full HD.
    pub auto: bool,
}

impl Proxies {
    pub fn new(notify: Notify) -> Self {
        let dir = proxies_dir();
        Self { queue: ProxyQueue::new(dir.clone(), notify), dir, states: HashMap::new(), enabled: true, auto: true }
    }

    pub fn state(&self, asset: &Asset) -> Option<&ProxyState> {
        self.states.get(&asset.path)
    }

    /// ¿Se puede crear un proxy de este medio? (video más grande que el tamaño del proxy)
    pub fn eligible(asset: &Asset) -> bool {
        asset.has_video && !asset.still && asset.width.min(asset.height) > proxy::PROXY_SHORT_SIDE
    }

    pub fn request(&mut self, asset: &Asset) {
        if !Self::eligible(asset) || matches!(self.states.get(&asset.path), Some(ProxyState::Queued | ProxyState::Working(_) | ProxyState::Ready(_))) {
            return;
        }
        self.states.insert(asset.path.clone(), ProxyState::Queued);
        self.queue.request(ProxyRequest {
            asset: asset.id.0,
            path: asset.path.clone(),
            width: asset.width,
            height: asset.height,
            fps: asset.fps,
            duration: asset.duration,
        });
    }

    /// Revisa los medios del proyecto: reconoce los proxies ya creados y, con `auto`,
    /// encola los videos pesados que no tienen.
    pub fn scan(&mut self, tl: &Timeline) {
        for a in &tl.assets {
            if !Self::eligible(a) || self.states.contains_key(&a.path) {
                continue;
            }
            if let Some(p) = proxy::existing_proxy_in(&self.dir, &a.path) {
                self.states.insert(a.path.clone(), ProxyState::Ready(p));
            } else if self.auto && proxy::needs_proxy(a.width, a.height) {
                self.request(a);
            }
        }
    }

    pub fn cancel_current(&self) {
        self.queue.cancel_current();
    }

    /// Procesa los avisos de la cola. Devuelve un mensaje para la barra de estado y
    /// si hay un proxy nuevo listo (hay que reconstruir el reproductor).
    pub fn poll(&mut self, tl: &Timeline) -> (Option<String>, bool) {
        let mut ready = false;
        let path_of = |id: u64| tl.assets.iter().find(|a| a.id.0 == id).map(|a| (a.path.clone(), a.name.clone()));
        let mut message = None;
        while let Ok(ev) = self.queue.events().try_recv() {
            match ev {
                ProxyEvent::Progress { asset, fraction } => {
                    if let Some((p, _)) = path_of(asset) {
                        self.states.insert(p, ProxyState::Working(fraction));
                    }
                }
                ProxyEvent::Done { asset, proxy, .. } => {
                    if let Some((p, name)) = path_of(asset) {
                        self.states.insert(p, ProxyState::Ready(proxy));
                        ready = true;
                        message = Some(format!("✅ Proxy listo: {name}. La vista previa ya lo usa."));
                    }
                }
                ProxyEvent::Failed { asset, error } => {
                    if let Some((p, name)) = path_of(asset) {
                        message = Some(format!("No se pudo crear el proxy de {name}: {error}"));
                        self.states.insert(p, ProxyState::Failed(error));
                    }
                }
                ProxyEvent::Cancelled { asset } => {
                    if let Some((p, _)) = path_of(asset) {
                        self.states.remove(&p);
                    }
                    message = Some("Proxy cancelado.".into());
                }
            }
        }
        (message, ready)
    }

    /// Proxies a usar en la vista previa (vacío si están desactivados).
    pub fn active(&self, tl: &Timeline) -> HashMap<u64, PathBuf> {
        if !self.enabled {
            return HashMap::new();
        }
        tl.assets
            .iter()
            .filter_map(|a| match self.states.get(&a.path) {
                Some(ProxyState::Ready(p)) => Some((a.id.0, p.clone())),
                _ => None,
            })
            .collect()
    }


    /// Etiqueta o botón de proxy de un medio en el media pool.
    pub fn asset_badge(&mut self, ui: &mut Ui, asset: &Asset) {
        if !Self::eligible(asset) {
            return;
        }
        match self.state(asset).cloned() {
            Some(ProxyState::Ready(_)) => {
                let color = if self.enabled { Color32::from_rgb(110, 200, 120) } else { Color32::GRAY };
                ui.label(RichText::new("Proxy ✓").small().color(color))
                    .on_hover_text("Copia liviana para editar con fluidez. Al exportar se usa el original.");
            }
            Some(ProxyState::Working(f)) => {
                ui.add(egui::ProgressBar::new(f).desired_width(70.0).text(RichText::new(format!("Proxy {:.0}%", f * 100.0)).small()));
                if ui.small_button("✕").on_hover_text("Cancelar el proxy").clicked() {
                    self.cancel_current();
                }
            }
            Some(ProxyState::Queued) => {
                ui.label(RichText::new("Proxy en cola").small().color(Color32::GRAY));
            }
            Some(ProxyState::Failed(e)) => {
                if ui.small_button("Reintentar proxy").on_hover_text(e).clicked() {
                    self.states.remove(&asset.path);
                    self.request(asset);
                }
            }
            None => {
                if ui.small_button("Crear proxy").on_hover_text("Copia 720p para que la vista previa vaya fluida").clicked() {
                    self.request(asset);
                }
            }
        }
    }
}
