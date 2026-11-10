# Snipsy

Editor de video no lineal escrito en Rust (egui + FFmpeg + wgpu).

## Requisitos (macOS)

```sh
brew install ffmpeg pkg-config cmake   # cmake: para compilar whisper.cpp
rustup update stable   # egui 0.37 requiere Rust ≥ 1.95
```

## Ejecutar

```sh
cargo run --release                    # abre la app
cargo run --release -- /ruta/video.mp4 # abre un video directamente
./scripts/make_app.sh --install        # arma Snipsy.app (ícono de Snipi) y la copia a /Applications
cargo test                             # pruebas (genera clips con ffmpeg)
cargo test --release -p snipsy-export -- --ignored --nocapture  # medir velocidad de exportación
cargo test --release -p snipsy-subtitles -- --ignored --nocapture  # transcripción real (descarga el modelo base)
```

## Atajos

| Tecla | Acción |
|---|---|
| Espacio / K | Reproducir / pausa |
| → / ← | Cuadro siguiente / anterior |
| Home / End | Inicio / final |
| A / B | Herramienta selección / cuchilla |
| T | Agregar texto en el cabezal |
| ⌘B | Cortar en el cabezal (clips seleccionados o todos) |
| Supr / ⌫ | Borrar selección (⇧ = borrado ripple, cierra el hueco) |
| N | Imán (snapping) |
| = / - / ⇧Z | Acercar / alejar / ajustar timeline |
| ⌘Z / ⇧⌘Z | Deshacer / rehacer |
| ⌘I / ⌘O / ⌘S | Importar / abrir proyecto / guardar |
| ⌘E | Exportar a MP4 |

En el timeline: arrastra un clip para moverlo (también entre pistas), arrastra sus
bordes para recortarlo, rueda/trackpad para desplazarte y pellizco o ⌘+rueda para zoom.
Los clips de video y su audio quedan vinculados y se editan juntos.

En el visor: haz clic en una capa para seleccionarla, arrástrala para moverla (se
imanta al centro) y arrastra sus esquinas para escalarla. El inspector (derecha)
ajusta posición, escala, rotación, opacidad y el estilo de los textos. Sin selección
muestra el formato del proyecto (16:9, 9:16, 1:1, 4:5…). «Rellenar» hace que un video
horizontal cubra un lienzo vertical.

Efectos (inspector, con un clip seleccionado):

- **Keyframes**: el rombo ◇ junto a cada propiedad crea un keyframe en el cuadro actual
  (◆ = hay uno aquí; clic para quitarlo, clic derecho para elegir la curva: suave,
  lineal o mantener). Con la propiedad animada, mover o escalar en el visor también
  crea/ajusta keyframes. Los rombos amarillos del timeline marcan los keyframes.
- **Animación rápida**: zoom lento (Ken Burns), entrar desde abajo y «pop».
- **Fundidos** de entrada/salida (a negro en video, a silencio en audio) y
  **disolvencia** con el clip anterior pegado en la misma pista (también funde el audio).
  Usa el material sobrante de ambos clips; si no hay, se acorta.
- **Color**: exposición, contraste, saturación, temperatura, tinte y LUT `.cube` con intensidad.
- **Audio**: volumen en dB (animable con keyframes), fundidos y fundido cruzado.
- **Ducking** (clip de música en su propia pista de audio): «Aplicar ducking» analiza
  las otras pistas, detecta cuándo se habla y baja la música (12 dB por defecto) con
  keyframes de volumen editables; sube sola en las pausas de más de medio segundo.
- **Velocidad**: 0,25× a 4× (cámara lenta / rápida). El clip cambia de duración, los
  siguientes se desplazan y el audio conserva el tono (filtro `atempo` de FFmpeg).

