//! A layer far larger than what it shows is processed only where it shows
//! ([`Renderer::shown_part`]), as After Effects renders a layer's region of interest: the same
//! frame as processing it whole, through its parent, scale and rotation, feathered masks and
//! effects that read around a pixel, at reduced resolutions and in a viewer region.

use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, FeatherFalloff, Footage, FootageKind, GroupKind, ItemId, ItemKind, Layer, LayerSource, MaskMode, Node, Project, Solid};
use effectcraft_raster::hash_noise;
use effectcraft_time::{FrameRate, Tick};

use crate::{FootageSource, Image, LayerCache, LayerTiming, RenderOpts, Renderer};

/// A large map: a fine texture with transparent holes (so masks, glow and blur all show).
struct Map;

impl FootageSource for Map {
    fn frame(&self, _: ItemId, f: &Footage, _: Tick) -> Option<Arc<Image>> {
        let mut img = Image::new(f.width, f.height);
        img.rows_mut().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                let (x, y) = (x as u32, y as u32);
                let a = if (x / 9 + y / 7) % 5 == 0 { 0.0 } else { 0.3 + 0.7 * hash_noise(x, y, 1) };
                *p = [hash_noise(x, y, 2) * a, hash_noise(x, y, 3) * a, 0.6 * a, a];
            }
        });
        Some(Arc::new(img))
    }
}

const W: u32 = 1200;
const H: u32 = 900;

fn add_effect(p: &mut Project, l: &mut Layer, id: &str, vals: &[(&str, Value)]) {
    let spec = effectcraft_effects::find(id).unwrap();
    let mut next = p.next_id;
    let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [W as f64, H as f64]);
    p.next_id = next;
    for (k, v) in vals {
        g.prop_mut(k).unwrap().value = v.clone();
    }
    l.props.sub_mut("effects").unwrap().children.push(g.into());
}

fn add_mask(p: &mut Project, l: &mut Layer, rect: ([f64; 2], f64, f64), mode: MaskMode, feather: [f64; 2], linear: bool) {
    let mut next = p.next_id;
    let mut m = build::mask(&mut Ids(&mut next), "Mask", ShapePath::rect(rect.0, rect.1, rect.2), mode, [255, 0, 0]);
    p.next_id = next;
    if let GroupKind::Mask { feather_falloff, .. } = &mut m.kind
        && linear
    {
        *feather_falloff = FeatherFalloff::Linear;
    }
    m.get_mut("feather").unwrap().value = Value::Vec2(feather);
    l.props.sub_mut("masks").unwrap().children.push(Node::Group(m));
}

/// A 160×120 comp with a 1200×900 map (or solid, `solid`) rotated 12° and parented to a null
/// whose scale goes from 145 % to 12 % over the first second: up close, a small part of it shows;
/// at the end, all of it. `dressed`: feathered masks (Gaussian and linear falloff) and Gaussian
/// Blur, Fast Box Blur and Glow.
fn project(solid: bool, dressed: bool) -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(160, 120, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let source = if solid {
        let sid = p.add_item("Big solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.9, 0.4, 0.2], width: W, height: H, pixel_aspect: 1.0 }));
        LayerSource::Solid { item: sid }
    } else {
        let f = Footage { path: "map.psd".into(), kind: FootageKind::Still, width: W, height: H, pixel_aspect: 1.0, has_video: true, ..Default::default() };
        LayerSource::Footage { item: p.add_item("map", Label::Aqua, None, ItemKind::Footage(f)) }
    };
    let mut cam = build::layer(&mut p, &comp, "camera", LayerSource::Null, (100, 100), None);
    cam.props.prop_mut("transform/scale").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([145.0, 145.0, 100.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([12.0, 12.0, 100.0]))];
    let mut map = build::layer(&mut p, &comp, "map", source, (W, H), None);
    map.parent = Some(cam.id);
    map.props.prop_mut("transform/rotation").unwrap().value = Value::Scalar(12.0);
    if dressed {
        // Edges inside the part shown up close (around map pixel (570, 440)).
        add_mask(&mut p, &mut map, ([600.0, 450.0], 120.0, 90.0), MaskMode::Add, [24.0, 16.0], false);
        add_mask(&mut p, &mut map, ([585.0, 440.0], 30.0, 30.0), MaskMode::Subtract, [10.0, 10.0], true);
        add_effect(&mut p, &mut map, "ec.blur.gaussian", &[("blurriness", Value::Scalar(6.0))]);
        add_effect(&mut p, &mut map, "ec.blur.fastbox", &[("radius", Value::Scalar(3.0)), ("iterations", Value::Scalar(2.0))]);
        add_effect(&mut p, &mut map, "ec.stylize.glow", &[("radius", Value::Scalar(10.0)), ("threshold", Value::Scalar(40.0))]);
    }
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    p.comp_mut(cid).unwrap().layers = vec![cam, map];
    (p, cid)
}

fn frame(p: &Project, cid: ItemId, t: f64, opts: RenderOpts) -> Image {
    Renderer::new(p, &Map, opts).comp_frame(cid, Tick::from_seconds_f64(t))
}

fn assert_same(a: &Image, b: &Image, what: &str) {
    assert_eq!((a.width, a.height), (b.width, b.height), "{what}");
    let worst = a.data.iter().zip(&b.data).map(|(p, q)| (0..4).map(|c| (p[c] - q[c]).abs()).fold(0.0f32, f32::max)).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "{what}: pixels differ by up to {worst}");
}

