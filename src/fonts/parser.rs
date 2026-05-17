//! ttf-parser 0.25 exposes fvar axes but not named instances, so we walk
//! the raw table bytes ourselves (`parse_fvar_instances`).

use super::{AxisInfo, FontInfo};
use anyhow::Result;
use std::path::Path;

pub(super) fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("ttf") | Some("otf") | Some("ttc") | Some("otc") | Some("dfont")
    )
}

pub(super) fn read_font_file(path: &Path, user_installed: bool) -> Result<Vec<FontInfo>> {
    let bytes = std::fs::read(path)?;
    let face_count = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
    let mut out = Vec::new();
    for index in 0..face_count {
        let Ok(face) = ttf_parser::Face::parse(&bytes, index) else { continue };
        let base = make_info(&face, path, user_installed);
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

fn make_info(face: &ttf_parser::Face, path: &Path, user_installed: bool) -> FontInfo {
    let family = pick_name(face, ttf_parser::name_id::FAMILY)
        .or_else(|| pick_name(face, ttf_parser::name_id::TYPOGRAPHIC_FAMILY))
        .unwrap_or_default();
    let style = pick_name(face, ttf_parser::name_id::SUBFAMILY)
        .or_else(|| pick_name(face, ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY))
        .unwrap_or_else(|| "Regular".to_string());
    let postscript = pick_name(face, ttf_parser::name_id::POST_SCRIPT_NAME).unwrap_or_default();
    let full_name = pick_name(face, ttf_parser::name_id::FULL_NAME)
        .unwrap_or_else(|| format!("{family} {style}"));

    let weight = face.weight().to_number() as f64;
    let stretch = stretch_to_percent(face.width());
    let italic = face.is_italic() || face.is_oblique();
    let variation_axes = extract_axes(face);

    FontInfo {
        family,
        style,
        postscript,
        weight,
        stretch,
        italic,
        variation_axes,
        user_installed,
        name: full_name,
        path: path.to_string_lossy().into_owned(),
    }
}

fn pick_name(face: &ttf_parser::Face, name_id: u16) -> Option<String> {
    let names = face.names();
    let mut fallback = None;
    // Prefer Windows/en-US (matches CoreText's default on orig macOS daemon).
    for n in (0..names.len()).filter_map(|i| names.get(i)) {
        if n.name_id != name_id {
            continue;
        }
        if n.platform_id == ttf_parser::PlatformId::Windows && n.language_id == 0x0409 {
            return n.to_string();
        }
        if fallback.is_none() {
            fallback = Some(n);
        }
    }
    fallback.and_then(|n| n.to_string())
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

fn stretch_to_percent(w: ttf_parser::Width) -> f64 {
    match w {
        ttf_parser::Width::UltraCondensed => 50.0,
        ttf_parser::Width::ExtraCondensed => 62.5,
        ttf_parser::Width::Condensed => 75.0,
        ttf_parser::Width::SemiCondensed => 87.5,
        ttf_parser::Width::Normal => 100.0,
        ttf_parser::Width::SemiExpanded => 112.5,
        ttf_parser::Width::Expanded => 125.0,
        ttf_parser::Width::ExtraExpanded => 150.0,
        ttf_parser::Width::UltraExpanded => 200.0,
    }
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

fn apply_instance(base: &FontInfo, inst: NamedInstance) -> FontInfo {
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
            "wght" => info.weight = *coord,
            "wdth" => info.stretch = *coord,
            _ => {}
        }
    }
    info.name = format!("{} {}", info.family, info.style);
    info
}
