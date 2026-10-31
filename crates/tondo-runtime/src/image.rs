//! The images of `std.image` (standard library 14.38): PNG, JPEG, GIF and
//! WebP read and written with the `image` crate, and what changes their
//! pixels, on the host's threads. An image never changes: each operation
//! gives a new one.

use super::native::{self, NativeObject};
use super::{ArrayHeader, Str, element, str_new};
use crate::io::byte_array;
use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader, Rgba, RgbaImage, imageops::FilterType};
use std::io::Cursor;

/// An image and the format it was read from, or why there is none.
enum Picture {
    Ready(DynamicImage, i64),
    Failed(String),
}

/// The formats of `std.image`'s `Format`, in order.
const FORMATS: [ImageFormat; 4] = [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Gif, ImageFormat::WebP];

fn code(format: Option<ImageFormat>) -> i64 {
    format.and_then(|f| FORMATS.iter().position(|&known| known == f)).map_or(-1, |i| i as i64)
}

fn bytes(array: *mut ArrayHeader) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(element(array, 1, 0), (*array).len).to_vec() }
}

fn ints(values: &[i64]) -> *mut ArrayHeader {
    let array = super::tondo_rt_array_new(8, values.len());
    for (index, &value) in values.iter().enumerate() {
        unsafe { *(element(array, 8, index) as *mut i64) = value };
    }
    array
}

fn picture<'a>(object: *mut NativeObject) -> Option<&'a DynamicImage> {
    match native::get::<Picture>(object) {
        Some(Picture::Ready(image, _)) => Some(image),
        _ => None,
    }
}

/// Runs `work` on the image on a host thread; the image outlives the work,
/// which the program waits for.
fn with<R: Send + 'static>(
    object: *mut NativeObject,
    work: impl FnOnce(&DynamicImage) -> R + Send + 'static,
) -> Option<R> {
    let image: &'static DynamicImage = unsafe { &*(picture(object)? as *const DynamicImage) };
    Some(crate::io::host(true, move || work(image)))
}

fn made(image: Option<DynamicImage>, format: i64) -> *mut NativeObject {
    native::new(match image {
        Some(image) => Picture::Ready(image, format),
        None => Picture::Failed("the image is not there".into()),
    })
}

/// `read`: the image of the bytes, turned as its EXIF orientation says.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_image_read(data: *mut ArrayHeader) -> *mut NativeObject {
    let data = bytes(data);
    let read = crate::io::host(true, move || {
        let reader = ImageReader::new(Cursor::new(data)).with_guessed_format()?;
        let format = code(reader.format());
        let mut decoder = reader.into_decoder()?;
        let orientation = image::ImageDecoder::orientation(&mut decoder)?;
        let mut image = DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        Ok::<_, image::ImageError>((image, format))
    });
    native::new(match read {
        Ok((image, format)) => Picture::Ready(image, format),
        Err(error) => Picture::Failed(error.to_string()),
    })
}

/// Why an image could not be read, or nothing.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_failure(object: *mut NativeObject) -> *mut Str {
    match native::get::<Picture>(object) {
        Some(Picture::Failed(why)) => str_new(why.as_bytes()),
        _ => str_new(b""),
    }
}

/// `info`: the format, width and height its header gives, as its EXIF
/// orientation shows it, without the pixels; empty when it is not an image.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_image_info(data: *mut ArrayHeader) -> *mut ArrayHeader {
    use image::{ImageDecoder, metadata::Orientation};
    let read = || {
        let reader = ImageReader::new(Cursor::new(bytes(data))).with_guessed_format().ok()?;
        let format = code(reader.format());
        let mut decoder = reader.into_decoder().ok()?;
        let (width, height) = decoder.dimensions();
        let turned = matches!(
            decoder.orientation().ok()?,
            Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH
        );
        let (width, height) = if turned { (height, width) } else { (width, height) };
        (format >= 0).then_some([format, width as i64, height as i64])
    };
    match read() {
        Some(found) => ints(&found),
        None => ints(&[]),
    }
}

/// The width, height and format an image was read from (-1 for one made).
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_size(object: *mut NativeObject) -> *mut ArrayHeader {
    match native::get::<Picture>(object) {
        Some(Picture::Ready(image, format)) => ints(&[image.width() as i64, image.height() as i64, *format]),
        _ => ints(&[0, 0, -1]),
    }
}

