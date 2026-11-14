//! Pushing RGBA frames and updating corner coordinates on `video` and `canvas` sources.

use std::sync::Arc;

use super::{StyleChange, StyleMutationError};
use crate::style::{
    source::{fresh_generation, CanvasSource, MediaFrame, Source, VideoSource},
    Style,
};

impl Style {
    /// Pushes a straight-alpha RGBA8 frame (`width * height * 4` bytes) and optional updated
    /// `[longitude, latitude]` corner coordinates (`[top_left, top_right, bottom_right, bottom_left]`)
    /// onto a `"video"` source.
    pub fn set_video_source_frame(
        &mut self,
        source_name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
        coordinates: Option<[[f64; 2]; 4]>,
    ) -> Result<StyleChange, StyleMutationError> {
        let frame = build_media_frame(source_name, width, height, rgba)?;
        let video = video_source_mut(&mut self.sources, source_name)?;
        video.frame = Some(frame);
        if let Some(coords) = coordinates {
            video.coordinates = coords;
        }
        video.generation = fresh_generation();
        Ok(reloaded_media(self, source_name))
    }

    /// Updates the four `[longitude, latitude]` corners of a `"video"` source without replacing
    /// its current frame, matching GL JS `VideoSource.setCoordinates`.
    pub fn set_video_coordinates(
        &mut self,
        source_name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, StyleMutationError> {
        let video = video_source_mut(&mut self.sources, source_name)?;
        video.coordinates = coordinates;
        video.generation = fresh_generation();
        Ok(reloaded_media(self, source_name))
    }

    /// Pushes a straight-alpha RGBA8 frame (`width * height * 4` bytes) and optional updated
    /// `[longitude, latitude]` corner coordinates (`[top_left, top_right, bottom_right, bottom_left]`)
    /// onto a `"canvas"` source.
    pub fn set_canvas_source_frame(
        &mut self,
        source_name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
        coordinates: Option<[[f64; 2]; 4]>,
    ) -> Result<StyleChange, StyleMutationError> {
        let frame = build_media_frame(source_name, width, height, rgba)?;
        let canvas = canvas_source_mut(&mut self.sources, source_name)?;
        canvas.frame = Some(frame);
        if let Some(coords) = coordinates {
            canvas.coordinates = coords;
        }
        canvas.generation = fresh_generation();
        Ok(reloaded_media(self, source_name))
    }

    /// Updates the four `[longitude, latitude]` corners of a `"canvas"` source without replacing
    /// its current frame, matching GL JS `CanvasSource.setCoordinates`.
    pub fn set_canvas_coordinates(
        &mut self,
        source_name: &str,
        coordinates: [[f64; 2]; 4],
    ) -> Result<StyleChange, StyleMutationError> {
        let canvas = canvas_source_mut(&mut self.sources, source_name)?;
        canvas.coordinates = coordinates;
        canvas.generation = fresh_generation();
        Ok(reloaded_media(self, source_name))
    }
}

fn build_media_frame(
    source_name: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<MediaFrame, StyleMutationError> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(StyleMutationError::InvalidMediaFrame {
            source_name: source_name.to_owned(),
            width,
            height,
            byte_len: rgba.len(),
        });
    }
    Ok(MediaFrame {
        width,
        height,
        rgba: Arc::from(rgba),
    })
}

fn video_source_mut<'a>(
    sources: &'a mut std::collections::HashMap<String, Source>,
    name: &str,
) -> Result<&'a mut VideoSource, StyleMutationError> {
    match sources.get_mut(name) {
        Some(Source::Video(video)) => Ok(video),
        Some(_) => Err(StyleMutationError::NotAVideoSource {
            source_name: name.to_owned(),
        }),
        None => Err(StyleMutationError::UnknownSource {
            source_name: name.to_owned(),
        }),
    }
}

fn canvas_source_mut<'a>(
    sources: &'a mut std::collections::HashMap<String, Source>,
    name: &str,
) -> Result<&'a mut CanvasSource, StyleMutationError> {
    match sources.get_mut(name) {
        Some(Source::Canvas(canvas)) => Ok(canvas),
        Some(_) => Err(StyleMutationError::NotACanvasSource {
            source_name: name.to_owned(),
        }),
        None => Err(StyleMutationError::UnknownSource {
            source_name: name.to_owned(),
        }),
    }
}

fn reloaded_media(style: &Style, source_name: &str) -> StyleChange {
    let layers = style
        .layers
        .iter()
        .filter(|layer| layer.source.as_deref() == Some(source_name))
        .map(|layer| layer.id.clone())
        .collect();
    StyleChange {
        layers,
        reloaded_image_sources: vec![source_name.to_owned()],
        ..StyleChange::default()
    }
}