/// The map's shown part at time `t` (`None`: processed whole).
fn shown(p: &Project, cid: ItemId, t: f64, opts: RenderOpts) -> Option<[f64; 4]> {
    let r = Renderer::new(p, &Map, opts);
    let ctx = r.eval_ctx(cid, Tick::from_seconds_f64(t)).unwrap();
    r.shown_part(&ctx, &p.comp(cid).unwrap().layers[1])
}

#[test]
fn a_shown_part_renders_the_same_frame_as_the_whole_layer() {
    for (solid, dressed) in [(false, false), (false, true), (true, false), (true, true)] {
        let (p, cid) = project(solid, dressed);
        for scale in [1.0, 0.5, 1.0 / 3.0] {
            let opts = RenderOpts { scale, ..Default::default() };
            // Up close only part of the map shows; at the end all of it does.
            assert!(shown(&p, cid, 0.0, opts).is_some(), "solid {solid}, dressed {dressed}, scale {scale}");
            assert!(shown(&p, cid, 1.5, opts).is_none(), "solid {solid}, dressed {dressed}, scale {scale}");
            for t in [0.0, 0.3, 0.6, 1.5] {
                let what = format!("solid {solid}, dressed {dressed}, scale {scale}, t {t}");
                assert_same(&frame(&p, cid, t, opts), &frame(&p, cid, t, RenderOpts { whole_layers: true, ..opts }), &what);
            }
        }
        // A viewer region of interest shows less still.
        let roi = RenderOpts { roi: Some([30.0, 20.0, 80.0, 60.0]), ..Default::default() };
        assert_same(&frame(&p, cid, 0.0, roi), &frame(&p, cid, 0.0, RenderOpts { whole_layers: true, ..roi }), "region of interest");
    }
}

#[test]
fn a_layer_off_the_frame_or_with_effects_that_read_anywhere_is_whole_or_nothing() {
    // Off the frame: nothing of it is processed, and nothing shows either way.
    let (mut p, cid) = project(false, true);
    let map = &mut p.comp_mut(cid).unwrap().layers[1];
    map.props.prop_mut("transform/position").unwrap().value = Value::Vec3([5000.0, 5000.0, 0.0]);
    let opts = RenderOpts::default();
    assert!(shown(&p, cid, 0.0, opts).is_some());
    assert_same(&frame(&p, cid, 0.0, opts), &frame(&p, cid, 0.0, RenderOpts { whole_layers: true, ..opts }), "off the frame");
    // An effect that may read anywhere (Twirl moves pixels across the layer): processed whole.
    let (mut p, cid) = project(false, false);
    let mut map = p.comp(cid).unwrap().layers[1].clone();
    add_effect(&mut p, &mut map, "ec.distort.twirl", &[]);
    p.comp_mut(cid).unwrap().layers[1] = map;
    assert!(shown(&p, cid, 0.0, opts).is_none());
}

/// Whether the map came from the layer cache in a render of frame `t` with `cache`.
fn map_cached(p: &Project, cid: ItemId, t: f64, cache: &LayerCache) -> bool {
    let prof = Mutex::new(Vec::<LayerTiming>::new());
    let mut r = Renderer::new(p, &Map, RenderOpts::default());
    r.cache = Some(cache);
    r.profile = Some(&prof);
    r.comp_frame(cid, Tick::from_seconds_f64(t));
    prof.into_inner().unwrap().iter().any(|l| l.layer == "map" && l.cached)
}

/// A still footage layer is cached like a solid (it was decoded and processed again every frame);
/// a shown part only while the layer holds still on the output: a moving layer's parts are never
/// asked for again.
#[test]
fn a_still_layer_is_cached_and_its_part_while_it_holds_still() {
    let (mut p, cid) = project(false, true);
    // Moving (the parent scales down): its parts are not kept.
    let cache = LayerCache::default();
    assert!(!map_cached(&p, cid, 0.3, &cache) && !map_cached(&p, cid, 0.3, &cache));
    // All of it shows and it holds still (after the last key): kept whole.
    assert!(!map_cached(&p, cid, 1.5, &cache) && map_cached(&p, cid, 1.5, &cache));
    // Up close and holding still: its part is kept.
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("transform/scale").unwrap().keys.clear();
    p.comp_mut(cid).unwrap().layers[0].props.prop_mut("transform/scale").unwrap().value = Value::Vec3([145.0, 145.0, 100.0]);
    let cache = LayerCache::default();
    assert!(shown(&p, cid, 0.3, RenderOpts::default()).is_some());
    assert!(!map_cached(&p, cid, 0.3, &cache) && map_cached(&p, cid, 0.3, &cache));
}