/// `blank`: an image of one color, as `0xRRGGBBAA`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_blank(width: i64, height: i64, color: i64) -> *mut NativeObject {
    let pixel = Rgba((color as u32).to_be_bytes());
    let image = RgbaImage::from_pixel(width.max(0) as u32, height.max(0) as u32, pixel);
    made(Some(DynamicImage::ImageRgba8(image)), -1)
}

/// Changes an image: `operation` 0 resizes to `a` by `b` with the filter
/// `c`, 1 crops `c` by `d` at (`a`, `b`), 2 to 4 turn it by quarters, 5 and
/// 6 flip it across and down, and 7 makes it gray.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_change(
    object: *mut NativeObject,
    operation: i64,
    a: i64,
    b: i64,
    c: i64,
    d: i64,
) -> *mut NativeObject {
    let (a, b, c, d) = (a.max(0) as u32, b.max(0) as u32, c.max(0) as u32, d.max(0) as u32);
    let changed = with(object, move |image| match operation {
        0 => {
            let filter = [FilterType::Nearest, FilterType::Triangle, FilterType::CatmullRom, FilterType::Lanczos3];
            image.resize_exact(a, b, filter[(c as usize).min(3)])
        }
        1 => image.crop_imm(a, b, c, d),
        2 => image.rotate90(),
        3 => image.rotate180(),
        4 => image.rotate270(),
        5 => image.fliph(),
        6 => image.flipv(),
        _ => image.grayscale(),
    });
    made(changed, -1)
}

/// A Gaussian blur of `sigma` pixels.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_blur(object: *mut NativeObject, sigma: f64) -> *mut NativeObject {
    made(with(object, move |image| image.fast_blur(sigma.max(0.0) as f32)), -1)
}

/// `top` drawn over the image at (`x`, `y`), blended by its alpha.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_overlay(
    object: *mut NativeObject,
    top: *mut NativeObject,
    x: i64,
    y: i64,
) -> *mut NativeObject {
    let Some(top) = picture(top) else { return made(None, -1) };
    let top: &'static DynamicImage = unsafe { &*(top as *const DynamicImage) };
    let drawn = with(object, move |image| {
        let mut base = image.to_rgba8();
        image::imageops::overlay(&mut base, &top.to_rgba8(), x, y);
        DynamicImage::ImageRgba8(base)
    });
    made(drawn, -1)
}

/// The color of the pixel at (`x`, `y`) as `0xRRGGBBAA`, or -1 outside.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_pixel(object: *mut NativeObject, x: i64, y: i64) -> i64 {
    match picture(object) {
        Some(image) if x >= 0 && y >= 0 && (x as u32) < image.width() && (y as u32) < image.height() => {
            u32::from_be_bytes(image.get_pixel(x as u32, y as u32).0) as i64
        }
        _ => -1,
    }
}

/// The image in a format, JPEG at a quality from 1 to 100; empty when it
/// cannot be written.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_encode(object: *mut NativeObject, format: i64, quality: i64) -> *mut ArrayHeader {
    let written = with(object, move |image| {
        let mut out = Cursor::new(Vec::new());
        let done = match format {
            1 => {
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100) as u8);
                DynamicImage::ImageRgb8(image.to_rgb8()).write_with_encoder(encoder)
            }
            2 => DynamicImage::ImageRgba8(image.to_rgba8()).write_to(&mut out, ImageFormat::Gif),
            3 => DynamicImage::ImageRgba8(image.to_rgba8()).write_to(&mut out, ImageFormat::WebP),
            _ => image.write_to(&mut out, ImageFormat::Png),
        };
        done.map(|()| out.into_inner()).unwrap_or_default()
    });
    byte_array(&written.unwrap_or_default())
}

/// `pixels`: the red, green, blue and alpha bytes of each pixel, row by row
/// from the top left.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_pixels(object: *mut NativeObject) -> *mut ArrayHeader {
    byte_array(&with(object, |image| image.to_rgba8().into_raw()).unwrap_or_default())
}

