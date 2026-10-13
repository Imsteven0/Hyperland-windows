//! Colour schemes in Caelestia's `scheme.json` format (`~/.local/state/caelestia`).
//! Dynamic schemes are generated from the wallpaper with Material Color
//! Utilities; static schemes (catppuccin, etc.) can be dropped in as-is.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use indexmap_lite::OrderedMap;
use material_colors::{
    color::Rgb,
    dynamic_color::{DynamicScheme, MaterialDynamicColors as M, Variant},
    image::extract_color,
};
use serde::{Deserialize, Serialize};

use crate::Palette;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scheme {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_flavour")]
    pub flavour: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_variant")]
    pub variant: String,
    /// Hex colours without `#`, keyed by Material role in camelCase.
    #[serde(default)]
    pub colours: OrderedMap,
}

fn default_name() -> String {
    "dynamic".into()
}
fn default_flavour() -> String {
    "default".into()
}
fn default_mode() -> String {
    "dark".into()
}
fn default_variant() -> String {
    "tonalspot".into()
}

impl Scheme {
    pub fn is_dark(&self) -> bool {
        self.mode != "light"
    }
    pub fn is_dynamic(&self) -> bool {
        self.name == "dynamic"
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        crate::config::write_atomic(path, &(serde_json::to_string_pretty(self)? + "\n"))
    }

    /// Builds a dynamic scheme from a source colour.
    pub fn generate(source: Rgb, variant: &str, dark: bool) -> Self {
        let scheme = DynamicScheme::by_variant(source, &parse_variant(variant), dark, None);
        let roles: [(&str, fn() -> material_colors::dynamic_color::DynamicColor<'static>); 49] = [
            ("primary_paletteKeyColor", M::primary_palette_key_color),
            ("secondary_paletteKeyColor", M::secondary_palette_key_color),
            ("tertiary_paletteKeyColor", M::tertiary_palette_key_color),
            ("neutral_paletteKeyColor", M::neutral_palette_key_color),
            ("neutral_variant_paletteKeyColor", M::neutral_variant_palette_key_color),
            ("background", M::background),
            ("onBackground", M::on_background),
            ("surface", M::surface),
            ("surfaceDim", M::surface_dim),
            ("surfaceBright", M::surface_bright),
            ("surfaceContainerLowest", M::surface_container_lowest),
            ("surfaceContainerLow", M::surface_container_low),
            ("surfaceContainer", M::surface_container),
            ("surfaceContainerHigh", M::surface_container_high),
            ("surfaceContainerHighest", M::surface_container_highest),
            ("onSurface", M::on_surface),
            ("surfaceVariant", M::surface_variant),
            ("onSurfaceVariant", M::on_surface_variant),
            ("inverseSurface", M::inverse_surface),
            ("inverseOnSurface", M::inverse_on_surface),
            ("outline", M::outline),
            ("outlineVariant", M::outline_variant),
            ("shadow", M::shadow),
            ("scrim", M::scrim),
            ("surfaceTint", M::surface_tint),
            ("primary", M::primary),
            ("onPrimary", M::on_primary),
            ("primaryContainer", M::primary_container),
            ("onPrimaryContainer", M::on_primary_container),
            ("inversePrimary", M::inverse_primary),
            ("secondary", M::secondary),
            ("onSecondary", M::on_secondary),
            ("secondaryContainer", M::secondary_container),
            ("onSecondaryContainer", M::on_secondary_container),
            ("tertiary", M::tertiary),
            ("onTertiary", M::on_tertiary),
            ("tertiaryContainer", M::tertiary_container),
            ("onTertiaryContainer", M::on_tertiary_container),
            ("error", M::error),
            ("onError", M::on_error),
            ("errorContainer", M::error_container),
            ("onErrorContainer", M::on_error_container),
            ("primaryFixed", M::primary_fixed),
            ("primaryFixedDim", M::primary_fixed_dim),
            ("onPrimaryFixed", M::on_primary_fixed),
            ("secondaryFixed", M::secondary_fixed),
            ("secondaryFixedDim", M::secondary_fixed_dim),
            ("tertiaryFixed", M::tertiary_fixed),
            ("tertiaryFixedDim", M::tertiary_fixed_dim),
        ];
        let mut colours = OrderedMap::default();
        for (name, role) in roles {
            colours.insert(name.to_owned(), hex(scheme.get_rgb(role())));
        }
        // Caelestia extends M3 with a "success" family (green, harmonised with the scheme).
        let success = if dark { 0xB5CCBA } else { 0x4F6354 };
        colours.insert("success".into(), format!("{success:06x}"));
        Self { name: "dynamic".into(), flavour: "default".into(), mode: if dark { "dark" } else { "light" }.into(), variant: variant.to_owned(), colours }
    }

    pub fn palette(&self) -> Palette {
        let c = |k: &str, fallback: u32| -> slint::Color {
            let v = self.colours.get(k).and_then(|h| u32::from_str_radix(h.trim_start_matches('#'), 16).ok()).unwrap_or(fallback);
            slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
        };
        Palette {
            primary: c("primary", 0xb5c4ff),
            on_primary: c("onPrimary", 0x1b2d61),
            primary_container: c("primaryContainer", 0x334479),
            on_primary_container: c("onPrimaryContainer", 0xdbe1ff),
            secondary: c("secondary", 0xc1c5dd),
            on_secondary: c("onSecondary", 0x2b3042),
            secondary_container: c("secondaryContainer", 0x414659),
            on_secondary_container: c("onSecondaryContainer", 0xdde1f9),
            tertiary: c("tertiary", 0xe2bbdb),
            on_tertiary: c("onTertiary", 0x422741),
            tertiary_container: c("tertiaryContainer", 0xa986a3),
            on_tertiary_container: c("onTertiaryContainer", 0x000000),
            error: c("error", 0xffb4ab),
            on_error: c("onError", 0x690005),
            error_container: c("errorContainer", 0x93000a),
            on_error_container: c("onErrorContainer", 0xffdad6),
            background: c("background", 0x121318),
            on_background: c("onBackground", 0xe3e1e9),
            surface: c("surface", 0x121318),
            on_surface: c("onSurface", 0xe3e1e9),
            surface_variant: c("surfaceVariant", 0x45464f),
            on_surface_variant: c("onSurfaceVariant", 0xc6c6d0),
            surface_dim: c("surfaceDim", 0x121318),
            surface_bright: c("surfaceBright", 0x38393f),
            surface_container_lowest: c("surfaceContainerLowest", 0x0d0e13),
            surface_container_low: c("surfaceContainerLow", 0x1a1b21),
            surface_container: c("surfaceContainer", 0x1e1f25),
            surface_container_high: c("surfaceContainerHigh", 0x292a2f),
            surface_container_highest: c("surfaceContainerHighest", 0x34343a),
            outline: c("outline", 0x8f909a),
            outline_variant: c("outlineVariant", 0x45464f),
            inverse_surface: c("inverseSurface", 0xe3e1e9),
            inverse_on_surface: c("inverseOnSurface", 0x2f3036),
            inverse_primary: c("inversePrimary", 0x4b5c92),
            shadow: c("shadow", 0x000000),
            scrim: c("scrim", 0x000000),
            success: c("success", 0xb5ccba),
        }
    }
}

fn hex(rgb: Rgb) -> String {
    format!("{:06x}", rgb.as_u32() & 0xFF_FFFF)
}

pub fn parse_variant(v: &str) -> Variant {
    match v.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "vibrant" => Variant::Vibrant,
        "expressive" => Variant::Expressive,
        "fidelity" => Variant::Fidelity,
        "fruitsalad" => Variant::FruitSalad,
        "monochrome" => Variant::Monochrome,
        "neutral" => Variant::Neutral,
        "rainbow" => Variant::Rainbow,
        "content" => Variant::Content,
        _ => Variant::TonalSpot,
    }
}

