//! ttf-parser 0.25 exposes fvar axes but not named instances, so we walk
//! the raw table bytes ourselves (`parse_fvar_instances`).

use super::{AxisInfo, FaceInfo};
use anyhow::Result;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[cfg(not(target_os = "macos"))]
pub(super) fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("ttf") | Some("otf") | Some("ttc") | Some("otc") | Some("dfont")
    )
}

pub(super) fn read_font_file(path: &Path, user_installed: bool) -> Result<Vec<FaceInfo>> {
    let bytes = std::fs::read(path)?;
    let modified_at = file_ctime(path);
    let face_count = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
    let mut out = Vec::new();
    for index in 0..face_count {
        let Ok(face) = ttf_parser::Face::parse(&bytes, index) else {
            continue;
        };
        let base = make_info(&face, modified_at, user_installed);
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
        .map(|s| canonicalize_style(face, &s))
        .unwrap_or_else(|| "Regular".to_string());
    let postscript = pick_name(face, ttf_parser::name_id::POST_SCRIPT_NAME).unwrap_or_default();

    let weight = face.weight().to_number();
    let stretch = read_os2_width(face);
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
        .map(|a| {
            let tag = a.tag.to_string();
            AxisInfo {
                name: pick_axis_name(face, &tag, a.name_id),
                tag,
                // Overridden later if a named-instance applies.
                value: a.def_value,
                min: a.min_value,
                max: a.max_value,
                default: a.def_value,
                hidden: a.hidden,
            }
        })
        .collect()
}

/// Upstream's axis-name rule depends on whether the tag is OT-registered:
///   - Registered tags (wght / wdth / slnt / ital / opsz) take the
///     human-readable label and PascalCase it: capitalise the first
///     letter of each space-separated word and drop the spaces. The
///     label comes from Win/en-US UTF-16BE if present (Inter's
///     "Optical size" → "OpticalSize"), otherwise Mac/en ASCII (Skia
///     stores only Mac names → "Weight").
///   - Custom tags prefer the Macintosh entry but only when ttf-parser
///     can decode it (i.e. UTF-16BE). Apple-shipped variable fonts often
///     carry a MacRoman/ASCII Mac entry that ttf-parser can't decode
///     (Recursive's `MONO` has Mac=ASCII "Monospace" alongside Win=UTF-16BE
///     "Monospace"); upstream surfaces the tag itself in that case. The
///     `decode_name` ASCII fallback used for family names would defeat
///     this rule, so we don't reach for it here.
///   - If no Mac entry is present at all (PingFangUI's `WDTH`/`HGHT`),
///     fall through to the Windows long name.
fn pick_axis_name(face: &ttf_parser::Face, tag: &str, name_id: u16) -> String {
    let names = face.names();
    let mut has_mac = false;
    let mut mac_utf16: Option<String> = None;
    let mut mac_ascii: Option<String> = None;
    let mut win_utf16: Option<String> = None;
    for j in 0..names.len() {
        let Some(n) = names.get(j) else { continue };
        if n.name_id != name_id {
            continue;
        }
        if n.platform_id == ttf_parser::PlatformId::Macintosh {
            if !has_mac {
                has_mac = true;
                mac_utf16 = n.to_string();
            }
            if n.language_id == 0 && mac_ascii.is_none() {
                mac_ascii = decode_ascii(&n);
            }
        } else if n.platform_id == ttf_parser::PlatformId::Windows
            && n.language_id == 0x0409
            && win_utf16.is_none()
        {
            win_utf16 = n.to_string();
        }
    }
    if is_registered_axis(tag) {
        return win_utf16
            .or(mac_ascii)
            .map(|s| pascal_case(&s))
            .unwrap_or_else(|| tag.to_string());
    }
    if has_mac {
        mac_utf16
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| tag.to_string())
    } else {
        win_utf16.unwrap_or_else(|| tag.to_string())
    }
}

fn decode_ascii(n: &ttf_parser::name::Name) -> Option<String> {
    if n.name.iter().all(|&b| b.is_ascii() && b != 0) {
        std::str::from_utf8(n.name).ok().map(|s| s.to_string())
    } else {
        None
    }
}

fn is_registered_axis(tag: &str) -> bool {
    matches!(tag, "wght" | "wdth" | "slnt" | "ital" | "opsz")
}

