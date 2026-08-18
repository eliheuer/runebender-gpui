// Copyright 2026 the Runebender Authors
// SPDX-License-Identifier: Apache-2.0

//! Re-export of the shared norad→kurbo path building in
//! `runebender_core::glyph_paths`.

pub use runebender_core::glyph_paths::*;

#[cfg(test)]
mod tests {
    use kurbo::Shape;
    use std::path::PathBuf;

    #[test]
    fn demo_font_outlines_are_sane() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../runebender-web/assets/test-fonts/VirtuaGrotesk-Regular.ufo");
        let font = norad::Font::load(path).expect("demo font loads");
        let mut with_outline = 0;
        for glyph in font.default_layer().iter() {
            let bez = super::glyph_to_bezpath(glyph, &font);
            if !bez.elements().is_empty() {
                with_outline += 1;
                assert!(bez.area().abs().is_finite(), "{}: bad area", glyph.name());
            }
        }
        assert!(with_outline > 300, "only {with_outline} glyphs had outlines");
        let a = font.get_glyph("A").expect("glyph A exists");
        let bbox = super::glyph_to_bezpath(a, &font).bounding_box();
        assert!(bbox.height() > 300.0);
        if let Some(aacute) = font.get_glyph("Aacute") {
            let bez2 = super::glyph_to_bezpath(aacute, &font);
            assert!(bez2.bounding_box().height() > bbox.height());
        }
    }
}
