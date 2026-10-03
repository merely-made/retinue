//! An `embedded-graphics` draw target that stores browser-ready RGBA bytes.
//!
//! Generic over any color convertible to `Rgb888`. No radio-face types and no
//! wasm dependencies, so this module can move to its own crate unchanged.

use alloc::{vec, vec::Vec};
use core::{convert::Infallible, marker::PhantomData};

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};

/// Row-major RGBA8 pixels, opaque, ready for a canvas `ImageData`.
pub struct RgbaFramebuffer<C> {
    size: Size,
    rgba: Vec<u8>,
    color: PhantomData<C>,
}

impl<C> RgbaFramebuffer<C> {
    /// An opaque black buffer.
    pub fn new(size: Size) -> Self {
        let mut rgba = vec![0; size.width as usize * size.height as usize * 4];
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = u8::MAX;
        }
        Self {
            size,
            rgba,
            color: PhantomData,
        }
    }

    pub const fn width(&self) -> u32 {
        self.size.width
    }

    pub const fn height(&self) -> u32 {
        self.size.height
    }

    pub fn as_rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }

    /// RGB8 without alpha, for encoders that want opaque rows.
    pub fn to_rgb(&self) -> Vec<u8> {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect()
    }

    pub fn pixel(&self, point: Point) -> Option<Rgb888> {
        let index = self.index(point)?;
        Some(Rgb888::new(
            self.rgba[index],
            self.rgba[index + 1],
            self.rgba[index + 2],
        ))
    }

    fn index(&self, point: Point) -> Option<usize> {
        let x = usize::try_from(point.x).ok()?;
        let y = usize::try_from(point.y).ok()?;
        let width = self.size.width as usize;
        (x < width && y < self.size.height as usize).then(|| (y * width + x) * 4)
    }

    fn put(&mut self, index: usize, color: Rgb888) {
        self.rgba[index] = color.r();
        self.rgba[index + 1] = color.g();
        self.rgba[index + 2] = color.b();
    }

    /// Encodes the buffer as an 8-bit RGB PNG.
    #[cfg(feature = "png")]
    pub fn write_png<W: std::io::Write>(&self, output: W) -> Result<(), png::EncodingError> {
        let mut encoder = png::Encoder::new(output, self.size.width, self.size.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.to_rgb())
    }
}

impl<C> OriginDimensions for RgbaFramebuffer<C> {
    fn size(&self) -> Size {
        self.size
    }
}

impl<C> DrawTarget for RgbaFramebuffer<C>
where
    C: PixelColor + Into<Rgb888>,
{
    type Color = C;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if let Some(index) = self.index(point) {
                self.put(index, color.into());
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let color = color.into();
        let area = area.intersection(&self.bounding_box());
        for point in area.points() {
            if let Some(index) = self.index(point) {
                self.put(index, color);
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let color: Rgb888 = color.into();
        for pixel in self.rgba.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[color.r(), color.g(), color.b(), u8::MAX]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::BinaryColor;

    use super::*;

    #[test]
    fn binary_color_maps_through_rgb888() {
        let mut frame = RgbaFramebuffer::<BinaryColor>::new(Size::new(2, 1));
        Pixel(Point::new(1, 0), BinaryColor::On)
            .draw(&mut frame)
            .unwrap();
        assert_eq!(frame.as_rgba(), &[0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn out_of_bounds_pixels_are_dropped() {
        let mut frame = RgbaFramebuffer::<Rgb888>::new(Size::new(2, 2));
        Pixel(Point::new(-1, 0), Rgb888::WHITE)
            .draw(&mut frame)
            .unwrap();
        Pixel(Point::new(2, 1), Rgb888::WHITE)
            .draw(&mut frame)
            .unwrap();
        assert!(frame.as_rgba().chunks(4).all(|p| p == [0, 0, 0, 255]));
    }
}
