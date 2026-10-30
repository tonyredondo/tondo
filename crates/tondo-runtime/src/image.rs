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
