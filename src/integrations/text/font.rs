use std::borrow::Cow;

use bevy::{prelude::*, reflect::TypePath, render::render_asset::RenderAsset};
use parley::{
    FontSettings, FontStyle, FontVariation, Layout, PositionedLayoutItem, RangedBuilder,
    StyleProperty,
};
use vello::{
    Scene,
    kurbo::Affine,
    peniko::{Brush, Fill},
};

use super::{VelloFontAxes, VelloTextAnchor, context::LOCAL_FONT_CONTEXT};
use crate::{
    integrations::text::context::{LOCAL_LAYOUT_CONTEXT, get_global_font_context},
    prelude::{VelloTextAlign, VelloTextStyle},
};

#[derive(Asset, TypePath, Debug, Clone)]
pub struct VelloFont {
    /// Defaults to Bevy's bevy_text default font family name.
    ///
    /// https://github.com/bevyengine/bevy/tree/v0.15.3/crates/bevy_text/src/FiraMono-subset.ttf
    pub(crate) family_name: String,
    pub bytes: Vec<u8>,
}

impl RenderAsset for VelloFont {
    type SourceAsset = VelloFont;

    type Param = ();

    fn prepare_asset(
        source_asset: Self::SourceAsset,
        _asset_id: AssetId<Self::SourceAsset>,
        _param: &mut bevy::ecs::system::SystemParamItem<Self::Param>,
        _previous_asset: Option<&Self>,
    ) -> Result<Self, bevy::render::render_asset::PrepareAssetError<Self::SourceAsset>> {
        Ok(source_asset)
    }
}

impl VelloFont {
    pub fn new(font_data: Vec<u8>) -> Self {
        Self {
            bytes: font_data,
            family_name: "Fira Mono".to_string(),
        }
    }

    pub fn layout(
        &self,
        value: &str,
        style: &VelloTextStyle,
        text_align: VelloTextAlign,
        max_advance: Option<f32>,
    ) -> Layout<Brush> {
        LOCAL_FONT_CONTEXT.with_borrow_mut(|font_context| {
            if font_context.is_none() {
                *font_context = Some(get_global_font_context().clone());
            }

            let font_context = font_context.as_mut().unwrap();

            LOCAL_LAYOUT_CONTEXT.with_borrow_mut(|layout_context| {
                let mut builder = layout_context.ranged_builder(font_context, value, 1.0, true);

                apply_font_styles(&mut builder, style);
                apply_variable_axes(&mut builder, &style.font_axes);

                builder.push_default(StyleProperty::FontStack(parley::FontStack::Single(
                    parley::FontFamily::Named(Cow::Owned(self.family_name.clone())),
                )));

                let mut layout = builder.build(value);
                layout.break_all_lines(max_advance);
                layout.align(
                    max_advance,
                    text_align.into(),
                    parley::AlignmentOptions::default(),
                );

                layout
            })
        })
    }

