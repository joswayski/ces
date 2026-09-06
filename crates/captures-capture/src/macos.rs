use core_foundation::{base::TCFType, string::CFString};
use core_graphics::{
    base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast},
    color_space::CGColorSpace,
    context::CGContext,
    display::CGDisplay,
    geometry::{CGPoint, CGRect, CGSize},
    image::CGImage,
    window::{kCGWindowImageDefault, kCGWindowListOptionAll},
};
use image::RgbaImage;

use crate::{CaptureError, CaptureResult};

pub(crate) fn capture_display(id: u32) -> CaptureResult<RgbaImage> {
    // Match xcap's composite and resolution, but retain the CGImage's profile
    // until color conversion. xcap copies its bytes and discards that profile.
    let source = CGDisplay::screenshot(
        CGDisplay::new(id).bounds(),
        kCGWindowListOptionAll,
        0,
        kCGWindowImageDefault,
    )
    .ok_or(CaptureError::SessionUnavailable)?;
    image_to_srgb(&source)
}

fn image_to_srgb(source: &CGImage) -> CaptureResult<RgbaImage> {
    // Both the frozen preview and its final crop use sRGB-tagged PNGs. Drawing
    // while the source profile is attached lets Core Graphics transform the
    // pixels, rather than relabeling display-native RGB values as sRGB.
    let name = CFString::new("kCGColorSpaceSRGB");
    let srgb = CGColorSpace::create_with_name(name.as_concrete_TypeRef())
        .ok_or_else(|| CaptureError::Image("sRGB color space is unavailable".into()))?;
    let width = source.width();
    let height = source.height();
    let mut context = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width * 4,
        &srgb,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
    );
    context.draw_image(
        CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width as f64, height as f64),
        ),
        source,
    );
    let mut pixels = context.data().to_vec();
    // image::RgbaImage uses straight alpha; Core Graphics bitmap contexts use
    // premultiplied alpha. Full display composites are normally opaque.
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width as u32, height as u32, pixels)
        .ok_or_else(|| CaptureError::Image("invalid sRGB capture dimensions".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::{base::kCGRenderingIntentDefault, data_provider::CGDataProvider};
    use std::sync::Arc;

    fn fixture(space: &str, pixels: Vec<u8>, width: usize, height: usize) -> CGImage {
        let name = CFString::new(space);
        let space = CGColorSpace::create_with_name(name.as_concrete_TypeRef()).unwrap();
        CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            &space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
            &CGDataProvider::from_buffer(Arc::new(pixels)),
            false,
            kCGRenderingIntentDefault,
        )
    }

    #[test]
    fn srgb_capture_preserves_channels_and_row_orientation() {
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 64, 64, 64, 255,
        ];
        let source = fixture("kCGColorSpaceSRGB", pixels.clone(), 2, 2);
        let image = image_to_srgb(&source).unwrap();
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(image.into_raw(), pixels);
    }

    #[test]
    fn display_p3_pixels_are_converted_not_relabelled_as_srgb() {
        // In-gamut Display P3 (180, 100, 80) maps to approximately sRGB
        // (193, 95, 74), not the unchanged, less saturated source triplet.
        let source = fixture("kCGColorSpaceDisplayP3", vec![180, 100, 80, 255], 1, 1);
        let image = image_to_srgb(&source).unwrap();
        let actual = image.get_pixel(0, 0).0;
        for (channel, expected) in actual.into_iter().zip([193_u8, 95, 74, 255]) {
            assert!(
                channel.abs_diff(expected) <= 3,
                "converted pixel: {actual:?}"
            );
        }
    }

    #[test]
    fn capture_returns_straight_alpha() {
        let source = fixture("kCGColorSpaceSRGB", vec![64, 32, 16, 128], 1, 1);
        let image = image_to_srgb(&source).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [128, 64, 32, 128]);
    }
}
