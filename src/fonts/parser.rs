//! ttf-parser 0.25 exposes fvar axes but not named instances, so we walk
//! the raw table bytes ourselves (`parse_fvar_instances`).

use super::{AxisInfo, FaceInfo};
use anyhow::Result;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub(super) fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("ttf") | Some("otf") | Some("ttc") | Some("otc") | Some("dfont")
    )
}

pub(super) fn read_font_file(path: &Path, user_installed: bool) -> Result<Vec<FaceInfo>> {
    let bytes = std::fs::read(path)?;
    let modified_at = file_ctime(path);
    let face_count = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
    let mut out = Vec::new();
    for index in 0..face_count {
        let Ok(face) = ttf_parser::Face::parse(&bytes, index) else { continue };
        let base = make_info(&face, modified_at, user_installed);
        // Drop hidden/system-internal faces; upstream excludes empty or
        // dot-prefixed family names.
        if base.family.is_empty() || base.family.starts_with('.') {
            continue;
        }
        let instances = parse_fvar_instances(&face, base.variation_axes.len());
        if instances.is_empty() {
            out.push(base);
        } else {
            for inst in instances {
                out.push(apply_instance(&base, inst));
            }
        }
    }
    Ok(out)
}

fn make_info(face: &ttf_parser::Face, modified_at: u64, user_installed: bool) -> FaceInfo {
    // Prefer the typographic name (16/17) over the legacy family (1/2):
    // for many TTC entries the legacy slot bakes the style into the family
    // (e.g. NameID 1 = "NanumMyeongjoExtraBold"), while the typographic
    // slot stays style-agnostic ("Nanum Myeongjo"), which is what CoreText
    // surfaces.
    let family = pick_name(face, ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
        .or_else(|| pick_name(face, ttf_parser::name_id::FAMILY))
        .unwrap_or_default();
    let style = pick_name(face, ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY)
        .or_else(|| pick_name(face, ttf_parser::name_id::SUBFAMILY))
        .unwrap_or_else(|| "Regular".to_string());
    let postscript = pick_name(face, ttf_parser::name_id::POST_SCRIPT_NAME).unwrap_or_default();

    let weight = face.weight().to_number();
    let stretch = width_to_int(face.width());
    let italic = face.is_italic() || face.is_oblique();
    let variation_axes = extract_axes(face);

    FaceInfo {
        family,
        style,
        postscript,
        weight,
        stretch,
        italic,
        variation_axes,
        modified_at,
        user_installed,
    }
}

fn pick_name(face: &ttf_parser::Face, name_id: u16) -> Option<String> {
    let names = face.names();
    let mut fallback = None;
    // Prefer Windows/en-US (matches CoreText's default on upstream agent).
    for n in (0..names.len()).filter_map(|i| names.get(i)) {
        if n.name_id != name_id {
            continue;
        }
        if n.platform_id == ttf_parser::PlatformId::Windows && n.language_id == 0x0409 {
            if let Some(s) = decode_name(&n) {
                return Some(s);
            }
        }
        if fallback.is_none() {
            fallback = Some(n);
        }
    }
    fallback.and_then(|n| decode_name(&n))
}

/// ttf-parser's `Name::to_string` only handles UTF-16BE-encoded entries;
/// Apple system fonts often only carry MacRoman-encoded names (the legacy
/// Macintosh platform entries). Fall back to lossy UTF-8 on the raw bytes
/// so ASCII-named families like "Apple Color Emoji" survive.
fn decode_name(n: &ttf_parser::name::Name) -> Option<String> {
    if let Some(s) = n.to_string() {
        return Some(s);
    }
    if n.name.iter().all(|&b| b.is_ascii() && b != 0) {
        std::str::from_utf8(n.name).ok().map(|s| s.to_string())
    } else {
        None
    }
}

fn extract_axes(face: &ttf_parser::Face) -> Vec<AxisInfo> {
    face.variation_axes()
        .into_iter()
        .map(|a| AxisInfo {
            tag: a.tag.to_string(),
            name: face
                .names()
                .into_iter()
                .find(|n| n.name_id == a.name_id)
                .and_then(|n| n.to_string())
                .unwrap_or_default(),
            // Overridden later if a named-instance applies.
            value: a.def_value as f64,
            min: a.min_value as f64,
            max: a.max_value as f64,
            default: a.def_value as f64,
            hidden: a.hidden,
        })
        .collect()
}

fn width_to_int(w: ttf_parser::Width) -> u8 {
    match w {
        ttf_parser::Width::UltraCondensed => 1,
        ttf_parser::Width::ExtraCondensed => 2,
        ttf_parser::Width::Condensed => 3,
        ttf_parser::Width::SemiCondensed => 4,
        ttf_parser::Width::Normal => 5,
        ttf_parser::Width::SemiExpanded => 6,
        ttf_parser::Width::Expanded => 7,
        ttf_parser::Width::ExtraExpanded => 8,
        ttf_parser::Width::UltraExpanded => 9,
    }
}

/// Map a `wdth` axis coordinate (percent) onto the OS/2 usWidthClass
/// bucket (1-9). Closest target wins.
fn quantize_width(percent: f64) -> u8 {
    const BUCKETS: &[(f64, u8)] = &[
        (50.0, 1),
        (62.5, 2),
        (75.0, 3),
        (87.5, 4),
        (100.0, 5),
        (112.5, 6),
        (125.0, 7),
        (150.0, 8),
        (200.0, 9),
    ];
    let mut best = 5u8;
    let mut best_diff = f64::INFINITY;
    for (target, bucket) in BUCKETS {
        let d = (percent - target).abs();
        if d < best_diff {
            best_diff = d;
            best = *bucket;
        }
    }
    best
}

fn file_ctime(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.ctime() as u64).unwrap_or(0)
}

