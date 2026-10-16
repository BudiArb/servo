/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Application of `mask-image` as a WebRender image mask clip.

use app_units::Au;
use pixels::{GradientMaskKind, GradientMaskRequest, GradientMaskStop};
use style::color::AbsoluteColor;
use style::color::mix::ColorInterpolationMethod;
use style::computed_values::background_origin::single_value::T as MaskOrigin;
use style::computed_values::mask_composite::single_value::T as MaskComposite;
use style::computed_values::mask_mode::single_value::T as MaskMode;
use style::values::computed::background::BackgroundSize as Size;
use style::values::computed::image::{Gradient, Image};
use style::values::generics::image::{ColorStop, GradientFlags};
use webrender_api::units::{DeviceIntSize, LayoutPoint, LayoutRect, LayoutSize};
use webrender_api::{self as wr, ClipChainId, FillRule};

use crate::fragment_tree::BoxFragment;

use super::DisplayListBuilder;
use super::StackingContext;
use super::ToWebRender;
use super::background::get_cyclic;
use super::gradient::{
    conic_gradient_items_to_color_stops, create_webrender_stops, gradient_items_to_color_stops,
    linear_gradient_line, radial_gradient_geometry, resolve_position,
};

/// Build a WebRender clip chain that applies the `mask-image` of the given
/// stacking context, if it has one that we support rendering.
///
/// Gradient masks are rasterized on the CPU via the image cache and applied as
/// WebRender image mask clips, which stretch the rasterized mask over their
/// `rect` and sample its red channel.
pub(super) fn build_image_mask_clip_chain(
    builder: &mut DisplayListBuilder,
    stacking_context: &StackingContext,
    parent_clip_chain_id: Option<ClipChainId>,
) -> Option<ClipChainId> {
    let fragment = stacking_context.fragment()?;
    let style = fragment.style();
    let svg = style.get_svg();

    // Layers with a `none` image do not contribute to the mask.
    let layers: Vec<_> = svg
        .mask_image
        .0
        .iter()
        .enumerate()
        .filter(|(_, image)| !matches!(image, Image::None))
        .collect();
    if layers.is_empty() {
        return None;
    }

    // TODO: Support `url()` mask images and `mask-composite` values other
    // than `intersect`, which is how multiple layers are combined here since
    // a WebRender clip chain intersects its clips. Elements with unsupported
    // masks render unmasked.
    if layers.iter().any(|&(index, _)| {
        index > 0 && !matches!(get_cyclic(&svg.mask_composite.0, index), MaskComposite::Intersect)
    }) {
        return None;
    }

    let image_masks = layers
        .into_iter()
        .map(|(index, image)| match image {
            Image::Gradient(gradient) => {
                build_gradient_mask_image_mask(builder, fragment, stacking_context, index, gradient)
            },
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    let spatial_id = builder.spatial_id(stacking_context.scroll_tree_node_id);
    let clip_ids = image_masks
        .into_iter()
        .map(|image_mask| {
            builder
                .wr()
                .define_clip_image_mask(spatial_id, image_mask, &[], FillRule::Nonzero)
        })
        .collect::<Vec<_>>();
    Some(
        builder
            .wr()
            .define_clip_chain(parent_clip_chain_id, clip_ids),
    )
}

/// Rasterize the gradient of a single `mask-image` layer and return the
/// resulting WebRender image mask, positioned according to the `mask-origin`,
/// `mask-size`, and `mask-position` of that layer.
fn build_gradient_mask_image_mask(
    builder: &mut DisplayListBuilder,
    fragment: &BoxFragment,
    stacking_context: &StackingContext,
    index: usize,
    gradient: &Gradient,
) -> Option<wr::ImageMask> {
    let style = fragment.style();
    let svg = style.get_svg();

    // https://drafts.fxtf.org/css-masking-1/#the-mask-origin
    let positioning_area = match get_cyclic(&svg.mask_origin.0, index) {
        MaskOrigin::ContentBox => fragment
            .content_rect()
            .translate(stacking_context.containing_block_origin.to_vector())
            .to_webrender(),
        MaskOrigin::PaddingBox => fragment
            .padding_rect()
            .translate(stacking_context.containing_block_origin.to_vector())
            .to_webrender(),
        MaskOrigin::BorderBox => fragment
            .border_rect()
            .translate(stacking_context.containing_block_origin.to_vector())
            .to_webrender(),
    };

    // https://drafts.fxtf.org/css-masking-1/#the-mask-size
    // Gradients have no natural dimensions, so `auto`, `contain`, and `cover`
    // all size the mask to the positioning area, as does an omitted dimension
    // of an explicit size.
    let area_size = positioning_area.size();
    let tile_size = match get_cyclic(&svg.mask_size.0, index) {
        Size::Contain | Size::Cover => area_size,
        Size::ExplicitSize { width, height } => LayoutSize::new(
            width
                .non_auto()
                .map(|length| {
                    length
                        .0
                        .to_used_value(Au::from_f32_px(area_size.width))
                        .to_f32_px()
                })
                .unwrap_or(area_size.width),
            height
                .non_auto()
                .map(|length| {
                    length
                        .0
                        .to_used_value(Au::from_f32_px(area_size.height))
                        .to_f32_px()
                })
                .unwrap_or(area_size.height),
        ),
    };
    if tile_size.width <= 0.0 || tile_size.height <= 0.0 {
        return None;
    }

    // https://drafts.fxtf.org/css-masking-1/#the-mask-position
    let tile_origin = LayoutPoint::new(
        positioning_area.min.x +
            get_cyclic(&svg.mask_position_x.0, index)
                .to_used_value(Au::from_f32_px(area_size.width - tile_size.width))
                .to_f32_px(),
        positioning_area.min.y +
            get_cyclic(&svg.mask_position_y.0, index)
                .to_used_value(Au::from_f32_px(area_size.height - tile_size.height))
                .to_f32_px(),
    );
    let tile_rect = LayoutRect::from_origin_and_size(tile_origin, tile_size);

    // `mask-mode: match-source` uses the alpha channel for CSS gradients.
    let luminance = matches!(get_cyclic(&svg.mask_mode.0, index), MaskMode::Luminance);
    let size = DeviceIntSize::new(
        tile_size.width.round().max(1.0) as i32,
        tile_size.height.round().max(1.0) as i32,
    );

    // The gradient is rasterized at the tile size, with its geometry resolved
    // against the tile.
    let request = match gradient {
        Gradient::Linear {
            items,
            direction,
            color_interpolation_method,
            flags,
            compat_mode: _,
        } => {
            let (start, end) = linear_gradient_line(direction, tile_size);
            let mut color_stops =
                gradient_items_to_color_stops(&style, items, Au::from_f32_px((end - start).length()));
            GradientMaskRequest {
                kind: GradientMaskKind::Linear {
                    start: (start.x, start.y),
                    end: (end.x, end.y),
                },
                stops: gradient_mask_stops(&mut color_stops, color_interpolation_method, *flags),
                repeating: flags.contains(GradientFlags::REPEATING),
                luminance,
                size,
            }
        },
        Gradient::Radial {
            shape,
            position,
            color_interpolation_method,
            items,
            flags,
            compat_mode: _,
        } => {
            let (center, radii) = radial_gradient_geometry(shape, position, tile_size);
            let mut color_stops =
                gradient_items_to_color_stops(&style, items, Au::from_f32_px(radii.width));
            GradientMaskRequest {
                kind: GradientMaskKind::Radial {
                    center: (center.x, center.y),
                    radii: (radii.width, radii.height),
                },
                stops: gradient_mask_stops(&mut color_stops, color_interpolation_method, *flags),
                repeating: flags.contains(GradientFlags::REPEATING),
                luminance,
                size,
            }
        },
        Gradient::Conic {
            angle,
            position,
            color_interpolation_method,
            items,
            flags,
        } => {
            let center = resolve_position(position, tile_size);
            let mut color_stops = conic_gradient_items_to_color_stops(&style, items);
            GradientMaskRequest {
                kind: GradientMaskKind::Conic {
                    center: (center.x, center.y),
                    start_angle: angle.radians(),
                },
                stops: gradient_mask_stops(&mut color_stops, color_interpolation_method, *flags),
                repeating: flags.contains(GradientFlags::REPEATING),
                luminance,
                size,
            }
        },
    };

    let image = builder.image_resolver.rasterize_gradient_mask(&request)?;
    let image_key = image.id?;
    Some(wr::ImageMask { image: image_key, rect: tile_rect })
}

/// Convert the fixed-up color stops of a gradient into mask stops,
/// interpolating in non-sRGB color spaces like the WebRender gradient path
/// does.
fn gradient_mask_stops(
    color_stops: &mut [ColorStop<AbsoluteColor, f32>],
    color_interpolation_method: &ColorInterpolationMethod,
    flags: GradientFlags,
) -> Vec<GradientMaskStop> {
    let extend_mode = if flags.contains(GradientFlags::REPEATING) {
        wr::ExtendMode::Repeat
    } else {
        wr::ExtendMode::Clamp
    };
    create_webrender_stops(color_stops, color_interpolation_method, extend_mode)
        .into_iter()
        .map(|stop| GradientMaskStop {
            offset: stop.offset,
            color: [stop.color.r, stop.color.g, stop.color.b, stop.color.a],
        })
        .collect()
}
