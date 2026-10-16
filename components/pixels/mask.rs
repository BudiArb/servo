/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Rasterization of CSS gradients into masks for `mask-image`, for use with
//! WebRender image mask clips.
//!
//! WebRender image masks sample the *red* channel of their texture, so the
//! computed mask value (the alpha or the luminance of the gradient) is written
//! to every channel of the BGRA8 output.

use std::hash::{Hash, Hasher};

use malloc_size_of_derive::MallocSizeOf;
use webrender_api::units::DeviceIntSize;

/// The shape of a gradient used as a `mask-image`, with its geometry already
/// resolved against the mask positioning area, in pixels relative to the
/// top-left corner of the mask tile.
#[derive(Clone, Copy, Debug, MallocSizeOf)]
pub enum GradientMaskKind {
    /// A radial gradient with a resolved center and ending shape radii.
    Radial {
        center: (f32, f32),
        radii: (f32, f32),
    },
    /// A linear gradient with the resolved start and end points of the
    /// gradient line.
    Linear {
        start: (f32, f32),
        end: (f32, f32),
    },
    /// A conic gradient with a resolved center and a starting angle in
    /// radians, following the CSS convention where `0deg` points up and
    /// angles increase clockwise.
    Conic {
        center: (f32, f32),
        start_angle: f32,
    },
}

/// One color stop of a gradient mask, with a fixed-up position in `[0, 1]` and
/// a resolved non-premultiplied RGBA color.
#[derive(Clone, Copy, Debug, MallocSizeOf)]
pub struct GradientMaskStop {
    pub offset: f32,
    pub color: [f32; 4],
}

/// A request to rasterize a gradient mask at a given size.
///
/// This is plain data (no `style` types) so that the results can be cached by
/// the image cache. Floating point fields are compared bitwise, which is
/// stable because the values are produced by the same computation on every
/// display list construction.
#[derive(Clone, Debug, MallocSizeOf)]
pub struct GradientMaskRequest {
    pub kind: GradientMaskKind,
    pub stops: Vec<GradientMaskStop>,
    /// Whether the gradient repeats outside of the `[0, 1]` stop range.
    pub repeating: bool,
    /// Whether the mask value is the luminance of the gradient
    /// (`mask-mode: luminance`) rather than its alpha.
    pub luminance: bool,
    pub size: DeviceIntSize,
}

impl PartialEq for GradientMaskRequest {
    fn eq(&self, other: &Self) -> bool {
        let gradient_mask_kind_eq = match (self.kind, other.kind) {
            (
                GradientMaskKind::Radial {
                    center: center_a,
                    radii: radii_a,
                },
                GradientMaskKind::Radial {
                    center: center_b,
                    radii: radii_b,
                },
            ) => {
                center_a.0.to_bits() == center_b.0.to_bits() &&
                    center_a.1.to_bits() == center_b.1.to_bits() &&
                    radii_a.0.to_bits() == radii_b.0.to_bits() &&
                    radii_a.1.to_bits() == radii_b.1.to_bits()
            },
            (
                GradientMaskKind::Linear {
                    start: start_a,
                    end: end_a,
                },
                GradientMaskKind::Linear {
                    start: start_b,
                    end: end_b,
                },
            ) => {
                start_a.0.to_bits() == start_b.0.to_bits() &&
                    start_a.1.to_bits() == start_b.1.to_bits() &&
                    end_a.0.to_bits() == end_b.0.to_bits() &&
                    end_a.1.to_bits() == end_b.1.to_bits()
            },
            (
                GradientMaskKind::Conic {
                    center: center_a,
                    start_angle: start_angle_a,
                },
                GradientMaskKind::Conic {
                    center: center_b,
                    start_angle: start_angle_b,
                },
            ) => {
                center_a.0.to_bits() == center_b.0.to_bits() &&
                    center_a.1.to_bits() == center_b.1.to_bits() &&
                    start_angle_a.to_bits() == start_angle_b.to_bits()
            },
            _ => false,
        };
        self.stops.len() == other.stops.len() &&
            self.stops
                .iter()
                .zip(other.stops.iter())
                .all(|(stop_a, stop_b)| {
                    stop_a.offset.to_bits() == stop_b.offset.to_bits() &&
                        stop_a.color == stop_b.color
                }) &&
            self.repeating == other.repeating &&
            self.luminance == other.luminance &&
            self.size == other.size &&
            gradient_mask_kind_eq
    }
}

