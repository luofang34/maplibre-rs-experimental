//! Adding, removing and updating a map's sources.

use super::{Map, MapError};
use crate::{
    environment::Environment,
    style::{mutation::StyleChange, source::Source},
    window::{HeadedMapWindow, MapWindowConfig},
};

impl<E: Environment> Map<E>
where
    <<E as Environment>::MapWindowConfig as MapWindowConfig>::MapWindow: HeadedMapWindow,
{
    /// Adds a source.
    pub fn add_source(&mut self, name: &str, source: Source) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.add_source(name, source))
    }

    /// Removes a source no layer draws from.
    pub fn remove_source(&mut self, name: &str) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.remove_source(name))
    }

    /// Points an image source at another picture or other corners, as GL JS
    /// `ImageSource.updateImage`; see [`Style::update_image_source`].
    pub fn update_image_source(
        &mut self,
        name: &str,
        image: crate::style::source::ImageSource,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.update_image_source(name, image))
    }

    /// Stretches an image source over other corners, as GL JS `ImageSource.setCoordinates`;
    /// see [`Style::set_image_coordinates`].
    pub fn set_image_coordinates(
        &mut self,
        name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.set_image_coordinates(name, coordinates))
    }
}