    #[expect(clippy::too_many_arguments, reason = "Common lint in bevy")]
    pub(crate) fn render(
        &self,
        scene: &mut Scene,
        mut transform: Affine,
        value: &str,
        style: &VelloTextStyle,
        text_align: VelloTextAlign,
        max_advance: Option<f32>,
        text_anchor: VelloTextAnchor,
        ui_content_box: Option<Rect>,
        clip: Option<vello::kurbo::Rect>,
    ) {
        let layout = self.layout(value, style, text_align, max_advance);

        let text_w = layout.width() as f64;
        let text_h = layout.height() as f64;

        let (dx, dy) = if let Some(content_box) = ui_content_box {
            let offset = compute_ui_anchor_offset(text_anchor, text_w, text_h, content_box);
            // Transform the logical offset through the linear part of the affine
            // (rotation + scale), so anchor positioning is correct under any transform.
            let c = transform.as_coeffs();
            (
                c[0] * offset.0 + c[2] * offset.1,
                c[1] * offset.0 + c[3] * offset.1,
            )
        } else {
            compute_world_anchor_offset(text_anchor, text_w, text_h)
        };

        transform = transform.then_translate(vello::kurbo::Vec2::new(dx, dy));

        // Precompute Y-axis clip range for vertical line culling (CPU-side).
        // Inverse-transform the clip rect corners to layout space and compute
        // their vertical AABB. Horizontal clipping is handled by Vello's
        // GPU-side clip path.
        let cull_y: Option<(f64, f64)> = clip.and_then(|r| {
            let inv = transform.inverse();
            // A zero-determinant affine has no inverse — skip culling.
            // Affine::inverse() returns an identity-ish result for singular
            // matrices, so check the determinant explicitly.
            let c = transform.as_coeffs();
            let det = c[0] * c[3] - c[1] * c[2];
            if det.abs() < f64::EPSILON {
                return None;
            }
            // Map clip-rect corners to layout space, extract Y-axis AABB.
            let corners = [
                inv * vello::kurbo::Point::new(r.x0, r.y0),
                inv * vello::kurbo::Point::new(r.x1, r.y0),
                inv * vello::kurbo::Point::new(r.x0, r.y1),
                inv * vello::kurbo::Point::new(r.x1, r.y1),
            ];
            let y_min = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
            let y_max = corners
                .iter()
                .map(|p| p.y)
                .fold(f64::NEG_INFINITY, f64::max);
            Some((y_min, y_max))
        });

        'lines: for line in layout.lines() {
            // Vertical line culling using Parley's LineMetrics. min_coord is the
            // line top (including ascenders), max_coord is the bottom (including
            // descenders). Lines emit top-to-bottom, so exceeding y_max means
            // all remaining lines are outside (early exit).
            if let Some((y_min, y_max)) = cull_y {
                let lm = line.metrics();
                if (lm.max_coord as f64) < y_min {
                    continue;
                }
                if (lm.min_coord as f64) > y_max {
                    break 'lines;
                }
            }

            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };

                let mut x = glyph_run.offset();
                let y = glyph_run.baseline();
                let run = glyph_run.run();
                let font = run.font();
                let font_size = run.font_size();
                let synthesis = run.synthesis();
                let glyph_xform = synthesis
                    .skew()
                    .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));

                scene
                    .draw_glyphs(font)
                    .brush(&style.brush)
                    .hint(true)
                    .transform(transform)
                    .glyph_transform(glyph_xform)
                    .font_size(font_size)
                    .normalized_coords(run.normalized_coords())
                    .draw(
                        Fill::NonZero,
                        glyph_run.glyphs().map(|glyph| {
                            let gx = x + glyph.x;
                            let gy = y - glyph.y;
                            x += glyph.advance;
                            vello::Glyph {
                                id: glyph.id as _,
                                x: gx,
                                y: gy,
                            }
                        }),
                    );
            }
        }
    }
}

/// Applies the font styles to the text
///
/// font_size - font size
/// line_height - line height
/// word_spacing - extra spacing between words
/// letter_spacing - extra spacing between letters
fn apply_font_styles(builder: &mut RangedBuilder<'_, Brush>, style: &VelloTextStyle) {
    builder.push_default(StyleProperty::FontSize(style.font_size));
    builder.push_default(StyleProperty::LineHeight(
        parley::LineHeight::MetricsRelative(style.line_height),
    ));
    builder.push_default(StyleProperty::WordSpacing(style.word_spacing));
    builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
}

/// Applies the variable axes to the text
///
/// wght - font weight
/// wdth - font width
/// opsz - optical size
/// ital - italic
/// slnt - slant
/// GRAD - grade
/// XOPQ - thick stroke
/// YOPQ - thin stroke
/// YTUC - uppercase height
/// YTLC - lowercase height
/// YTAS - ascender height
/// YTDE - descender depth
/// YTFI - figure height
fn apply_variable_axes(builder: &mut RangedBuilder<'_, Brush>, axes: &VelloFontAxes) {
    let mut variable_axes: Vec<FontVariation> = vec![];

    if let Some(weight) = axes.weight {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("wght"),
            value: weight,
        });
    }

    if let Some(width) = axes.width {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("wdth"),
            value: width,
        });
    }

    if let Some(optical_size) = axes.optical_size {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("opsz"),
            value: optical_size,
        });
    }

    if let Some(grade) = axes.grade {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("GRAD"),
            value: grade,
        });
    }

    if let Some(thick_stroke) = axes.thick_stroke {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("XOPQ"),
            value: thick_stroke,
        });
    }

    if let Some(thin_stroke) = axes.thin_stroke {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("YOPQ"),
            value: thin_stroke,
        });
    }

    if let Some(counter_width) = axes.counter_width {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("XTRA"),
            value: counter_width,
        });
    }

    if let Some(uppercase_height) = axes.uppercase_height {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("YTUC"),
            value: uppercase_height,
        });
    }

    if let Some(lowercase_height) = axes.lowercase_height {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("YTLC"),
            value: lowercase_height,
        });
    }

    if let Some(ascender_height) = axes.ascender_height {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("YTAS"),
            value: ascender_height,
        });
    }

    if let Some(descender_depth) = axes.descender_depth {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("YTDE"),
            value: descender_depth,
        });
    }

    if let Some(figure_height) = axes.figure_height {
        variable_axes.push(parley::swash::Setting {
            tag: parley::swash::tag_from_str_lossy("YTFI"),
            value: figure_height,
        });
    }

    if axes.italic {
        builder.push_default(StyleProperty::FontStyle(FontStyle::Italic));
    } else if axes.slant.is_some() {
        builder.push_default(StyleProperty::FontStyle(FontStyle::Oblique(axes.slant)));
    }

    builder.push_default(StyleProperty::FontVariations(FontSettings::List(
        variable_axes.into(),
    )));
}

