# std.image

images read, changed and written, PNG, JPEG, GIF and WebP, with the sizes and crops a site makes of them; the runtime decodes, encodes and changes the pixels, on the host's threads. An image never changes: each operation gives a new one.

```tondo
import std.image

let photo = image.open("photo.jpg")?
photo.fit(800, 600).save("photo-800.webp")?
photo.fill(200, 200).save("avatar.jpg", quality: 80)?
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.image` shows the same in a terminal.

- [Functions](#functions): [`read`](#read), [`open`](#open), [`info`](#info), [`exif`](#exif), [`blank`](#blank), [`fromPixels`](#frompixels)
- [Format](#format): [`Format.of`](#formatof)
- [ImageError](#imageerror)
- [Info](#info-1)
- [Color](#color): [`Color.rgb`](#colorrgb), [`Color.hex`](#colorhex)
- [Filter](#filter)
- [Anchor](#anchor)
- [Image](#image): [`Image.width`](#imagewidth), [`Image.height`](#imageheight), [`Image.format`](#imageformat), [`Image.resize`](#imageresize), [`Image.fit`](#imagefit), [`Image.fill`](#imagefill), [`Image.crop`](#imagecrop), [`Image.rotate`](#imagerotate), [`Image.flipHorizontal`](#imagefliphorizontal), [`Image.flipVertical`](#imageflipvertical), [`Image.grayscale`](#imagegrayscale), [`Image.blur`](#imageblur), [`Image.overlay`](#imageoverlay), [`Image.pixels`](#imagepixels), [`Image.adjust`](#imageadjust), [`Image.invert`](#imageinvert), [`Image.sepia`](#imagesepia), [`Image.sharpen`](#imagesharpen), [`Image.pixelate`](#imagepixelate), [`Image.opacity`](#imageopacity), [`Image.colors`](#imagecolors), [`Image.pixel`](#imagepixel), [`Image.encode`](#imageencode), [`Image.save`](#imagesave)
- [Exif](#exif-1)

### Functions

#### read

```tondo
pub fn read(bytes: Array[Byte]): Image ! ImageError
```

The image of the bytes of a PNG, JPEG, GIF or WebP, turned as its EXIF
orientation says; the first frame of an animation.

#### open

```tondo
pub fn open(path: String): Image ! ImageError
```

The image in the file at `path`.

#### info

```tondo
pub fn info(bytes: Array[Byte]): Info ! ImageError
```

The format and size of an image, from its header alone.

#### exif

```tondo
pub fn exif(bytes: Array[Byte]): Exif?
```

The EXIF of a JPEG, PNG, WebP, TIFF or HEIF image, as Hugo's `.Exif`;
`none` when it has none.

#### blank

```tondo
pub fn blank(width: Int, height: Int, color: Color): Image
```

An image of one color.

#### fromPixels

```tondo
pub fn fromPixels(width: Int, height: Int, pixels: Array[Byte]): Image ! ImageError
```

The image of `pixels`: four bytes each, red, green, blue and alpha, row
by row from the top left, as `Image.pixels` gives them.

### Format

```tondo
pub enum Format {
    Png
    Jpeg
    Gif
    WebP
}
```

A format of image.

```tondo
import std.console
import std.image

let photo = image.blank(2, 2, image.Color.rgb(200, 100, 50))
for path in ["a.png", "b.webp", "c.txt"] {
    match image.Format.of(path) {
        some(format) => console.println("{path}: {photo.encode(format)?.length()} bytes")
        none => console.println("{path}: not an image format")
    }
}
```

Implements `Display`.

#### Format.of

```tondo
pub fn Format.of(path: String): Format?
```

The format a path's extension names: `.png`, `.jpg` or `.jpeg`, `.gif`,
`.webp`.

### ImageError

```tondo
pub type ImageError = {
    message: String
}
```

Why an image could not be read or written.

```tondo
import std.console
import std.image

match image.read([1, 2, 3]) {
    ok(picture) => console.println("{picture.width()}x{picture.height()}")
    err(error) => console.println("not an image: {error.message}")
}
```

Implements `Display`.

### Info

```tondo
pub type Info = {
    format: Format
    width: Int
    height: Int
}
```

What the header of an image says, read without its pixels.

```tondo
import std.console
import std.image

let bytes = image.blank(640, 480, image.Color.rgb(0, 0, 0)).encode(image.Format.Png)?
let info = image.info(bytes)?
console.println("{info.width}x{info.height}, png: {info.format == image.Format.Png}")
```

### Color

```tondo
pub type Color = {
    red: Int
    green: Int
    blue: Int
    alpha: Int
}
```

A color: red, green, blue and alpha, each from 0 to 255.

```tondo
import std.console
import std.image

let orange = image.Color.hex("#ff8800")?
let canvas = image.blank(8, 8, orange).overlay(image.blank(4, 4, image.Color.rgb(0, 0, 255)), 0, 0)
for color in canvas.colors(count: 2) {
    console.println("{color.red} {color.green} {color.blue} {color.alpha}")
}
```

Implements `Display`.

#### Color.rgb

```tondo
pub fn Color.rgb(red: Int, green: Int, blue: Int): Color
```

An opaque color.

#### Color.hex

```tondo
pub fn Color.hex(text: String): Color ! ImageError
```

The color of `#rrggbb` or `#rrggbbaa`, the `#` optional.

### Filter

```tondo
pub enum Filter {
    Nearest
    Linear
    CatmullRom
    Lanczos
}
```

How a resize samples the pixels: `Lanczos`, the sharpest, is the
default; `Nearest` keeps hard edges, as pixel art wants.

```tondo
import std.image

// Pixel art keeps its hard edges with `Nearest`; photos look best with `Lanczos`.
let sprite = image.open("sprite.png")?
sprite.resize(width: some(sprite.width() * 4), filter: image.Filter.Nearest).save("sprite-4x.png")?
```

### Anchor

```tondo
pub enum Anchor {
    Center
    Top
    Bottom
    Left
    Right
    TopLeft
    TopRight
    BottomLeft
    BottomRight
}
```

The part of an image a crop to another shape keeps.

```tondo
import std.image

// A square avatar that keeps the top of a portrait, where the face is.
let portrait = image.open("portrait.jpg")?
portrait.fill(256, 256, anchor: image.Anchor.Top).save("avatar.webp", quality: 80)?
```

### Image

```tondo
pub type Image = {
    // private fields
}
```

An image, whose pixels never change.

```tondo
import std.image

let photo = image.open("photo.jpg")?
photo.fit(1200, 1200).sharpen().save("large.jpg", quality: 85)?
photo.fill(300, 200).grayscale().save("thumbnail.webp")?
photo.crop(0, 0, photo.width() / 2, photo.height()).flipHorizontal().save("half.png")?
```

#### Image.width

```tondo
pub fn Image.width(self): Int
```

Its width in pixels.

#### Image.height

```tondo
pub fn Image.height(self): Int
```

Its height in pixels.

#### Image.format

```tondo
pub fn Image.format(self): Format?
```

The format it was read from; `none` for one this program made.

#### Image.resize

```tondo
pub fn Image.resize(
    self,
    width: Int? = none,
    height: Int? = none,
    filter: Filter = Filter.Lanczos,
): Image
```

The image at another size: given one side, the other keeps the
proportions; given both, it is stretched to them.

#### Image.fit

```tondo
pub fn Image.fit(self, width: Int, height: Int, filter: Filter = Filter.Lanczos): Image
```

The image made smaller, if it must be, to fit inside `width` by
`height` with its proportions: a thumbnail. It never grows.

#### Image.fill

```tondo
pub fn Image.fill(
    self,
    width: Int,
    height: Int,
    anchor: Anchor = Anchor.Center,
    filter: Filter = Filter.Lanczos,
): Image
```

The image resized to cover `width` by `height` and cropped to it,
keeping the part `anchor` says: a cover or an avatar.

#### Image.crop

```tondo
pub fn Image.crop(self, x: Int, y: Int, width: Int, height: Int): Image
```

The part `width` by `height` at (`x`, `y`), from the top left, cut to
what the image has.

#### Image.rotate

```tondo
pub fn Image.rotate(self, degrees: Int): Image
```

The image turned clockwise by a multiple of 90 degrees, as `-90` or
`270`.

#### Image.flipHorizontal

```tondo
pub fn Image.flipHorizontal(self): Image
```

The image mirrored left to right.

#### Image.flipVertical

```tondo
pub fn Image.flipVertical(self): Image
```

The image mirrored top to bottom.

#### Image.grayscale

```tondo
pub fn Image.grayscale(self): Image
```

The image in shades of gray.

#### Image.blur

```tondo
pub fn Image.blur(self, sigma: Float): Image
```

The image blurred, by `sigma` pixels.

#### Image.overlay

```tondo
pub fn Image.overlay(self, top: Image, x: Int, y: Int): Image
```

`top` drawn over the image at (`x`, `y`), blended by its alpha: a
watermark or a badge.

#### Image.pixels

```tondo
pub fn Image.pixels(self): Array[Byte]
```

Its pixels: four bytes each, red, green, blue and alpha, row by row
from the top left, to change in place and make an image again with
`image.fromPixels`.

#### Image.adjust

```tondo
pub fn Image.adjust(
    self,
    brightness: Float = 0.0,
    contrast: Float = 0.0,
    saturation: Float = 0.0,
    gamma: Float = 1.0,
    hue: Float = 0.0,
): Image
```

The image with its colors adjusted in the scales of Hugo's filters:
`brightness` and `contrast` from -100 to 100, `saturation` from -100 to
500, `gamma` over 0, where 1 changes nothing and more lightens, and `hue`
turned by degrees from -180 to 180; they apply in that order, and what
is not given does not change.

#### Image.invert

```tondo
pub fn Image.invert(self): Image
```

The image in its opposite colors, as a negative.

#### Image.sepia

```tondo
pub fn Image.sepia(self, amount: Float = 100.0): Image
```

The image toned in sepia, by `amount` percent.

#### Image.sharpen

```tondo
pub fn Image.sharpen(self, sigma: Float = 1.0, amount: Float = 1.0): Image
```

The image sharpened: its difference from a blur of `sigma` pixels, as
`blur` takes, added `amount` times.

#### Image.pixelate

```tondo
pub fn Image.pixelate(self, size: Int): Image
```

The image in squares of `size` pixels, each of their average color.

#### Image.opacity

```tondo
pub fn Image.opacity(self, alpha: Float): Image
```

The image with its alpha multiplied by `alpha`, from 0, transparent, to
1, as it is.

#### Image.colors

```tondo
pub fn Image.colors(self, count: Int = 3): Array[Color]
```

Up to `count` colors that cover most of the image, the most first, each
far enough from the others to tell apart: for a background or a
placeholder while it loads.

#### Image.pixel

```tondo
pub fn Image.pixel(self, x: Int, y: Int): Color?
```

The color of the pixel at (`x`, `y`), from the top left; `none`
outside the image.

#### Image.encode

```tondo
pub fn Image.encode(self, format: Format, quality: Int = 85): Array[Byte] ! ImageError
```

The bytes of the image in a format; JPEG and WebP at a `quality` from 1
to 100, WebP with loss below 100 and without at 100.

#### Image.save

```tondo
pub fn Image.save(self, path: String, quality: Int = 85): !ImageError
```

Writes the image to `path`, in the format its extension names.

### Exif

```tondo
pub type Exif = {
    date: time.DateTime?
    latitude: Float?
    longitude: Float?
    camera: String?
    orientation: Int
    tags: Map[String, String]
}
```

What a photo's EXIF says: when it was taken, in UTC, from its clock at
the offset the photo gives, or as UTC without one; where, in degrees,
south and west below zero; its camera; how it is turned, 1 upright to
8 as EXIF numbers it, which `read` applies; and every tag by its name,
as text: `tags["FNumber"]` is `f/2.8`.

```tondo
import std.console
import std.fs
import std.image

let bytes = fs.readBytes("photo.jpg")?
match image.exif(bytes) {
    some(exif) => {
        console.println("taken {exif.date} with {exif.camera.unwrapOr("an unknown camera")}")
        match (exif.latitude, exif.longitude) {
            (some(latitude), some(longitude)) => console.println("at {latitude}, {longitude}")
            _ => console.println("no place")
        }
    }
    none => console.println("no EXIF data")
}
```