/// `fromPixels`: an image of `width` by `height` of those bytes, four a
/// pixel; failed when they are not as many.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_image_from_pixels(
    width: i64,
    height: i64,
    data: *mut ArrayHeader,
) -> *mut NativeObject {
    let data = bytes(data);
    let (wide, high) = (width.max(0) as u32, height.max(0) as u32);
    match RgbaImage::from_raw(wide, high, data) {
        Some(image) if width >= 0 && height >= 0 => native::new(Picture::Ready(DynamicImage::ImageRgba8(image), -1)),
        _ => native::new(Picture::Failed(format!(
            "{width} by {height} pixels take {} bytes, four a pixel",
            width.max(0) * height.max(0) * 4
        ))),
    }
}

/// Each pixel's channels, from 0 to 1, changed by `change`, alpha kept.
fn each_pixel(image: &DynamicImage, change: impl Fn([f32; 3]) -> [f32; 3]) -> DynamicImage {
    let mut pixels = image.to_rgba8();
    for pixel in pixels.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let changed = change([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]);
        let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        pixel.0 = [byte(changed[0]), byte(changed[1]), byte(changed[2]), a];
    }
    DynamicImage::ImageRgba8(pixels)
}

/// Hue, saturation and lightness, each from 0 to 1, of a color.
fn hsl([r, g, b]: [f32; 3]) -> [f32; 3] {
    let (high, low) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (high + low) / 2.0;
    if high == low {
        return [0.0, 0.0, lightness];
    }
    let spread = high - low;
    let saturation = if lightness > 0.5 { spread / (2.0 - high - low) } else { spread / (high + low) };
    let hue = if high == r {
        (g - b) / spread + if g < b { 6.0 } else { 0.0 }
    } else if high == g {
        (b - r) / spread + 2.0
    } else {
        (r - g) / spread + 4.0
    };
    [hue / 6.0, saturation, lightness]
}

fn rgb([hue, saturation, lightness]: [f32; 3]) -> [f32; 3] {
    if saturation == 0.0 {
        return [lightness; 3];
    }
    let q =
        if lightness < 0.5 { lightness * (1.0 + saturation) } else { lightness + saturation - lightness * saturation };
    let p = 2.0 * lightness - q;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [channel(hue + 1.0 / 3.0), channel(hue), channel(hue - 1.0 / 3.0)]
}

/// `adjust`: brightness and contrast from -100 to 100, saturation from -100
/// to 500, a gamma over 0 and a hue in degrees, in Hugo's scales, applied in
/// that order: brightness, contrast, gamma, saturation, hue.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_adjust(
    object: *mut NativeObject,
    brightness: f64,
    contrast: f64,
    saturation: f64,
    gamma: f64,
    hue: f64,
) -> *mut NativeObject {
    let shift = brightness.clamp(-100.0, 100.0) as f32 / 100.0;
    let percent = contrast.clamp(-100.0, 100.0) as f32;
    let stretch = if percent <= 0.0 { 1.0 + percent / 100.0 } else { 100.0 / (100.0 - percent).max(1e-3) };
    let exponent = 1.0 / gamma.max(1e-5) as f32;
    let saturated = 1.0 + saturation.clamp(-100.0, 500.0) as f32 / 100.0;
    let turn = hue.clamp(-180.0, 180.0) as f32 / 360.0;
    let changed = with(object, move |image| {
        each_pixel(image, |channels| {
            let level = |x: f32| (((x + shift).clamp(0.0, 1.0) - 0.5) * stretch + 0.5).clamp(0.0, 1.0).powf(exponent);
            let leveled = channels.map(level);
            if saturated == 1.0 && turn == 0.0 {
                return leveled;
            }
            let [h, s, l] = hsl(leveled);
            rgb([(h + turn).rem_euclid(1.0), (s * saturated).min(1.0), l])
        })
    });
    made(changed, -1)
}

