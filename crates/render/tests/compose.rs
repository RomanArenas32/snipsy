//! Pruebas del compositor en una GPU sin ventana.

use snipsy_render::*;

fn pixel(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

#[test]
fn composes_layers_with_position_and_opacity() {
    let Some((device, queue)) = headless_device() else {
        eprintln!("sin GPU; se omite");
        return;
    };
    let mut comp = Compositor::new(&device, &queue);
    let red = vec![255u8, 0, 0, 255].repeat(4);
    let green = vec![0u8, 255, 0, 255].repeat(4);
    // Rojo cubre la mitad izquierda; verde al 50% sobre el cuarto superior derecho.
    let layers = [
        Layer {
            source: LayerSource { key: 1, version: 1, width: 2, height: 2, rgba: &red },
            corners: [[0.0, 0.0], [32.0, 0.0], [32.0, 64.0], [0.0, 64.0]],
            opacity: 1.0,
            grade: Grade::NEUTRAL,
            lut: None,
        },
        Layer {
            source: LayerSource { key: 2, version: 1, width: 2, height: 2, rgba: &green },
            corners: [[32.0, 0.0], [64.0, 0.0], [64.0, 32.0], [32.0, 32.0]],
            opacity: 0.5,
            grade: Grade::NEUTRAL,
            lut: None,
        },
    ];
    assert!(comp.render((64, 64), &layers));
    let img = comp.read_canvas().unwrap();
    assert_eq!(img.len(), 64 * 64 * 4);
    assert_eq!(pixel(&img, 64, 10, 40), [255, 0, 0, 255]);
    let g = pixel(&img, 64, 48, 10);
    assert!(g[0] < 5 && (120..=135).contains(&g[1]), "verde al 50%: {g:?}");
    assert_eq!(pixel(&img, 64, 48, 48), [0, 0, 0, 255], "fondo negro");
    // Mismo tamaño: no se recrea el lienzo.
    assert!(!comp.render((64, 64), &layers[..1]));
}

#[test]
fn rasterizes_text_with_outline() {
    let mut tr = TextRenderer::new();
    let r = tr.render(&TextParams {
        text: "Hola\nmundo",
        size_px: 48.0,
        bold: true,
        font: None,
        font_file: None,
        color: [255, 255, 255, 255],
        outline: Some([0, 0, 0, 255]),
        background: None,
        word_colors: None,
    });
    assert!(r.width > 80 && r.height > 100, "{}x{}", r.width, r.height);
    let opaque = r.rgba.chunks(4).filter(|p| p[3] > 200).count();
    let white = r.rgba.chunks(4).filter(|p| p[3] > 200 && p[0] > 200).count();
    let black = r.rgba.chunks(4).filter(|p| p[3] > 200 && p[0] < 50).count();
    assert!(opaque > 500 && white > 100 && black > 100, "opaque {opaque} white {white} black {black}");
}

#[test]
fn applies_color_grade_and_lut() {
    let Some((device, queue)) = headless_device() else {
        eprintln!("sin GPU; se omite");
        return;
    };
    let mut comp = Compositor::new(&device, &queue);
    let color = vec![200u8, 60, 30, 255].repeat(4);
    let render = |comp: &mut Compositor, grade: Grade, lut: Option<&Lut>| {
        let layer = Layer {
            source: LayerSource { key: 1, version: 1, width: 2, height: 2, rgba: &color },
            corners: [[0.0, 0.0], [16.0, 0.0], [16.0, 16.0], [0.0, 16.0]],
            opacity: 1.0,
            grade,
            lut: lut.map(|l| LutSource { key: 7, size: l.size, rgba: &l.rgba }),
        };
        comp.render((16, 16), &[layer]);
        pixel(&comp.read_canvas().unwrap(), 16, 8, 8)
    };

    let neutral = render(&mut comp, Grade::NEUTRAL, None);
    for (a, b) in neutral.iter().zip([200, 60, 30, 255]) {
        assert!((*a as i32 - b).abs() <= 1, "neutro: {neutral:?}");
    }
    let gray = render(&mut comp, Grade { saturation: 0.0, ..Grade::NEUTRAL }, None);
    assert!(gray[0] == gray[1] && gray[1] == gray[2], "sin saturación: {gray:?}");
    let bright = render(&mut comp, Grade { exposure: 1.0, ..Grade::NEUTRAL }, None);
    assert!(bright[0] > 245 && bright[1] > 75, "exposición +1: {bright:?}");
    let warm = render(&mut comp, Grade { temperature: 1.0, ..Grade::NEUTRAL }, None);
    assert!(warm[2] < 30, "cálido: {warm:?}");

    // LUT que invierte los colores, al 100 % y al 50 %.
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for b in [1.0, 0.0] {
        for g in [1.0, 0.0] {
            for r in [1.0, 0.0] {
                text += &format!("{r} {g} {b}\n");
            }
        }
    }
    let invert = Lut::parse(&text).unwrap();
    let inv = render(&mut comp, Grade { lut_mix: 1.0, ..Grade::NEUTRAL }, Some(&invert));
    for (a, b) in inv.iter().zip([55, 195, 225]) {
        assert!((*a as i32 - b).abs() <= 3, "LUT invertida: {inv:?}");
    }
    let half = render(&mut comp, Grade { lut_mix: 0.5, ..Grade::NEUTRAL }, Some(&invert));
    assert!((half[0] as i32 - 128).abs() <= 3, "LUT al 50 %: {half:?}");
}

#[test]
fn colors_each_word() {
    let mut tr = TextRenderer::new();
    let params = |colors| TextParams {
        text: "AAA BBB",
        size_px: 40.0,
        bold: true,
        font: None,
        font_file: None,
        color: [255, 255, 255, 255],
        outline: Some([0, 0, 0, 255]),
        background: None,
        word_colors: colors,
    };
    let plain = tr.render(&params(None));
    let yellow = [[255, 255, 255, 255], [255, 214, 10, 255]];
    let hl = tr.render(&params(Some(&yellow)));
    let hidden = [[255, 255, 255, 255], [0, 0, 0, 0]];
    let rev = tr.render(&params(Some(&hidden)));
    // Mismo tamaño: el texto no se mueve al cambiar los colores.
    assert_eq!((plain.width, plain.height), (hl.width, hl.height));
    assert_eq!((plain.width, plain.height), (rev.width, rev.height));
    let half = |r: &TextRaster, right: bool, f: &dyn Fn(&[u8]) -> bool| {
        let w = r.width as usize;
        // Zonas claramente de una u otra palabra (el centro es el espacio entre ambas).
        let zone = |x: usize| if right { x >= w * 6 / 10 } else { x < w * 4 / 10 };
        r.rgba.chunks(4).enumerate().filter(|(i, p)| zone(i % w) && f(p)).count()
    };
    let is_yellow = |p: &[u8]| p[3] > 200 && p[0] > 200 && p[2] < 80;
    let is_opaque = |p: &[u8]| p[3] > 200;
    assert_eq!(half(&hl, false, &is_yellow), 0, "la primera palabra no se resalta");
    assert!(half(&hl, true, &is_yellow) > 50, "la segunda sí");
    assert!(half(&rev, false, &is_opaque) > 50);
    assert_eq!(half(&rev, true, &is_opaque), 0, "palabra oculta, sin contorno");
}

#[test]
fn loads_font_from_file() {
    let file = std::path::Path::new("/System/Library/Fonts/Supplemental/Courier New.ttf");
    if !file.exists() {
        eprintln!("sin Courier New; se omite");
        return;
    }
    assert_eq!(font_file_family(file).as_deref(), Some("Courier New"));
    let mut tr = TextRenderer::new();
    let params = |font_file| TextParams {
        text: "iiiiiiii",
        size_px: 40.0,
        bold: false,
        font: None,
        font_file,
        color: [255, 255, 255, 255],
        outline: None,
        background: None,
        word_colors: None,
    };
    let sans = tr.render(&params(None));
    let mono = tr.render(&params(Some(file)));
    // En una fuente monoespaciada la «i» ocupa lo mismo que una «m»: mucho más ancha.
    assert!(mono.width > sans.width * 2, "sans {} vs mono {}", sans.width, mono.width);
}

#[test]
fn draws_still_image_logo() {
    let Some((device, queue)) = headless_device() else {
        eprintln!("sin GPU; se omite");
        return;
    };
    use snipsy_core::{Asset, AssetId, BrandKit, Corner, Timeline};
    let dir = std::env::temp_dir().join(format!("snipsy_logo_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("logo.png");
    image::RgbaImage::from_pixel(100, 50, image::Rgba([255, 0, 0, 255])).save(&png).unwrap();
    let img = load_image(&png).unwrap();
    assert_eq!((img.width, img.height), (100, 50));

    let mut tl = Timeline::new();
    tl.width = 200;
    tl.height = 200;
    let a = tl.add_asset(Asset {
        id: AssetId(0),
        path: png.clone(),
        name: "logo.png".into(),
        duration: 5.0,
        fps: 30.0,
        width: 100,
        height: 50,
        has_video: true,
        has_audio: false,
        still: true,
        faces: None,
    });
    // Logo de 40 px de ancho (20 %) a 10 px (5 %) de la esquina superior izquierda.
    let kit = BrandKit { logo_corner: Corner::TopLeft, logo_size: 0.2, margin: 0.05, ..Default::default() };
    kit.place_logo(&mut tl, a).unwrap();
    assert_eq!(tl.video_tracks.len(), 2);

    let mut scene = Scene::new();
    let layers = scene.layers(&tl, 10, &Default::default(), 1.0);
    assert_eq!(layers.len(), 1, "la imagen fija no necesita al motor de video");
    let gpu = Scene::gpu_layers(&layers, &tl, 1.0);
    let mut comp = Compositor::new(&device, &queue);
    comp.render((200, 200), &gpu);
    let out = comp.read_canvas().unwrap();
    assert_eq!(pixel(&out, 200, 30, 20), [255, 0, 0, 255], "dentro del logo");
    assert_eq!(pixel(&out, 200, 5, 5)[0], 0, "margen");
    assert_eq!(pixel(&out, 200, 55, 20)[0], 0, "a la derecha del logo");
    assert_eq!(pixel(&out, 200, 30, 35)[0], 0, "debajo del logo (alto 20 px)");
    std::fs::remove_dir_all(&dir).unwrap();
}