/// Computes the (dx, dy) translation offset for world-space text anchoring.
///
/// Positions the text bounding box relative to the transform origin.
/// `TopLeft=(0,0)` means text grows down-right from origin.
pub(crate) fn compute_world_anchor_offset(
    text_anchor: VelloTextAnchor,
    text_w: f64,
    text_h: f64,
) -> (f64, f64) {
    match text_anchor {
        VelloTextAnchor::TopLeft => (0.0, 0.0),
        VelloTextAnchor::Left => (0.0, -text_h / 2.0),
        VelloTextAnchor::BottomLeft => (0.0, -text_h),
        VelloTextAnchor::Top => (-text_w / 2.0, 0.0),
        VelloTextAnchor::Center => (-text_w / 2.0, -text_h / 2.0),
        VelloTextAnchor::Bottom => (-text_w / 2.0, -text_h),
        VelloTextAnchor::TopRight => (-text_w, 0.0),
        VelloTextAnchor::Right => (-text_w, -text_h / 2.0),
        VelloTextAnchor::BottomRight => (-text_w, -text_h),
    }
}

/// Computes the (dx, dy) translation offset for UI text anchoring.
///
/// Aligns text within the node's content box (the region inside the node's
/// border and padding, as reported by `ComputedNode::content_box`). The
/// UiGlobalTransform places the origin at the node's center, and the content
/// box is in object-centered coordinates, so anchor offsets are computed
/// against the content-box rect directly.
pub(crate) fn compute_ui_anchor_offset(
    text_anchor: VelloTextAnchor,
    text_w: f64,
    text_h: f64,
    content_box: Rect,
) -> (f64, f64) {
    let cb_w = content_box.width() as f64;
    let cb_h = content_box.height() as f64;
    let top_left_x = content_box.min.x as f64;
    let top_left_y = content_box.min.y as f64;

    let (anchor_x, anchor_y) = match text_anchor {
        VelloTextAnchor::TopLeft => (0.0, 0.0),
        VelloTextAnchor::Top => ((cb_w - text_w) / 2.0, 0.0),
        VelloTextAnchor::TopRight => (cb_w - text_w, 0.0),
        VelloTextAnchor::Left => (0.0, (cb_h - text_h) / 2.0),
        VelloTextAnchor::Center => ((cb_w - text_w) / 2.0, (cb_h - text_h) / 2.0),
        VelloTextAnchor::Right => (cb_w - text_w, (cb_h - text_h) / 2.0),
        VelloTextAnchor::BottomLeft => (0.0, cb_h - text_h),
        VelloTextAnchor::Bottom => ((cb_w - text_w) / 2.0, cb_h - text_h),
        VelloTextAnchor::BottomRight => (cb_w - text_w, cb_h - text_h),
    };

    (top_left_x + anchor_x, top_left_y + anchor_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- World-space text ---

    #[test]
    fn world_center_offsets_by_half_text_dims() {
        let (dx, dy) = compute_world_anchor_offset(VelloTextAnchor::Center, 200.0, 40.0);
        assert_eq!((dx, dy), (-100.0, -20.0));
    }

    #[test]
    fn world_top_left_is_zero() {
        let (dx, dy) = compute_world_anchor_offset(VelloTextAnchor::TopLeft, 200.0, 40.0);
        assert_eq!((dx, dy), (0.0, 0.0));
    }

    #[test]
    fn world_bottom_right_offsets_by_full_text_dims() {
        let (dx, dy) = compute_world_anchor_offset(VelloTextAnchor::BottomRight, 200.0, 40.0);
        assert_eq!((dx, dy), (-200.0, -40.0));
    }

    // --- UI text ---
    // Anchor positions text within the node's content box.
    // Transform origin is at the node's CENTER (UiGlobalTransform origin), and
    // the content box is in object-centered coords.
    //
    // Given: node 400x200 with no padding/border, text 200x40
    // → content_box: min=(-200,-100), max=(200,100)
    //
    // TopLeft:     (-200, -100)
    // Center:      (-100, -20)
    // BottomRight: (0, 60)

    const NODE_W: f32 = 400.0;
    const NODE_H: f32 = 200.0;
    const TEXT_W: f64 = 200.0;
    const TEXT_H: f64 = 40.0;

    fn full_node_content_box() -> Rect {
        Rect::from_center_size(Vec2::ZERO, Vec2::new(NODE_W, NODE_H))
    }

    #[test]
    fn ui_top_left_positions_at_node_top_left() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::TopLeft,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (-200.0, -100.0));
    }

    #[test]
    fn ui_center_centers_text_in_node() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::Center,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (-100.0, -20.0));
    }

    #[test]
    fn ui_bottom_right_positions_at_node_bottom_right() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::BottomRight,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (0.0, 60.0));
    }

    #[test]
    fn ui_left_vertically_centers_at_left_edge() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::Left,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (-200.0, -20.0));
    }

    #[test]
    fn ui_top_right_positions_at_node_top_right() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::TopRight,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (0.0, -100.0));
    }

    #[test]
    fn ui_bottom_centers_at_bottom_edge() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::Bottom,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (-100.0, 60.0));
    }

    #[test]
    fn ui_top_centers_at_top_edge() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::Top,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (-100.0, -100.0));
    }

    #[test]
    fn ui_right_vertically_centers_at_right_edge() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::Right,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (0.0, -20.0));
    }

    #[test]
    fn ui_bottom_left_positions_at_node_bottom_left() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::BottomLeft,
            TEXT_W,
            TEXT_H,
            full_node_content_box(),
        );
        assert_eq!((dx, dy), (-200.0, 60.0));
    }

    // --- Padding-aware anchoring ---
    // For a 400x200 node with 10px symmetric padding, content_box has
    // min=(-190,-90), max=(190,90), width=380, height=180.

    fn padded_content_box() -> Rect {
        Rect::from_corners(Vec2::new(-190.0, -90.0), Vec2::new(190.0, 90.0))
    }

    #[test]
    fn ui_top_left_respects_padding() {
        // Without padding: text top-left lands at the border-box corner (-200, -100).
        // With 10px padding: it lands at the content-box corner (-190, -90).
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::TopLeft,
            TEXT_W,
            TEXT_H,
            padded_content_box(),
        );
        assert_eq!((dx, dy), (-190.0, -90.0));
    }

    #[test]
    fn ui_center_respects_padding() {
        // Symmetric padding keeps the content-box centered on the node center,
        // so Center anchor is unchanged: text centered around the origin.
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::Center,
            TEXT_W,
            TEXT_H,
            padded_content_box(),
        );
        assert_eq!((dx, dy), (-100.0, -20.0));
    }

    #[test]
    fn ui_bottom_right_respects_padding() {
        let (dx, dy) = compute_ui_anchor_offset(
            VelloTextAnchor::BottomRight,
            TEXT_W,
            TEXT_H,
            padded_content_box(),
        );
        assert_eq!((dx, dy), (-10.0, 50.0));
    }

    #[test]
    fn ui_asymmetric_padding_shifts_center() {
        // Asymmetric content_box: padding-left=20, padding-right=0,
        // padding-top=0, padding-bottom=0 on a 400x200 node →
        // content_box min=(-180,-100), max=(200,100), width=380, height=200.
        let cb = Rect::from_corners(Vec2::new(-180.0, -100.0), Vec2::new(200.0, 100.0));
        let (dx, dy) = compute_ui_anchor_offset(VelloTextAnchor::Center, TEXT_W, TEXT_H, cb);
        // Content-box center is at x = (-180+200)/2 = 10. Text centered around it
        // is offset by (10 - text_w/2, 0 - text_h/2) = (-90, -20).
        assert_eq!((dx, dy), (-90.0, -20.0));
    }

    /// The full UI anchor pipeline — compute logical offset, transform through
    /// the affine's linear part, apply to affine — must produce correct
    /// translations at non-1x scale factors.
    #[test]
    fn ui_anchor_pipeline_correct_at_2x_dpi() {
        let scale = 2.0_f64;
        // Affine for a UI node at (250, 150) logical on a 2x display.
        let affine = Affine::new([scale, 0.0, 0.0, scale, 500.0, 300.0]);

        // Node is 200x100 logical, text is 100x20, no padding.
        // Center anchor: offset = (-50, -10) logical.
        let cb = Rect::from_center_size(Vec2::ZERO, Vec2::new(200.0, 100.0));
        let offset = compute_ui_anchor_offset(VelloTextAnchor::Center, 100.0, 20.0, cb);

        // Apply linear part of affine to offset — same path as render().
        let c = affine.as_coeffs();
        let dx = c[0] * offset.0 + c[2] * offset.1;
        let dy = c[1] * offset.0 + c[3] * offset.1;
        let result = affine.then_translate(vello::kurbo::Vec2::new(dx, dy));
        let c = result.as_coeffs();

        // Text origin should land at (250-50, 150-10) = (200, 140) logical,
        // which is (400, 280) physical.
        assert_eq!((c[4], c[5]), (400.0, 280.0));
    }

    /// Anchor offset must be rotated correctly when the node's affine includes
    /// a rotation. A 90-degree CCW rotation swaps axes, so the logical x-offset
    /// becomes a physical y-offset and vice versa.
    #[test]
    fn ui_anchor_pipeline_correct_under_rotation() {
        let scale = 2.0_f64;
        let cos90 = 0.0_f64;
        let sin90 = 1.0_f64;
        // 90-degree CCW rotation with 2x scale, node center at (500, 300) physical.
        // Affine columns: [cos*s, sin*s, -sin*s, cos*s, tx, ty]
        let affine = Affine::new([
            cos90 * scale,
            sin90 * scale,
            -sin90 * scale,
            cos90 * scale,
            500.0,
            300.0,
        ]);

        // Node is 200x100 logical, text is 100x20, no padding.
        // Center anchor: offset = (-50, -10) logical.
        let cb = Rect::from_center_size(Vec2::ZERO, Vec2::new(200.0, 100.0));
        let offset = compute_ui_anchor_offset(VelloTextAnchor::Center, 100.0, 20.0, cb);

        // Apply linear part of affine to offset — same path as render().
        let c = affine.as_coeffs();
        let dx = c[0] * offset.0 + c[2] * offset.1;
        let dy = c[1] * offset.0 + c[3] * offset.1;
        let result = affine.then_translate(vello::kurbo::Vec2::new(dx, dy));
        let c = result.as_coeffs();

        // Under 90-degree CCW rotation, logical (-50, -10) maps to physical:
        //   screen_x += cos*s*(-50) + (-sin*s)*(-10) = 0*(-50) + (-2)*(-10) = +20
        //   screen_y += sin*s*(-50) + cos*s*(-10)    = 2*(-50) + 0*(-10)    = -100
        // So text origin lands at (500+20, 300-100) = (520, 200) physical.
        assert_eq!((c[4], c[5]), (520.0, 200.0));
    }

    #[test]
    fn ui_center_is_origin_when_text_fills_node() {
        let cb = Rect::from_center_size(Vec2::ZERO, Vec2::new(400.0, 200.0));
        let (ui_dx, ui_dy) = compute_ui_anchor_offset(VelloTextAnchor::Center, 400.0, 200.0, cb);
        assert_eq!((ui_dx, ui_dy), (-200.0, -100.0));

        let (ui_tl_dx, ui_tl_dy) =
            compute_ui_anchor_offset(VelloTextAnchor::TopLeft, 400.0, 200.0, cb);
        assert_eq!((ui_tl_dx, ui_tl_dy), (-200.0, -100.0));

        let (w_dx, w_dy) = compute_world_anchor_offset(VelloTextAnchor::TopLeft, 400.0, 200.0);
        assert_eq!((w_dx, w_dy), (0.0, 0.0));
    }
}