Subtítulos automáticos (**💬 Subtítulos**): transcribe el audio del timeline con Whisper
(whisper.cpp con Metal, todo local) y crea textos sincronizados por palabra en la pista
«Subtítulos». Estilo *Reel* (2–3 palabras grandes en mayúsculas) o *Clásico* (frases en
dos líneas). El modelo se descarga la primera vez en
`~/Library/Application Support/Snipsy/models`. Cada subtítulo es un texto editable;
«Aplicar estilo a todos los subtítulos» copia el formato de uno a toda la pista, y
«Guardar .srt…» exporta el archivo de subtítulos. Animaciones: resaltar la palabra que
se dice (color a elección), aparecer palabra a palabra y entrada con rebote; también
se pueden activar en cualquier texto desde el inspector («Animación de palabras»).

Marca (**🎨 Marca**): kits con el logo, los colores (principal, secundario, acento,
texto) y la tipografía de la empresa, guardados en
`~/Library/Application Support/Snipsy/brands` (con copia del logo y de la fuente).
Con un clic: **Poner logo** (pista «Logo», arriba de todo, en la esquina, tamaño y
opacidad elegidos), **Aplicar a todos los textos** (fuente y colores) e **Insertar**
textos prediseñados en el cabezal: título, rótulo con nombre y cargo, y llamada a la
acción con rebote. Opcionalmente, los subtítulos automáticos salen con la marca.
La fuente de cualquier texto se elige en el inspector (instaladas o archivo .ttf/.otf).
También se pueden importar imágenes (PNG, JPG, WebP) como clips, sin límite de duración.

Biblioteca (**📚 Biblioteca**, panel izquierdo): tu material de siempre, disponible en
todos los proyectos, en `~/Library/Application Support/Snipsy/library`. Categorías:
música, efectos de sonido, videos, imágenes, **Mis ediciones** (proyectos guardados,
que se abren o se usan como plantilla) y **Exportados** (cada video exportado se agrega
solo). «Agregar…» o «Carpeta…» copian los archivos (y detectan la categoría: un audio
de menos de 15 s es un efecto). Búsqueda por nombre o etiqueta (sin importar tildes),
favoritos ★, escucha previa ▶, miniaturas, «➕ Al cabezal» (la música va a la pista
«Música», los efectos a una pista de audio libre y videos/imágenes encima del video
principal) o arrastrar al timeline.

Formatos: al cambiar el formato del proyecto (16:9, 9:16, 1:1, 4:5, 4K) el contenido
se **reencuadra** solo: los videos a pantalla completa la siguen llenando (con el mismo
zoom relativo), y textos y logo conservan su tamaño respecto al lado corto y su
distancia al borde más cercano (⌘Z vuelve atrás). En **📤 Exportar** se pueden marcar
varios formatos: se exportan uno tras otro como `Nombre_9x16.mp4`, `Nombre_1x1.mp4`…

Snipi (✂, pestaña del panel izquierdo o ⌘K): la mascota de Snipsy. Entiende órdenes en
español y las ejecuta al momento, sin IA ni internet: «corta los silencios y pásalo a
vertical», «quita del 0:10 al 0:15», «título "OFERTA 2x1" al inicio», «blanco y negro»,
«velocidad 2x», «subtitula», «aplica mi marca», «exporta en 9:16 y 1:1», «deshaz»… Tolera
faltas de ortografía y acentos. Todo lo de un pedido se deshace con un solo ⌘Z, y exportar
siempre abre el diálogo para confirmar. «ayuda» muestra lo que sabe. Lo que no entiende lo
mandará a la IA (Claude Haiku) en la siguiente etapa. Las órdenes son acciones en JSON
(`core::actions`) y el intérprete está en `core::intent`.

Cortar silencios (✂, en el inspector sin nada seleccionado): quita las pausas de un
video hablado y cierra los huecos en todas las pistas (video, voz, textos, subtítulos),
conservando los vínculos video/audio. Ajustes: sensibilidad, pausa mínima y aire que se
deja antes y después de cada frase. La pista de música (se adivina por el nombre o por el
ducking) y el logo de marca no se cortan: siguen de corrido y se acortan al final. Solo
se corta donde hay audio (un plano mudo a propósito no se toca). ⌘Z lo deshace.

