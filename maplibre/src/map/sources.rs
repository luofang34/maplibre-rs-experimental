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

    /// Points an image source at a picture and optionally other corners, as GL JS
    /// `ImageSource.updateImage`; see [`crate::style::Style::update_image_source`].
    pub fn update_image_source(
        &mut self,
        name: &str,
        url: String,
        coordinates: Option<[[f64; 2]; 4]>,
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.update_image_source(name, url, coordinates))
    }

    /// Stretches an image source over other corners, as GL JS `ImageSource.setCoordinates`;
    /// see [`crate::style::Style::set_image_coordinates`].
    pub fn set_image_coordinates(
        &mut self,
        name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, MapError> {
        self.mutate_style(|style| style.set_image_coordinates(name, coordinates))
    }
}
