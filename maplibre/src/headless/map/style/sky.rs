//! The sun, its light, the atmosphere and the stars a host sets on the offscreen map in place of the
//! style's `light` and `sky`, as [`crate::map::Map`] sets its own.

use crate::{
    celestial::{EcefDirection, TwilightBand},
    headless::map::HeadlessMap,
};

impl HeadlessMap {
    /// Sets the direction towards the sun in ECEF axes, as
    /// [`crate::context::MapContext::set_sun_direction`] does; `None` returns to the style's light.
    pub fn set_sun_direction(&mut self, sun: Option<EcefDirection>) {
        self.map_context.set_sun_direction(sun);
    }

    /// Sets the solar elevations over which the terrain turns from night to day, as
    /// [`crate::context::MapContext::set_twilight_band`] does.
    pub fn set_twilight_band(&mut self, twilight: TwilightBand) {
        self.map_context.set_twilight_band(twilight);
    }

    /// Replaces the stars drawn behind the globe, as
    /// [`crate::context::MapContext::set_stars`] does.
    pub fn set_stars(&mut self, stars: Vec<crate::celestial::Star>) {
        self.map_context.set_stars(stars);
    }

    /// Sets or clears the sun's angular radius, as
    /// [`crate::context::MapContext::set_sun_angular_radius`] does.
    pub fn set_sun_angular_radius(&mut self, radius: Option<crate::celestial::AngularRadius>) {
        self.map_context.set_sun_angular_radius(radius);
    }

    /// Sets or clears the moon's disc, as [`crate::context::MapContext::set_moon_disc`] does.
    pub fn set_moon_disc(&mut self, moon: Option<crate::celestial::Disc>) {
        self.map_context.set_moon_disc(moon);
    }

    /// Sets or clears the Earth's shadow the moon is lit through, as
    /// [`crate::context::MapContext::set_earth_shadow`] does.
    pub fn set_earth_shadow(&mut self, shadow: Option<crate::celestial::EarthShadow>) {
        self.map_context.set_earth_shadow(shadow);
    }

    /// Draws the globe's atmosphere without a style `sky`, as
    /// [`crate::context::MapContext::set_atmosphere`] does.
    pub fn set_atmosphere(&mut self, atmosphere: bool) {
        self.map_context.set_atmosphere(atmosphere);
    }
}

#[cfg(test)]
mod bright_moon;
#[cfg(test)]
mod daylight;
#[cfg(test)]
mod eye_ground;
#[cfg(test)]
mod host_atmosphere;
#[cfg(test)]
mod look_up;
#[cfg(test)]
mod lunar_eclipse;
#[cfg(test)]
mod moon;
#[cfg(test)]
mod starfield;
#[cfg(test)]
mod sun;