/// Extracts the dominant (scored) colour from an image, downscaled first as
/// recommended by Material Color Utilities.
pub fn source_from_image(path: &Path) -> Result<Rgb> {
    let img = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
    let small = img.resize(128, 128, image::imageops::FilterType::Triangle).into_rgb8();
    let pixels: Vec<Rgb> = small.pixels().map(|p| Rgb::new(p[0], p[1], p[2])).collect();
    Ok(extract_color(&pixels))
}

/// Minimal insertion-ordered map so scheme.json keeps a stable, readable key order.
mod indexmap_lite {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeMap};

    #[derive(Debug, Clone, Default)]
    pub struct OrderedMap(Vec<(String, String)>);

    impl OrderedMap {
        pub fn get(&self, k: &str) -> Option<&String> {
            self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
        }
        pub fn insert(&mut self, k: String, v: String) {
            match self.0.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => self.0.push((k, v)),
            }
        }
    }

    impl Serialize for OrderedMap {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut m = s.serialize_map(Some(self.0.len()))?;
            for (k, v) in &self.0 {
                m.serialize_entry(k, v)?;
            }
            m.end()
        }
    }

    impl<'de> Deserialize<'de> for OrderedMap {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let map = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
            Ok(Self(
                map.into_iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k, s.trim_start_matches('#').to_owned())))
                    .collect(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_scheme_roundtrips_to_palette() {
        let s = Scheme::generate(Rgb::from_u32(0x4a6fa5), "tonalspot", true);
        assert!(s.colours.get("primary").is_some());
        let json = serde_json::to_string(&s).unwrap();
        let back: Scheme = serde_json::from_str(&json).unwrap();
        assert_eq!(back.colours.get("surface"), s.colours.get("surface"));
        let _ = back.palette();
    }
}
