use lt_core::screen::imaging::{
    arithmetic::{self, Operation},
    color_conversion, conversion, distance_transform, filters, inpaint,
    morphology::{self, StructuringElement},
    resize::{self, Interpolation},
    statistics, threshold, ImageBuffer, PixelType,
};
use lt_core::screen::{
    erase::{self, TextRegion},
    geom::RectF,
    paint::backdrop::{self, Backdrop},
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

struct Random64(u64);
impl Random64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, below: usize) -> usize {
        (self.next() % below as u64) as usize
    }
}
fn random_bytes(w: usize, h: usize, p: PixelType, seed: u64, below: usize) -> ImageBuffer {
    let mut random = Random64(seed);
    let image = ImageBuffer::new(w, h, p);
    for y in 0..h {
        for x in 0..image.row_bytes() {
            image.set_byte(y, x, random.below(below) as u8);
        }
    }
    image
}
fn random_floats(w: usize, h: usize, p: PixelType, seed: u64, below: usize) -> ImageBuffer {
    let mut random = Random64(seed);
    let image = ImageBuffer::new(w, h, p);
    for y in 0..h {
        for x in 0..w * p.channels() {
            image.set_float(y, x, random.below(below) as f32 / 64.0);
        }
    }
    image
}
fn random(w: usize, h: usize, p: PixelType, seed: u64) -> ImageBuffer {
    if p.is_float() {
        random_floats(w, h, p, seed, 255 * 64)
    } else {
        random_bytes(w, h, p, seed, 256)
    }
}
fn doubles(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn routines() -> Vec<(String, Vec<u8>)> {
    let mut cases = Vec::new();
    for (name, w, h, p, seed) in [
        ("gray/u8", 37, 5, PixelType::U8C3, 101),
        ("gray/f32", 29, 4, PixelType::F32C3, 102),
    ] {
        cases.push((
            name.into(),
            color_conversion::to_gray(&random(w, h, p, seed)).bytes(),
        ));
    }
    for size in [5, 25] {
        let parent = random_bytes(160, 90, PixelType::U8C1, 103 + size as u64, 256);
        let v = parent.view(17, 9, 120, 60);
        let element = StructuringElement::ellipse(size, size);
        let light = ImageBuffer::new(v.width, v.height, v.pixel_type);
        let dark = ImageBuffer::new(v.width, v.height, v.pixel_type);
        morphology::top_hat(&v, &light, &element);
        morphology::black_hat(&v, &dark, &element);
        cases.push((
            format!("hats/ellipse{size}-view"),
            [light.bytes(), dark.bytes()].concat(),
        ));
    }
    {
        let parent = random_bytes(70, 50, PixelType::U8C1, 105, 2);
        let v = parent.view(0, 3, 64, 40);
        let target = ImageBuffer::new(v.width, v.height, v.pixel_type);
        morphology::dilate(&v, &target, &StructuringElement::ellipse(3, 3), 2);
        cases.push(("dilate/cross-twice-view".into(), target.bytes()));
    }
    {
        let source = random_floats(53, 31, PixelType::F32C1, 106, 255 * 64);
        let eroded = ImageBuffer::new(53, 31, PixelType::F32C1);
        let dilated = ImageBuffer::new(53, 31, PixelType::F32C1);
        morphology::erode(&source, &eroded, &StructuringElement::rectangle(3, 3), 1);
        morphology::dilate(&source, &dilated, &StructuringElement::rectangle(13, 13), 1);
        cases.push((
            "morphology/f32-rect".into(),
            [eroded.bytes(), dilated.bytes()].concat(),
        ));
    }
    for (name, p, from, to, how) in [
        (
            "area-u8c3-fraction",
            PixelType::U8C3,
            (101, 37),
            (50, 18),
            Interpolation::Area,
        ),
        (
            "area-u8c1-half",
            PixelType::U8C1,
            (66, 34),
            (33, 17),
            Interpolation::Area,
        ),
        (
            "area-f32c3-half",
            PixelType::F32C3,
            (46, 24),
            (23, 12),
            Interpolation::Area,
        ),
        (
            "area-f32c1-fraction",
            PixelType::F32C1,
            (47, 25),
            (15, 8),
            Interpolation::Area,
        ),
        (
            "linear-u8c3-double",
            PixelType::U8C3,
            (45, 21),
            (90, 42),
            Interpolation::Linear,
        ),
        (
            "linear-u8c1-fraction",
            PixelType::U8C1,
            (31, 17),
            (77, 40),
            Interpolation::Linear,
        ),
        (
            "linear-f32c3-up",
            PixelType::F32C3,
            (12, 7),
            (23, 13),
            Interpolation::Linear,
        ),
        (
            "nearest-f32c1-down",
            PixelType::F32C1,
            (57, 41),
            (14, 10),
            Interpolation::Nearest,
        ),
        (
            "nearest-u8c3-down",
            PixelType::U8C3,
            (150, 61),
            (25, 15),
            Interpolation::Nearest,
        ),
    ] {
        let source = random(from.0, from.1, p, 107);
        cases.push((
            format!("resize/{name}"),
            resize::to(&source, to.0, to.1, how).bytes(),
        ));
    }
    {
        let parent = random_bytes(60, 40, PixelType::U8C1, 108, 256);
        let v = parent.view(1, 2, 50, 30);
        let target = ImageBuffer::new(50, 30, PixelType::U8C1);
        filters::blur(&v, &target, 5);
        cases.push(("blur/u8-5-view".into(), target.bytes()));
    }
    {
        let image = random_floats(41, 23, PixelType::F32C1, 109, 255 * 64);
        filters::blur(&image, &image, 3);
        cases.push(("blur/f32-3-in-place".into(), image.bytes()));
    }
    {
        let parent = random_bytes(90, 70, PixelType::U8C1, 110, 256);
        cases.push((
            "boxsum/33-view".into(),
            filters::box_sum(&parent.view(20, 10, 50, 40), 33).bytes(),
        ));
    }
    cases.push((
        "laplacian/f32".into(),
        filters::laplacian(&random_floats(33, 19, PixelType::F32C1, 111, 255 * 64)).bytes(),
    ));
    for (name, sigma) in [("1", 1.0), ("4.3", 4.3), ("16", 16.0)] {
        cases.push((
            format!("gaussian/sigma{name}"),
            filters::gaussian_blur(&random_bytes(60, 40, PixelType::U8C3, 112, 256), sigma).bytes(),
        ));
    }
    {
        let source = random_bytes(57, 23, PixelType::U8C1, 113, 90);
        let otsu = threshold::otsu(&source);
        let target = ImageBuffer::new(57, 23, PixelType::U8C1);
        threshold::binary(&source, &target, 18.0f64.max(otsu * 0.8) + 0.9, 255.0);
        cases.push((
            "threshold/otsu-and-binary".into(),
            [doubles(&[otsu]), target.bytes()].concat(),
        ));
    }
    {
        let source = random_floats(40, 9, PixelType::F32C1, 114, 64);
        let target = ImageBuffer::new(40, 9, PixelType::F32C1);
        threshold::binary(&source, &target, 0.9, 1.0);
        cases.push(("threshold/f32".into(), target.bytes()));
    }
    {
        let mask = random_bytes(48, 36, PixelType::U8C1, 115, 8);
        for y in 0..36 {
            for x in 0..48 {
                mask.set_byte(
                    y,
                    x,
                    if mask.byte(y, x) < 6 || (x > 10 && x < 30 && y > 5) {
                        255
                    } else {
                        0
                    },
                );
            }
        }
        let distance = distance_transform::chamfer3(&mask);
        let mut rounded = Vec::new();
        for y in 0..36 {
            for x in 0..48 {
                let v = distance.float(y, x);
                assert!((v - 5.0).abs() >= 1e-3);
                rounded.push((f64::from(v) * 1000.0).round_ties_even() / 1000.0);
            }
        }
        cases.push(("distance/chamfer3".into(), doubles(&rounded)));
    }
    {
        let image = ImageBuffer::new(7, 9, PixelType::U8C1);
        image.set_to(250.0, None);
        let mask = ImageBuffer::new(7, 9, PixelType::U8C1);
        mask.view(0, 0, 7, 7).set_to(255.0, None);
        let source = random_bytes(23, 11, PixelType::U8C1, 116, 256);
        let where_ = random_bytes(23, 11, PixelType::U8C1, 117, 2);
        let (least, most) = statistics::min_max(&source, Some(&where_));
        cases.push((
            "statistics/means-on-ties".into(),
            doubles(&[
                statistics::mean(&image, Some(&mask)),
                statistics::mean(&image, None),
                statistics::mean(&source, Some(&where_)),
                least,
                most,
                statistics::count_non_zero(&where_) as f64,
                statistics::bounding_rect(&where_).2 as f64,
            ]),
        ));
    }
    {
        let bytes = random_bytes(19, 7, PixelType::U8C3, 118, 256);
        let floats = ImageBuffer::new(19, 7, PixelType::F32C3);
        let mut random = Random64(119);
        for y in 0..7 {
            for x in 0..19 * 3 {
                floats.set_float(y, x, (random.below(41) as i32 - 20) as f32 * 0.5);
            }
        }
        let target = ImageBuffer::new(19, 7, PixelType::U8C3);
        arithmetic::add_to_bytes(&bytes, &floats, &target);
        cases.push(("arithmetic/add-to-bytes-on-halves".into(), target.bytes()));
    }
    {
        let a = random_floats(27, 5, PixelType::F32C1, 120, 255 * 64);
        let b = random_floats(27, 5, PixelType::F32C1, 121, 255 * 64);
        let t = ImageBuffer::new(27, 5, PixelType::F32C1);
        arithmetic::binary(&a, &b, &t, Operation::Subtract);
        arithmetic::scalar(&t, 0.0, &t, Operation::Max);
        arithmetic::scalar(&t, 9.0 * 0.7979, &t, Operation::Min);
        arithmetic::scalar(&t, 0.7979, &t, Operation::Divide);
        arithmetic::binary(&t, &a, &t, Operation::Multiply);
        arithmetic::scalar(&b, 1e-3, &b, Operation::Max);
        arithmetic::divide(&a, &b, &a, 255.0);
        arithmetic::scalar(&a, 1.0, &a, Operation::ReverseSubtract);
        arithmetic::binary(&a, &t, &a, Operation::Add);
        cases.push(("arithmetic/float-chain".into(), a.bytes()));
    }
    {
        let floats = ImageBuffer::new(16, 3, PixelType::F32C1);
        for x in 0..16 {
            floats.set_float(0, x, x as f32 * 0.5 - 1.0);
            floats.set_float(1, x, x as f32 / 15.0);
            floats.set_float(2, x, 250.0 + x as f32 * 0.5);
        }
        let plain = conversion::convert(&floats, PixelType::U8C1, 1.0);
        let scaled = conversion::convert(&floats, PixelType::U8C1, 255.0);
        let bytes = random_bytes(16, 3, PixelType::U8C1, 122, 256);
        let back = conversion::convert(&bytes, PixelType::F32C1, 1.0 / 255.0);
        cases.push((
            "conversion/on-halves".into(),
            [plain.bytes(), scaled.bytes(), back.bytes()].concat(),
        ));
    }
    {
        let source = random_bytes(40, 30, PixelType::U8C3, 123, 256);
        let mask = ImageBuffer::new(40, 30, PixelType::U8C1);
        for y in 0..30 {
            for x in 0..40 {
                mask.set_byte(
                    y,
                    x,
                    if (x < 4 && y > 10 && y < 20)
                        || (x > 12 && x < 26 && y > 8 && y < 14)
                        || (y == 29 && x > 30)
                    {
                        255
                    } else {
                        0
                    },
                );
            }
        }
        cases.push((
            "inpaint/against-edges".into(),
            inpaint::navier_stokes(&source, &mask, 3.0).bytes(),
        ));
    }
    cases
}
#[test]
fn imaging_routines_match_recorded_hashes() {
    let expected: BTreeMap<String, String> =
        serde_json::from_str(include_str!("fixtures/screen/imaging-golden.json")).unwrap();
    let cases = routines();
    assert_eq!(cases.len(), 30);
    let mut failures = Vec::new();
    for (name, bytes) in cases {
        let hash = format!("{:X}", Sha256::digest(&bytes));
        if expected[&name] != hash {
            failures.push(format!("{name}: {hash} != {}", expected[&name]));
        }
        println!("{name}: {hash}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn picture(width: usize, height: usize, seed: u64, grain: i32, flat: bool) -> ImageBuffer {
    let mut random = Random64(seed);
    let image = ImageBuffer::new(width, height, PixelType::U8C3);
    let phase = random.below(1024) as i32;
    for y in 0..height {
        for x in 0..width {
            let across = ((x as i32 * 4 + phase) % 320 - 160).abs() - 80;
            let down = ((y as i32 * 5 + phase) % 192 - 96).abs() - 48;
            let ramp =
                x as i32 * 60 / (width as i32).max(1) + y as i32 * 40 / (height as i32).max(1);
            let wave = if flat { 0 } else { across / 2 + down / 2 };
            for c in 0..3 {
                let noise = if grain > 0 {
                    random.below((grain * 2 + 1) as usize) as i32 - grain
                } else {
                    0
                };
                image.set_byte(
                    y,
                    x * 3 + c,
                    (70 + c as i32 * 25 + ramp + wave + noise).clamp(0, 255) as u8,
                );
            }
        }
    }
    image
}
fn glyphs(image: &ImageBuffer, line: (i32, i32, i32, i32), glyph: i32, bright: bool, seed: u64) {
    let mut random = Random64(seed);
    let stroke = 2.max(glyph / 6);
    let outline = 1.max(glyph / 12);
    let (body, edge) = if bright { (245, 15) } else { (20, 235) };
    let mut bars = Vec::new();
    let mut x = line.0;
    while x + stroke < line.0 + line.2 {
        let height = glyph / 2 + random.below((glyph / 2 + 1) as usize) as i32;
        let top = line.1 + (line.3 - height) / 2;
        bars.push((x, top, stroke, height));
        if random.below(3) == 0 {
            bars.push((x, top + height / 3, stroke * 3, stroke));
        }
        x += stroke * 2 + random.below((stroke * 3 + 1) as usize) as i32;
    }
    for y in 0.max(line.1 - glyph)..(image.height as i32).min(line.1 + line.3 + glyph) {
        for x in 0.max(line.0 - glyph)..(image.width as i32).min(line.0 + line.2 + glyph) {
            let (mut covered, mut ringed) = (0, 0);
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x * 4 + sx;
                    let py = y * 4 + sy;
                    let (mut in_body, mut in_edge) = (false, false);
                    for b in &bars {
                        if px >= b.0 * 4
                            && px < (b.0 + b.2) * 4
                            && py >= b.1 * 4
                            && py < (b.1 + b.3) * 4
                        {
                            in_body = true;
                        } else if px >= (b.0 - outline) * 4
                            && px < (b.0 + b.2 + outline) * 4
                            && py >= (b.1 - outline) * 4
                            && py < (b.1 + b.3 + outline) * 4
                        {
                            in_edge = true;
                        }
                    }
                    if in_body {
                        covered += 1;
                    } else if in_edge {
                        ringed += 1;
                    }
                }
            }
            if covered + ringed == 0 {
                continue;
            }
            for c in 0..3 {
                let at = x as usize * 3 + c;
                image.set_byte(
                    y as usize,
                    at,
                    ((i32::from(image.byte(y as usize, at)) * (16 - covered - ringed)
                        + body * covered
                        + edge * ringed
                        + 8)
                        / 16) as u8,
                );
            }
        }
    }
}
type Stroke = ((i32, i32, i32, i32), i32, bool);
struct Scene {
    name: &'static str,
    width: usize,
    height: usize,
    seed: u64,
    grain: i32,
    flat: bool,
    lines: Vec<Stroke>,
}
fn scenes() -> Vec<Scene> {
    vec![
        Scene {
            name: "subtitle-bright",
            width: 640,
            height: 220,
            seed: 1,
            grain: 6,
            flat: false,
            lines: vec![((150, 140, 340, 44), 40, true)],
        },
        Scene {
            name: "subtitle-dark",
            width: 640,
            height: 220,
            seed: 2,
            grain: 6,
            flat: false,
            lines: vec![((120, 30, 380, 40), 36, false)],
        },
        Scene {
            name: "two-lines",
            width: 640,
            height: 220,
            seed: 3,
            grain: 4,
            flat: false,
            lines: vec![
                ((100, 90, 420, 34), 30, true),
                ((130, 136, 360, 34), 30, true),
            ],
        },
        Scene {
            name: "panel-small",
            width: 420,
            height: 200,
            seed: 4,
            grain: 3,
            flat: false,
            lines: vec![
                ((20, 20, 300, 16), 14, true),
                ((20, 42, 260, 16), 14, true),
                ((20, 64, 330, 16), 14, false),
                ((20, 120, 200, 16), 14, false),
            ],
        },
        Scene {
            name: "display-large",
            width: 640,
            height: 300,
            seed: 5,
            grain: 5,
            flat: false,
            lines: vec![((60, 60, 500, 130), 110, true)],
        },
        Scene {
            name: "against-edge",
            width: 400,
            height: 160,
            seed: 6,
            grain: 6,
            flat: false,
            lines: vec![
                ((-10, 120, 260, 44), 40, true),
                ((250, -6, 170, 30), 26, false),
            ],
        },
        Scene {
            name: "slope",
            width: 640,
            height: 220,
            seed: 7,
            grain: 0,
            flat: true,
            lines: vec![((150, 90, 340, 40), 36, true)],
        },
        Scene {
            name: "busy",
            width: 480,
            height: 200,
            seed: 8,
            grain: 40,
            flat: false,
            lines: vec![((80, 80, 300, 40), 36, false)],
        },
        Scene {
            name: "dense",
            width: 520,
            height: 240,
            seed: 9,
            grain: 2,
            flat: false,
            lines: vec![
                ((40, 40, 440, 44), 44, true),
                ((40, 86, 440, 44), 44, true),
                ((40, 132, 440, 44), 44, true),
            ],
        },
        Scene {
            name: "far-apart",
            width: 1100,
            height: 320,
            seed: 10,
            grain: 5,
            flat: false,
            lines: vec![
                ((30, 30, 200, 30), 26, true),
                ((800, 260, 260, 34), 30, false),
            ],
        },
        Scene {
            name: "tiny",
            width: 3,
            height: 3,
            seed: 11,
            grain: 0,
            flat: false,
            lines: vec![((0, 0, 3, 3), 3, true)],
        },
    ]
}
fn run_scene(scene: &Scene) -> Vec<u8> {
    let picture = picture(
        scene.width,
        scene.height,
        scene.seed,
        scene.grain,
        scene.flat,
    );
    let mut regions = Vec::new();
    for (index, (line, glyph, bright)) in scene.lines.iter().enumerate() {
        glyphs(
            &picture,
            *line,
            *glyph,
            *bright,
            scene.seed * 31 + index as u64,
        );
        regions.push(TextRegion {
            bounds: RectF::new(
                f64::from(line.0) - 2.5,
                f64::from(line.1) - 1.75,
                f64::from(line.2) + 5.0,
                f64::from(line.3) + 3.5,
            ),
            glyph_height: Some(f64::from(*glyph)),
            within: None,
        });
    }
    let mask = erase::build_mask(&picture, &regions);
    let repaired = erase::repair(&picture, &mask);
    let mut output = repaired.image.bytes();
    let backdrop = Backdrop::from_repaired(&repaired.image).unwrap();
    let washes = [[0, 0, 0], [255, 255, 255], [40, 90, 160]];
    let texts = [[255, 255, 255], [0, 0, 0], [250, 220, 90]];
    for (i, b) in regions.iter().enumerate() {
        let glyph = b.glyph_height.unwrap_or(b.bounds.height);
        let feather = backdrop::feather(glyph);
        let area = RectF::new(
            b.bounds.x - feather,
            b.bounds.y - feather,
            b.bounds.width + feather * 2.0,
            b.bounds.height + feather * 2.0,
        );
        if let Some(plate) = backdrop.plate(
            area,
            washes[i % 3],
            texts[i % 3],
            glyph,
            b.bounds.height,
            None,
        ) {
            output.extend_from_slice(&(plate.width as i32).to_le_bytes());
            output.extend_from_slice(&(plate.height as i32).to_le_bytes());
            output.extend_from_slice(&plate.text);
            output.extend_from_slice(&plate.bgra);
        } else {
            output.push(0);
        }
    }
    output
}
#[test]
fn repair_scenes_match_recorded_hashes() {
    let expected: BTreeMap<String, String> =
        serde_json::from_str(include_str!("fixtures/screen/imaging-golden.json")).unwrap();
    let mut failures = Vec::new();
    for scene in scenes() {
        let bytes = run_scene(&scene);
        let name = format!("scene/{}", scene.name);
        let hash = format!("{:X}", Sha256::digest(&bytes));
        println!("{name}: {hash}");
        if expected[&name] != hash {
            failures.push(format!("{name}: {hash} != {}", expected[&name]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn every_recorded_imaging_case_still_exists() {
    let expected: BTreeMap<String, String> =
        serde_json::from_str(include_str!("fixtures/screen/imaging-golden.json")).unwrap();
    let names: Vec<_> = routines()
        .into_iter()
        .map(|(name, _)| name)
        .chain(scenes().iter().map(|scene| format!("scene/{}", scene.name)))
        .collect();
    assert_eq!(names.len(), 41);
    for name in expected.keys() {
        assert!(names.contains(name), "{name}");
    }
}

#[test]
fn repair_and_plate_pixels_are_identical_with_serial_and_parallel_rows() {
    use lt_core::screen::imaging::parallel_rows::with_threads;
    for scene in scenes() {
        let serial = with_threads(1, || run_scene(&scene));
        let parallel = with_threads(4, || run_scene(&scene));
        assert_eq!(serial, parallel, "{}", scene.name);
    }
}