/// The tones of an image: `operation` 0 inverts it, 1 makes it sepia by
/// `amount` percent, 2 sharpens it by `amount` over a blur of `sigma`, 3
/// pixelates it in squares of `amount` pixels and 4 multiplies its alpha by
/// `amount`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_tone(
    object: *mut NativeObject,
    operation: i64,
    amount: f64,
    sigma: f64,
) -> *mut NativeObject {
    let amount = amount as f32;
    let changed = with(object, move |image| match operation {
        0 => each_pixel(image, |channels| channels.map(|x| 1.0 - x)),
        1 => {
            let k = amount.clamp(0.0, 100.0) / 100.0;
            each_pixel(image, |[r, g, b]| {
                [
                    r * (1.0 - 0.607 * k) + g * 0.769 * k + b * 0.189 * k,
                    r * 0.349 * k + g * (1.0 - 0.314 * k) + b * 0.168 * k,
                    r * 0.272 * k + g * 0.534 * k + b * (1.0 - 0.869 * k),
                ]
            })
        }
        2 => {
            let original = image.to_rgba8();
            let blurred = image::imageops::blur(&original, sigma.max(0.1) as f32);
            let mut sharp = original.clone();
            for (pixel, soft) in sharp.pixels_mut().zip(blurred.pixels()) {
                for channel in 0..3 {
                    let (x, y) = (pixel.0[channel] as f32, soft.0[channel] as f32);
                    pixel.0[channel] = (x + (x - y) * amount).clamp(0.0, 255.0).round() as u8;
                }
            }
            DynamicImage::ImageRgba8(sharp)
        }
        3 => {
            let size = (amount.round() as u32).max(1);
            let mut pixels = image.to_rgba8();
            let (width, height) = pixels.dimensions();
            for top in (0..height).step_by(size as usize) {
                for left in (0..width).step_by(size as usize) {
                    let (right, bottom) = ((left + size).min(width), (top + size).min(height));
                    let mut sum = [0u64; 4];
                    for y in top..bottom {
                        for x in left..right {
                            for (total, value) in sum.iter_mut().zip(pixels.get_pixel(x, y).0) {
                                *total += value as u64;
                            }
                        }
                    }
                    let count = ((right - left) * (bottom - top)) as u64;
                    let average = Rgba(sum.map(|total| (total / count) as u8));
                    for y in top..bottom {
                        for x in left..right {
                            pixels.put_pixel(x, y, average);
                        }
                    }
                }
            }
            DynamicImage::ImageRgba8(pixels)
        }
        _ => {
            let mut pixels = image.to_rgba8();
            let factor = amount.clamp(0.0, 1.0);
            for pixel in pixels.pixels_mut() {
                pixel.0[3] = (pixel.0[3] as f32 * factor).round() as u8;
            }
            DynamicImage::ImageRgba8(pixels)
        }
    });
    made(changed, -1)
}

/// `colors`: up to `count` colors that cover most of an image, as
/// `0xRRGGBBAA`, the most of it first: its pixels, on a small copy, in
/// buckets of similar colors, each given by its most frequent color when it
/// is far enough from those given before.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_image_colors(object: *mut NativeObject, count: i64) -> *mut ArrayHeader {
    let found = with(object, move |image| {
        // Fewer pixels to count, none of them mixed from others.
        let small = if image.width() > 64 || image.height() > 64 {
            image.resize(64, 64, FilterType::Nearest).to_rgba8()
        } else {
            image.to_rgba8()
        };
        let mut exact: std::collections::HashMap<[u8; 3], u64> = std::collections::HashMap::new();
        for pixel in small.pixels().filter(|pixel| pixel.0[3] >= 128) {
            let [r, g, b, _] = pixel.0;
            *exact.entry([r, g, b]).or_default() += 1;
        }
        // Each bucket's pixels, and its most frequent color.
        let mut buckets: std::collections::HashMap<[u8; 3], (u64, u64, [u8; 3])> = std::collections::HashMap::new();
        for (color, seen) in exact {
            let (total, best, chosen) = buckets.entry(color.map(|c| c >> 4)).or_default();
            *total += seen;
            if seen > *best || (seen == *best && color < *chosen) {
                (*best, *chosen) = (seen, color);
            }
        }
        let mut ranked: Vec<(u64, [u8; 3])> = buckets.into_values().map(|(total, _, color)| (total, color)).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut chosen: Vec<[u8; 3]> = Vec::new();
        for (_, color) in ranked {
            let near = |other: &[u8; 3]| {
                other.iter().zip(color).map(|(&a, b)| (a as i64 - b as i64).pow(2)).sum::<i64>() < 48 * 48
            };
            if !chosen.iter().any(near) {
                chosen.push(color);
            }
            if chosen.len() as i64 >= count {
                break;
            }
        }
        chosen
            .iter()
            .map(|&[r, g, b]| (r as i64) << 24 | (g as i64) << 16 | (b as i64) << 8 | 255)
            .collect::<Vec<i64>>()
    });
    ints(&found.unwrap_or_default())
}