Seguimiento de rostros (🎯): cuando un video queda recortado (p. ej. 16:9 en 9:16), el
recorte sigue a la persona en vez de quedarse al centro. Se activa o desactiva por clip
(inspector, «Seguir rostro») y de forma general para el reencuadre (en el selector de
formato y en el diálogo de exportación). Usa Vision de Apple (en el Mac, sin internet):
analiza 5 cuadros por segundo en segundo plano (con el proxy si existe), elige el rostro
principal (sin saltar entre personas), suaviza el movimiento y nunca deja ver los
bordes del video. La trayectoria se guarda en el proyecto. La prueba de punta a punta
necesita una foto con un rostro: `SNIPSY_FACE_IMAGE=foto.jpg cargo test -p snipsy-faces --release`.

Proxies (**⚡ Proxies**): al importar videos de más de Full HD (4K, 1440p) se crea en
segundo plano una copia 720p con keyframes cada medio segundo, en
`~/Library/Application Support/Snipsy/proxies`. La vista previa la usa en cuanto está
lista (saltar en el timeline pasa de ~250 ms a ~10 ms por salto); el audio y la
exportación usan siempre el original. El media pool muestra el estado de cada proxy
(«Crear proxy» para hacerlo a mano con videos de 720p a 1080p). Si el archivo original
cambia, se vuelve a crear.

## Estructura

```
crates/
├── core/   # modelo: timeline (en cuadros), ediciones, deshacer, proyecto .snipsy
├── media/  # FFmpeg: lectores con seek preciso, motores de reproducción, mezclador, escritor MP4
├── audio/  # salida cpal + cola de muestras; el reloj de audio es el reloj maestro
├── render/ # compositor GPU (wgpu), escena compartida visor/exportación, texto (cosmic-text)
├── export/ # exportación MP4 en segundo plano (GPU sin ventana + mezcla + codificación)
├── faces/  # rostros con Vision (macOS) y trayectoria suavizada para el reencuadre
├── subtitles/ # Whisper (whisper.cpp + Metal): transcripción, agrupado en subtítulos, SRT
└── app/    # interfaz egui: media pool, visor, inspector, timeline y reproductor
```

## Hoja de ruta

1. ✅ Visor: abrir, reproducir con audio sincronizado, cuadro a cuadro, scrubbing
2. ✅ Línea de tiempo: pistas, cortar/recortar/mover, zoom, deshacer, proyectos
3. ✅ Composición en GPU (wgpu): capas, transformaciones, texto, formatos 9:16 / 1:1 / 16:9
4. ✅ Exportación MP4 (H.264/HEVC con VideoToolbox, AAC, BT.709, faststart)
5. ✅ Efectos y color: keyframes, fundidos, disolvencias, corrección de color, LUTs, volumen
6. ✅ Velocidad de clips (cámara lenta/rápida con tono de audio preservado)
7. ✅ Subtítulos automáticos (Whisper local, estilo reel o clásico, exportación SRT)
8. ✅ Subtítulos animados (palabra resaltada, palabra a palabra, rebote)
9. ✅ Kits de marca (logo, colores, tipografías, textos prediseñados), imágenes fijas
10. ✅ Proxies para editar 4K con fluidez
11. ✅ Ducking automático de la música bajo la voz
12. ✅ Reencuadre automático y exportación en varios formatos a la vez
13. ✅ Biblioteca personal (música, efectos, videos, imágenes, ediciones y exportados)
14. ✅ Seguimiento de rostros al reencuadrar (activable por clip y en general)
15. ✅ Cortar silencios automáticamente
16. ✅ Snipi: asistente con atajos en español (sin IA)
17. ⏳ Snipi con IA (Claude Haiku) para pedidos complejos