fn pascal_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for word in input.split(' ') {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Match upstream's Galvji-Oblique → "Italic" rewrite. The trigger is an
/// italic face whose en-US Subfamily reads "Oblique" while a sibling
/// Win/en-* entry already spells the style as "Italic". Fonts without
/// such a sibling (Avenir-Oblique, Helvetica-Oblique) keep "Oblique".
fn canonicalize_style(face: &ttf_parser::Face, style: &str) -> String {
    let italic = face.is_italic() || face.is_oblique();
    if !italic || !style.eq_ignore_ascii_case("Oblique") {
        return style.to_string();
    }
    let names = face.names();
    let has_italic_sibling = (0..names.len()).filter_map(|i| names.get(i)).any(|n| {
        n.name_id == ttf_parser::name_id::SUBFAMILY
            && n.platform_id == ttf_parser::PlatformId::Windows
            // English regional langs all end in 0x09 in the low byte.
            && n.language_id & 0xff == 0x09
            && n.to_string().is_some_and(|s| s.eq_ignore_ascii_case("Italic"))
    });
    if has_italic_sibling {
        "Italic".to_string()
    } else {
        style.to_string()
    }
}

/// Read `OS/2.usWidthClass` directly: ttf-parser maps the out-of-range
/// value 0 to `Normal` (5), but upstream and CoreText both surface it as
/// the minimum bucket (1).
fn read_os2_width(face: &ttf_parser::Face) -> u8 {
    let Some(data) = face.raw_face().table(ttf_parser::Tag::from_bytes(b"OS/2")) else {
        return 5;
    };
    if data.len() < 8 {
        return 5;
    }
    let raw = u16::from_be_bytes([data[6], data[7]]);
    raw.clamp(1, 9) as u8
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

#[cfg(unix)]
pub(super) fn file_ctime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|m| m.ctime() as u64)
        .unwrap_or(0)
}

#[cfg(not(unix))]
pub(super) fn file_ctime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct NamedInstance {
    style: String,
    coords: Vec<f32>,
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
    let i32_at = |o: usize| i32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);

    let axes_offset = u16_at(4) as usize;
    let axis_size = u16_at(10) as usize;
    let instance_count = u16_at(12) as usize;
    let instance_size = u16_at(14) as usize;

    let coords_bytes = axis_count.saturating_mul(4);
    if instance_size < coords_bytes + 4 {
        return Vec::new();
    }

    let instances_offset = axes_offset.saturating_add(axis_count.saturating_mul(axis_size));
    let mut out = Vec::with_capacity(instance_count);
    for i in 0..instance_count {
        let base = match instances_offset.checked_add(i.saturating_mul(instance_size)) {
            Some(b)
                if b.checked_add(instance_size)
                    .is_some_and(|e| e <= data.len()) =>
            {
                b
            }
            _ => break,
        };

        let subfamily_id = u16_at(base);
        // Coords are fixed16.16 (i32 / 65536); the +4 skips subfamilyID
        // and reserved flags. f32 keeps upstream's serialised precision.
        let coords: Vec<f32> = (0..axis_count)
            .map(|a| i32_at(base + 4 + a * 4) as f32 / 65536.0)
            .collect();
        let style = pick_name(face, subfamily_id).unwrap_or_else(|| "Regular".to_string());
        out.push(NamedInstance { style, coords });
    }
    out
}

/// Upstream builds the instance postscript as
/// `family.replace(' ', '') + '-' + style.replace(' ', '')`, ignoring
/// both the per-instance psNameID (sometimes lower-case CJK region codes
/// like "cn" instead of "SC") and NameID 25 (often baked with the style,
/// e.g. "STIXTwoTextItalic"). wght truncates to u16 (Skia-Bold's
/// wght=1.949997 surfaces as weight=1). Italic flips when slnt or ital
/// is non-zero on the instance, since fvar drives "Italic" via those
/// axes rather than per-face fsSelection bits.
fn apply_instance(base: &FaceInfo, inst: NamedInstance) -> FaceInfo {
    let mut info = base.clone();
    info.postscript = format!(
        "{}-{}",
        base.family.replace(' ', ""),
        inst.style.replace(' ', "")
    );
    info.style = inst.style;
    for (axis, coord) in info.variation_axes.iter_mut().zip(inst.coords.iter()) {
        axis.value = *coord;
        match axis.tag.as_str() {
            "wght" => info.weight = *coord as u16,
            "wdth" => info.stretch = quantize_width(*coord as f64),
            "slnt" | "ital" => {
                if *coord != 0.0 {
                    info.italic = true;
                }
            }
            _ => {}
        }
    }
    info
}