impl Eq for GradientMaskRequest {}

impl Hash for GradientMaskRequest {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.kind {
            GradientMaskKind::Radial { center, radii } => {
                0u8.hash(state);
                center.0.to_bits().hash(state);
                center.1.to_bits().hash(state);
                radii.0.to_bits().hash(state);
                radii.1.to_bits().hash(state);
            },
            GradientMaskKind::Linear { start, end } => {
                1u8.hash(state);
                start.0.to_bits().hash(state);
                start.1.to_bits().hash(state);
                end.0.to_bits().hash(state);
                end.1.to_bits().hash(state);
            },
            GradientMaskKind::Conic {
                center,
                start_angle,
            } => {
                2u8.hash(state);
                center.0.to_bits().hash(state);
                center.1.to_bits().hash(state);
                start_angle.to_bits().hash(state);
            },
        }
        self.stops.len().hash(state);
        for stop in &self.stops {
            stop.offset.to_bits().hash(state);
            for channel in stop.color {
                channel.to_bits().hash(state);
            }
        }
        self.repeating.hash(state);
        self.luminance.hash(state);
        self.size.hash(state);
    }
}

/// Rasterize the gradient described by `request` into BGRA8 pixels where every
/// channel of a pixel holds the mask value for that pixel.
///
/// Returns `None` when the request has no color stops or when its size is
/// empty or too large to rasterize.
pub fn rasterize_gradient_mask(request: &GradientMaskRequest) -> Option<Vec<u8>> {
    /// Masks are rasterized at layout pixel resolution, so this only guards
    /// against unreasonably large allocations.
    const MAX_MASK_SIDE_LENGTH: i32 = 4096;

    if request.stops.is_empty() ||
        request.size.width <= 0 ||
        request.size.height <= 0 ||
        request.size.width > MAX_MASK_SIDE_LENGTH ||
        request.size.height > MAX_MASK_SIDE_LENGTH
    {
        return None;
    }

    let width = request.size.width as usize;
    let height = request.size.height as usize;
    let mut pixels = vec![0u8; width * height * 4];

    // https://drafts.csswg.org/css-images-3/#gradients
    // > If the gradient's line has zero length ... the gradient must be
    // > rendered as a solid color image whose color is the color of the last
    // > color stop.
    //
    // The same applies to radial gradients whose ending shape has both radii
    // equal to zero.
    let degenerate = match request.kind {
        GradientMaskKind::Radial { radii, .. } => radii.0 == 0.0 && radii.1 == 0.0,
        GradientMaskKind::Linear { start, end } => start == end,
        GradientMaskKind::Conic { .. } => false,
    };
    if degenerate {
        let color = request.stops.last().unwrap().color;
        let value = mask_value(color, request.luminance);
        let byte = mask_value_to_byte(value);
        pixels.chunks_mut(4).for_each(|pixel| pixel.fill(byte));
        return Some(pixels);
    }

    for (index, pixel) in pixels.chunks_mut(4).enumerate() {
        let position = (
            (index % width) as f32 + 0.5,
            (index / width) as f32 + 0.5,
        );
        let progress = match request.kind {
            GradientMaskKind::Radial { center, radii } => {
                radial_progress(position, center, radii)
            },
            GradientMaskKind::Linear { start, end } => linear_progress(position, start, end),
            GradientMaskKind::Conic {
                center,
                start_angle,
            } => conic_progress(position, center, start_angle),
        };
        let progress = if request.repeating {
            progress.rem_euclid(1.0)
        } else {
            progress
        };
        let color = sample_color(&request.stops, progress);
        let byte = mask_value_to_byte(mask_value(color, request.luminance));
        pixel.fill(byte);
    }

    Some(pixels)
}

/// Interpolation position of `position` in a radial gradient with the given
/// center and ending shape radii.
fn radial_progress(position: (f32, f32), center: (f32, f32), radii: (f32, f32)) -> f32 {
    let dx = (position.0 - center.0) / radii.0;
    let dy = (position.1 - center.1) / radii.1;
    dx.hypot(dy)
}

/// Interpolation position of `position` on the gradient line from `start` to
/// `end`.
fn linear_progress(position: (f32, f32), start: (f32, f32), end: (f32, f32)) -> f32 {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    ((position.0 - start.0) * dx + (position.1 - start.1) * dy) / (dx * dx + dy * dy)
}