struct NamedInstance {
    style: String,
    postscript: Option<String>,
    coords: Vec<f64>,
}

/// fvar layout (MS OT spec): 16-byte header, then `axisCount` * 20-byte
/// axis records, then `instanceCount` * `instanceSize` records each
/// containing `subfamilyID, flags, coords[axisCount] (fixed16.16), [psNameID]`.
fn parse_fvar_instances(face: &ttf_parser::Face, axis_count: usize) -> Vec<NamedInstance> {
    let Some(data) = face.raw_face().table(ttf_parser::Tag::from_bytes(b"fvar")) else {
        return Vec::new();
    };
    if data.len() < 16 || axis_count == 0 {
        return Vec::new();
    }
    let u16_at = |o: usize| u16::from_be_bytes([data[o], data[o + 1]]);
    let i32_at = |o: usize| {
        i32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
    };

    let axes_offset = u16_at(4) as usize;
    let axis_size = u16_at(10) as usize;
    let instance_count = u16_at(12) as usize;
    let instance_size = u16_at(14) as usize;

    // instanceSize = 4 + axisCount*4, or +2 if the optional psNameID trails.
    let coords_bytes = axis_count.saturating_mul(4);
    let has_ps_name = instance_size >= coords_bytes + 6;
    let min_required = coords_bytes + 4;
    if instance_size < min_required {
        return Vec::new();
    }

    let instances_offset = axes_offset.saturating_add(axis_count.saturating_mul(axis_size));
    let mut out = Vec::with_capacity(instance_count);
    for i in 0..instance_count {
        let base = match instances_offset.checked_add(i.saturating_mul(instance_size)) {
            Some(b) if b.checked_add(instance_size).is_some_and(|e| e <= data.len()) => b,
            _ => break,
        };

        let subfamily_id = u16_at(base);
        // Skip 2 bytes of reserved instance flags before the coords.
        let coords: Vec<f64> = (0..axis_count)
            .map(|a| i32_at(base + 4 + a * 4) as f64 / 65536.0)
            .collect();
        let ps_name = if has_ps_name {
            let id = u16_at(base + 4 + coords_bytes);
            if id == 0xFFFF { None } else { pick_name(face, id) }
        } else {
            None
        };

        let style = pick_name(face, subfamily_id).unwrap_or_else(|| "Regular".to_string());
        out.push(NamedInstance { style, postscript: ps_name, coords });
    }
    out
}

fn apply_instance(base: &FaceInfo, inst: NamedInstance) -> FaceInfo {
    let mut info = base.clone();
    info.style = inst.style;
    if let Some(ps) = inst.postscript {
        info.postscript = ps;
    }
    // wght/wdth are surfaced at the top level so `weight`/`stretch` reflect
    // the instance, not the file-level defaults.
    for (axis, coord) in info.variation_axes.iter_mut().zip(inst.coords.iter()) {
        axis.value = *coord;
        match axis.tag.as_str() {
            "wght" => info.weight = coord.round() as u16,
            "wdth" => info.stretch = quantize_width(*coord),
            _ => {}
        }
    }
    info
}
