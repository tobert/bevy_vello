use std::sync::Arc;
use vello_svg::usvg::{self};

use super::asset::VelloSvg;
use crate::integrations::VectorLoaderError;

/// Deserialize an SVG file from bytes using default `usvg` options.
///
/// The default options carry an empty `fontdb::Database`, so SVG `<text>`
/// elements that resolve to outlines via font lookup will be dropped. Use
/// [`load_svg_from_bytes_with_options`] to supply a populated `fontdb`.
pub fn load_svg_from_bytes(bytes: &[u8]) -> Result<VelloSvg, VectorLoaderError> {
    load_svg_from_bytes_with_options(bytes, &usvg::Options::default())
}

/// Deserialize an SVG file from bytes using caller-provided `usvg::Options`.
///
/// Pass an `Options` whose `fontdb` has been populated when the SVG contains
/// `<text>` elements that must be flattened to outlines for Vello rendering.
pub fn load_svg_from_bytes_with_options(
    bytes: &[u8],
    options: &usvg::Options,
) -> Result<VelloSvg, VectorLoaderError> {
    let svg_str = std::str::from_utf8(bytes)?;

    // Parse SVG
    let tree = usvg::Tree::from_str(svg_str, options).map_err(vello_svg::Error::Svg)?;

    // Process the loaded SVG into Vello-compatible data
    let scene = vello_svg::render_tree(&tree);

    let width = tree.size().width();
    let height = tree.size().height();

    let asset = VelloSvg {
        scene: Arc::new(scene),
        width,
        height,
        alpha: 1.0,
    };

    Ok(asset)
}

/// Deserialize an SVG file from a string slice using default `usvg` options.
///
/// See [`load_svg_from_bytes`] for the `fontdb` caveat. Use
/// [`load_svg_from_str_with_options`] to render SVG `<text>`.
pub fn load_svg_from_str(svg_str: &str) -> Result<VelloSvg, VectorLoaderError> {
    load_svg_from_bytes(svg_str.as_bytes())
}

/// Deserialize an SVG file from a string slice using caller-provided
/// `usvg::Options`.
pub fn load_svg_from_str_with_options(
    svg_str: &str,
    options: &usvg::Options,
) -> Result<VelloSvg, VectorLoaderError> {
    load_svg_from_bytes_with_options(svg_str.as_bytes(), options)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG_WITH_TEXT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
<text x="10" y="30" font-family="DejaVu Sans" font-size="20">Hi</text>
</svg>"#;

    /// Without a populated `fontdb`, `usvg` cannot resolve the font family
    /// and the `<text>` element is dropped — the resulting scene contains
    /// no glyph outlines.
    #[test]
    fn default_options_drop_svg_text_without_fonts() {
        let svg = load_svg_from_str(SVG_WITH_TEXT).expect("svg parse should succeed");
        assert_eq!(
            svg.scene.encoding().n_paths,
            0,
            "empty fontdb should produce no paths from a text-only SVG",
        );
    }

    /// With a populated `fontdb`, `usvg` flattens `<text>` to outlines and
    /// emits one or more path commands per glyph.
    #[test]
    fn options_with_fontdb_render_svg_text() {
        let mut options = usvg::Options::default();
        let font_bytes = include_bytes!("../../../examples/assets/DejaVuSans.ttf");
        options.fontdb_mut().load_font_data(font_bytes.to_vec());

        let svg = load_svg_from_str_with_options(SVG_WITH_TEXT, &options)
            .expect("svg parse should succeed");
        assert!(
            svg.scene.encoding().n_paths > 0,
            "populated fontdb should flatten <text> to outline paths",
        );
    }
}
