//! Changing an image source's picture or corners, as GL JS `ImageSource.updateImage` and
//! `setCoordinates` do.

use super::{StyleChange, StyleMutationError};
use crate::style::{
    source::{fresh_generation, ImageSource, Source},
    Style,
};

impl Style {
    /// Points an image source at a picture, fetched again even from the same URL, and
    /// optionally at other corners, as GL JS `ImageSource.updateImage`.
    pub fn update_image_source(
        &mut self,
        name: &str,
        url: String,
        coordinates: Option<[[f64; 2]; 4]>,
    ) -> Result<StyleChange, StyleMutationError> {
        let image = image_source_mut(&mut self.sources, name)?;
        image.url = url;
        image.generation = fresh_generation();
        if let Some(coordinates) = coordinates {
            image.coordinates = coordinates;
        }
        Ok(reloaded(name))
    }

    /// Stretches an image source's picture over other corners, `[longitude, latitude]` of the
    /// top left, top right, bottom right and bottom left, as GL JS `ImageSource.setCoordinates`;
    /// the picture is not fetched again.
    pub fn set_image_coordinates(
        &mut self,
        name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, StyleMutationError> {
        image_source_mut(&mut self.sources, name)?.coordinates = coordinates;
        Ok(reloaded(name))
    }
}

fn image_source_mut<'a>(
    sources: &'a mut std::collections::HashMap<String, Source>,
    name: &str,
) -> Result<&'a mut ImageSource, StyleMutationError> {
    match sources.get_mut(name) {
        Some(Source::Image(image)) => Ok(image),
        Some(_) => Err(StyleMutationError::NotAnImageSource {
            source_name: name.to_owned(),
        }),
        None => Err(StyleMutationError::UnknownSource {
            source_name: name.to_owned(),
        }),
    }
}

fn reloaded(name: &str) -> StyleChange {
    StyleChange {
        reloaded_image_sources: vec![name.to_owned()],
        ..StyleChange::default()
    }
}