/// Interpolation position of `position` in a conic gradient with the given
/// center and starting angle.
fn conic_progress(position: (f32, f32), center: (f32, f32), start_angle: f32) -> f32 {
    let dx = position.0 - center.0;
    let dy = position.1 - center.1;
    // The angle is measured from the direction pointing up, increasing
    // clockwise, matching the CSS conic gradient convention.
    let angle = dx.atan2(-dy);
    ((angle - start_angle) / (2.0 * std::f32::consts::PI)).rem_euclid(1.0)
}

/// Sample the color of the gradient at the given interpolation position,
/// clamping to the color of the first or last stop outside of the stop range.
fn sample_color(stops: &[GradientMaskStop], progress: f32) -> [f32; 4] {
    let partition = stops.partition_point(|stop| stop.offset < progress);
    if partition == 0 {
        return stops[0].color;
    }
    if partition == stops.len() {
        return stops[stops.len() - 1].color;
    }

    let (start, end) = (&stops[partition - 1], &stops[partition]);
    let span = end.offset - start.offset;
    let amount = if span <= 0.0 {
        0.0
    } else {
        ((progress - start.offset) / span).clamp(0.0, 1.0)
    };

    let mut color = [0.0; 4];
    for (channel, (start_channel, end_channel)) in color
        .iter_mut()
        .zip(start.color.iter().zip(end.color.iter()))
    {
        *channel = start_channel + (end_channel - start_channel) * amount;
    }
    color
}

/// The mask value of a color: its alpha, or its luminance in
/// `mask-mode: luminance`.
fn mask_value(color: [f32; 4], luminance: bool) -> f32 {
    if luminance {
        0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
    } else {
        color[3]
    }
}

