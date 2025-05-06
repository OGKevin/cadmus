use image::{self as image_crate, ColorType, GenericImageView, Pixel};
use std::path::Path;
use crate::{color::Color, framebuffer::{Framebuffer, Pixmap, Samples}, geom::ColorSource};

use super::Document;

pub struct Image {
    image: image_crate::DynamicImage,
    file_name: String
}

impl Document for Image {
    fn dims(&self, _index: usize) -> Option<(f32, f32)> {
        let (width, height) = self.image.dimensions();

        Some((width as f32, height as f32))
    }

    fn pages_count(&self) -> usize {
        1
    }

    fn toc(&mut self) -> Option<Vec<super::TocEntry>> {
        None
    }

    fn chapter<'a>(&mut self, _offset: usize,_tocc: &'a [super::TocEntry]) -> Option<(&'a super::TocEntry, f32)> {
        None
    }

    fn chapter_relative<'a>(&mut self, _offset: usize, _dir: crate::geom::CycleDir, _toc: &'a [super::TocEntry]) -> Option<&'a super::TocEntry> {
        None
    }

    fn words(&mut self, _loc: super::Location) -> Option<(Vec<super::BoundedText>, usize)> {
        None
    }

    fn lines(&mut self, _loc: super::Location) -> Option<(Vec<super::BoundedText>, usize)> {
        None
    }

    fn links(&mut self, _loc: super::Location) -> Option<(Vec<super::BoundedText>, usize)> {
        None
    }

    fn images(&mut self, _loc: super::Location) -> Option<(Vec<crate::geom::Boundary>, usize)> {
        None
    }

    fn pixmap(&mut self, loc: super::Location, scale: f32, samples: Samples) -> Option<(crate::framebuffer::Pixmap, usize)> {
        let _ = loc;

        let width: u32 = (self.image.width() as f32 * scale) as u32;
        let height: u32 = (self.image.height() as f32 * scale) as u32;

        let scaled_image = self.image.resize(width, height, image::imageops::FilterType::Lanczos3);
        let mut pixmap = Pixmap::new(scaled_image.width(), scaled_image.height(), samples);

        // FIXME(ogkevin): this is slow af :sob:
        for pixel in scaled_image.pixels() {
            let (x, y, pixel) = pixel;
            pixmap.set_pixel(x, y, Color::from_rgba(&pixel.to_rgba().0));
        }

        Some((pixmap, 0))
    }

    fn layout(&mut self, width: u32, height: u32, font_size: f32, dpi: u16) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn set_font_family(&mut self, _family_name: &str, _search_path: &str) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn set_margin_width(&mut self, _width: i32) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn set_text_align(&mut self, _text_align: crate::metadata::TextAlign) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn set_line_height(&mut self, _line_height: f32) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn set_hyphen_penalty(&mut self, _hyphen_penalty: i32) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn set_stretch_tolerance(&mut self, _stretch_tolerance: f32) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn set_ignore_document_css(&mut self, _ignore: bool) {
        // TODO(ogkevin): do we panic or just nop?
        unimplemented!()
    }

    fn title(&self) -> Option<String> {
        Some(self.file_name.clone())
    }

    fn author(&self) -> Option<String> {
        None
    }

    fn metadata(&self, _key: &str) -> Option<String> {
        None
    }

    fn is_reflowable(&self) -> bool {
        false
    }
}

pub fn open<P: AsRef<Path>>(path: P) -> Option<Image> {
    let path_ref = path.as_ref();
    let file_name = path_ref.to_str().expect("expected path to not be empty");
    let img = image_crate::open(path_ref).expect("Failed to open image");

    return Some(Image { image: img, file_name: file_name.to_string()});
}
