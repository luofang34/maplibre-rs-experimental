//! Changing an image source's picture or corners, as GL JS `ImageSource.updateImage` and
//! `setCoordinates` do.

use super::{StyleChange, StyleMutationError};
use crate::style::{
    source::{ImageSource, Source},
    Style,
};

impl Style {
    /// Points an image source at another picture or other corners.
    pub fn update_image_source(
        &mut self,
        name: &str,
        image: ImageSource,
    ) -> Result<StyleChange, StyleMutationError> {
        match self.sources.get_mut(name) {
            Some(Source::Image(current)) => *current = image,
            Some(_) => {
                return Err(StyleMutationError::NotAnImageSource {
                    source_name: name.to_owned(),
                })
            }
            None => {
                return Err(StyleMutationError::UnknownSource {
                    source_name: name.to_owned(),
                })
            }
        }
        Ok(StyleChange {
            reloaded_image_sources: vec![name.to_owned()],
            ..StyleChange::default()
        })
    }

    /// Stretches an image source's picture over other corners: `[longitude, latitude]` of the
    /// top left, top right, bottom right and bottom left.
    pub fn set_image_coordinates(
        &mut self,
        name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, StyleMutationError> {
        let url = match self.sources.get(name) {
            Some(Source::Image(current)) => current.url.clone(),
            Some(_) => {
                return Err(StyleMutationError::NotAnImageSource {
                    source_name: name.to_owned(),
                })
            }
            None => {
                return Err(StyleMutationError::UnknownSource {
                    source_name: name.to_owned(),
                })
            }
        };
        self.update_image_source(name, ImageSource { url, coordinates })
    }
}
