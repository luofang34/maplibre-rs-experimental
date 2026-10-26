//! The sun and its light a host sets on the offscreen map in place of the style's `light`, as
//! [`crate::map::Map`] sets its own.

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
}

#[cfg(test)]
mod daylight;
