//! Style, tile storage, view and renderer state for a map frame.

#![deny(missing_docs)]

use crate::{
    render::{view_state::ViewState, Renderer},
    style::Style,
    tcs::world::World,
    window::PhysicalSize,
};

mod flight;
mod ground;

pub(crate) use flight::camera_flight_system;
pub use flight::{FlightId, FlightOutcome};

/// Mutable frame state shared by scheduled systems, independent of host service types.
pub struct MapContext {
    /// Style evaluated by rendering and tile request systems.
    pub style: Style,
    /// Loaded tile components and shared system resources.
    pub world: World,
    /// Camera, viewport and projection inputs for the current frame.
    pub view_state: ViewState,
    /// GPU state and render graph used to draw this map.
    pub renderer: Renderer,
}

impl MapContext {
    /// The time of the frame being rendered, as the host wrote it; zero until it does.
    pub fn frame_time(&self) -> std::time::Duration {
        crate::render::frame_input::frame_time(&self.world)
    }

    /// The credits of the sources drawn at the camera's zoom, for a host's credits panel; see
    /// [`Style::attributions`].
    pub fn attributions(&self) -> Vec<String> {
        self.style.attributions(self.view_state.zoom().value())
    }

    /// Updates the logical viewport and resizes the physical presentation surface.
    /// `scale_factor` is the number of physical pixels per logical unit.
    /// Logical dimensions remain at least one unit even below the device scale.
    pub fn resize(&mut self, size: PhysicalSize, scale_factor: f64) {
        self.view_state.resize(size.to_logical(scale_factor));
        crate::sdf::provided::set_pixel_ratio(&mut self.world, scale_factor);
        self.renderer.resize_surface(size);
        // A new surface holds nothing, even when the logical size and so the camera are unchanged.
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Sets the direction towards the sun in ECEF axes, which drives the atmosphere in place
    /// of the style's root `light` and darkens the terrain's night side; `None` returns the
    /// atmosphere to the style's light and lights all terrain as day.
    pub fn set_sun_direction(&mut self, sun: Option<crate::celestial::EcefDirection>) {
        if crate::celestial::sun_direction(&self.world) != sun {
            crate::celestial::set_sun_direction(&mut self.world, sun);
            crate::render::frame_signals::mark_dirty(&mut self.world);
        }
    }

    /// Sets the solar elevations over which the ground turns from night to day while a sun
    /// direction is set.
    pub fn set_twilight_band(&mut self, twilight: crate::celestial::TwilightBand) {
        if crate::celestial::lighting(&self.world).twilight != twilight {
            crate::celestial::set_twilight_band(&mut self.world, twilight);
            crate::render::frame_signals::mark_dirty(&mut self.world);
        }
    }

    /// Draws the globe's atmosphere in full, lit by the sun direction while one is set, even
    /// when the style has no `sky`; `false` leaves the atmosphere to the style's `sky`.
    pub fn set_atmosphere(&mut self, atmosphere: bool) {
        if crate::celestial::lighting(&self.world).atmosphere != atmosphere {
            crate::celestial::set_atmosphere(&mut self.world, atmosphere);
            crate::render::frame_signals::mark_dirty(&mut self.world);
        }
    }

    /// Replaces the stars drawn behind a globe; the starfield plugin draws them.
    pub fn set_stars(&mut self, stars: Vec<crate::celestial::Star>) {
        crate::celestial::set_stars(&mut self.world, stars);
        crate::render::frame_signals::mark_dirty(&mut self.world);
    }

    /// Sets or clears the sun's angular radius. The disc stands where
    /// [`Self::set_sun_direction`] points, with a glare that follows how much of it shows; with no
    /// sun direction it is not drawn. The starfield plugin draws it.
    pub fn set_sun_angular_radius(&mut self, radius: Option<crate::celestial::AngularRadius>) {
        if crate::celestial::discs(&self.world).sun_radius != radius {
            crate::celestial::set_sun_angular_radius(&mut self.world, radius);
            crate::render::frame_signals::mark_dirty(&mut self.world);
        }
    }

    /// Sets or clears the moon's disc. Its direction is the one seen from the camera's eye: the
    /// moon is near enough that the Earth's centre sees it elsewhere. Its phase follows
    /// [`Self::set_sun_direction`]; with no sun set, the whole disc is drawn lit, a fallback
    /// rather than a phase. The starfield plugin draws it.
    pub fn set_moon_disc(&mut self, moon: Option<crate::celestial::Disc>) {
        if crate::celestial::discs(&self.world).moon != moon {
            crate::celestial::set_moon_disc(&mut self.world, moon);
            crate::render::frame_signals::mark_dirty(&mut self.world);
        }
    }

    /// Sets the share of the sun's disc the moon leaves uncovered where the eye stands, 1
    /// outside an eclipse. The sky's scattered sunlight and the day side's light scale with
    /// it, so totality darkens the sky and the ground; the sun's disc and glare do not.
    pub fn set_sun_visible_fraction(&mut self, fraction: crate::celestial::VisibleFraction) {
        if crate::celestial::lighting(&self.world).sun_visible != fraction {
            crate::celestial::set_sun_visible_fraction(&mut self.world, fraction);
            crate::render::frame_signals::mark_dirty(&mut self.world);
        }
    }

    /// The sun direction set with [`Self::set_sun_direction`].
    pub fn sun_direction(&self) -> Option<crate::celestial::EcefDirection> {
        crate::celestial::sun_direction(&self.world)
    }

    /// Switches how the camera navigates the globe, for the style's projection: free-globe
    /// navigation needs the pure `vertical-perspective` projection. A center held by
    /// terrain stands on the loaded ground under it at once, as
    /// [`crate::terrain::elevation::ground_center`] stands it.
    pub fn set_navigation_mode(
        &mut self,
        mode: crate::render::view_state::NavigationMode,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        let projection = self
            .style
            .projection
            .as_ref()
            .map_or_else(Default::default, |projection| {
                projection.projection_type.clone()
            });
        self.view_state.set_navigation_mode(mode, &projection)?;
        crate::terrain::elevation::ground_center(&mut self.view_state, &self.world);
        Ok(())
    }

    /// Moves the map at once, as GL JS `jumpTo`, in either navigation mode, as
    /// [`ViewState::jump_to`] moves it. A center held by terrain stands on the loaded ground
    /// under it at once, or at sea level where none has loaded, never on the ground it left.
    pub fn jump_to(
        &mut self,
        options: crate::render::view_state::CameraOptions,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        self.view_state.jump_to(options)?;
        crate::terrain::elevation::ground_center(&mut self.view_state, &self.world);
        Ok(())
    }

    /// Restores a stored free-globe pose, for the style's projection: it must be the pure
    /// `vertical-perspective` projection. A center held by terrain stands on the loaded
    /// ground under the pose's target at once, rather than at the pose's own target height, as
    /// [`crate::terrain::elevation::ground_center`] stands it.
    pub fn restore_globe_pose(
        &mut self,
        pose: crate::render::view_state::GlobePose,
    ) -> Result<(), crate::render::view_state::NavigationError> {
        let projection = self
            .style
            .projection
            .as_ref()
            .map_or_else(Default::default, |projection| {
                projection.projection_type.clone()
            });
        self.view_state.restore_globe_pose(pose, &projection)?;
        crate::terrain::elevation::ground_center(&mut self.view_state, &self.world);
        Ok(())
    }
}