fn mask_value_to_byte(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod test {
    use super::*;

    fn solid_request(kind: GradientMaskKind, size: DeviceIntSize) -> GradientMaskRequest {
        GradientMaskRequest {
            kind,
            stops: vec![GradientMaskStop {
                offset: 0.0,
                color: [0.0, 0.0, 0.0, 1.0],
            }],
            repeating: false,
            luminance: false,
            size,
        }
    }

    fn pixels_to_values(pixels: &[u8]) -> Vec<u8> {
        pixels.chunks(4).map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn empty_requests_are_rejected() {
        let mut request = solid_request(
            GradientMaskKind::Radial {
                center: (1.0, 1.0),
                radii: (1.0, 1.0),
            },
            DeviceIntSize::new(2, 2),
        );
        request.stops.clear();
        assert!(rasterize_gradient_mask(&request).is_none());

        request.stops = vec![GradientMaskStop {
            offset: 0.0,
            color: [1.0; 4],
        }];
        request.size = DeviceIntSize::new(0, 2);
        assert!(rasterize_gradient_mask(&request).is_none());
    }

    #[test]
    fn single_stop_is_solid() {
        let request = solid_request(
            GradientMaskKind::Radial {
                center: (1.0, 1.0),
                radii: (1.0, 1.0),
            },
            DeviceIntSize::new(2, 2),
        );
        let pixels = rasterize_gradient_mask(&request).unwrap();
        assert_eq!(pixels, vec![255u8; 16]);
    }

    #[test]
    fn degenerate_shapes_render_the_last_stop() {
        let mut request = solid_request(
            GradientMaskKind::Linear {
                start: (1.0, 1.0),
                end: (1.0, 1.0),
            },
            DeviceIntSize::new(2, 1),
        );
        request.stops = vec![
            GradientMaskStop {
                offset: 0.0,
                color: [1.0, 1.0, 1.0, 1.0],
            },
            GradientMaskStop {
                offset: 1.0,
                color: [0.0, 0.0, 0.0, 0.0],
            },
        ];
        let pixels = rasterize_gradient_mask(&request).unwrap();
        assert_eq!(pixels, vec![0u8; 8]);
    }

    #[test]
    fn linear_gradient_interpolates_along_the_line() {
        let request = GradientMaskRequest {
            kind: GradientMaskKind::Linear {
                start: (0.0, 0.0),
                end: (4.0, 0.0),
            },
            stops: vec![
                GradientMaskStop {
                    offset: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                },
                GradientMaskStop {
                    offset: 1.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                },
            ],
            repeating: false,
            luminance: false,
            size: DeviceIntSize::new(4, 1),
        };
        let pixels = rasterize_gradient_mask(&request).unwrap();
        // Pixel centers are at 0.5, 1.5, 2.5, 3.5 along a 4px gradient line,
        // so t is 1/8, 3/8, 5/8, 7/8 and the alpha is 1 - t.
        assert_eq!(pixels_to_values(&pixels), vec![223, 159, 96, 32]);
    }

    #[test]
    fn non_repeating_gradients_clamp_to_end_stops() {
        let request = GradientMaskRequest {
            kind: GradientMaskKind::Radial {
                center: (2.0, 0.5),
                radii: (1.0, 1.0),
            },
            stops: vec![
                GradientMaskStop {
                    offset: 0.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                },
                GradientMaskStop {
                    offset: 1.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                },
            ],
            repeating: false,
            luminance: false,
            size: DeviceIntSize::new(4, 1),
        };
        let pixels = rasterize_gradient_mask(&request).unwrap();
        // The pixel centers are at distance 1.5, 0.5, 0.5, 1.5 from the
        // center. The outer pixels are outside of the ending shape and
        // clamped to the last stop, the inner ones are halfway through it.
        assert_eq!(pixels_to_values(&pixels), vec![255, 128, 128, 255]);
    }

    #[test]
    fn repeating_gradients_wrap_around() {
        let request = GradientMaskRequest {
            kind: GradientMaskKind::Linear {
                start: (0.0, 0.0),
                end: (2.0, 0.0),
            },
            stops: vec![
                GradientMaskStop {
                    offset: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                },
                GradientMaskStop {
                    offset: 1.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                },
            ],
            repeating: true,
            luminance: false,
            size: DeviceIntSize::new(4, 1),
        };
        let pixels = rasterize_gradient_mask(&request).unwrap();
        // The gradient repeats every two pixels.
        assert_eq!(pixels_to_values(&pixels), vec![191, 64, 191, 64]);
    }

    #[test]
    fn luminance_mode_uses_color_channels() {
        let request = GradientMaskRequest {
            kind: GradientMaskKind::Linear {
                start: (0.0, 0.0),
                end: (2.0, 0.0),
            },
            stops: vec![GradientMaskStop {
                offset: 0.0,
                color: [1.0, 0.0, 0.0, 1.0],
            }],
            repeating: false,
            luminance: true,
            size: DeviceIntSize::new(2, 1),
        };
        let pixels = rasterize_gradient_mask(&request).unwrap();
        // 0.2126 * 255 == 54.
        assert_eq!(pixels_to_values(&pixels), vec![54, 54]);
    }

    #[test]
    fn conic_gradient_rotates_from_start_angle() {
        let request = GradientMaskRequest {
            kind: GradientMaskKind::Conic {
                center: (2.0, 0.5),
                start_angle: 0.0,
            },
            stops: vec![
                GradientMaskStop {
                    offset: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                },
                GradientMaskStop {
                    offset: 1.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                },
            ],
            repeating: false,
            luminance: false,
            size: DeviceIntSize::new(4, 1),
        };
        let pixels = rasterize_gradient_mask(&request).unwrap();
        // All pixel centers are on the horizontal line through the center.
        // Angles from up, clockwise: the first two pixels are due west
        // (270deg, t = 0.75), the last two due east (90deg, t = 0.25).
        assert_eq!(pixels_to_values(&pixels), vec![64, 64, 191, 191]);

        let request = GradientMaskRequest {
            kind: GradientMaskKind::Conic {
                center: (2.0, 0.5),
                start_angle: std::f32::consts::FRAC_PI_2,
            },
            ..request
        };
        let pixels = rasterize_gradient_mask(&request).unwrap();
        // Starting at 90deg rotates the gradient so that due east is at
        // t = 0 and due west at t = 0.5.
        assert_eq!(pixels_to_values(&pixels), vec![128, 128, 255, 255]);
    }

    #[test]
    fn equal_requests_hash_and_compare_equal() {
        let kind = GradientMaskKind::Radial {
            center: (1.0, 2.0),
            radii: (3.0, 4.0),
        };
        let first = solid_request(kind, DeviceIntSize::new(10, 10));
        let mut second = solid_request(kind, DeviceIntSize::new(10, 10));
        assert_eq!(first, second);

        second.size = DeviceIntSize::new(11, 10);
        assert_ne!(first, second);

        let mut map = std::collections::HashMap::new();
        map.insert(first, 1);
        second.size = DeviceIntSize::new(10, 10);
        assert_eq!(map.get(&second), Some(&1));
    }
}
