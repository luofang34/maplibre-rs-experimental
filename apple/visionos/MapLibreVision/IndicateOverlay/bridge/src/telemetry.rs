use indicate_instrument_state::{
    AirData, AircraftState, AltitudeClass, AltitudeDeclaration, Attitude, EstimateQuality,
    FreshnessPolicy, GeoidModelId, HeadingReference, HeadingSample, Kinematics, PanelData, Quat,
    Stamped, ValidFlags, resolve,
};

/// One replay sample. Flagged fields use SI units and true-north angles.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ReplayTelemetry {
    /// Presence: position 1, vertical speed 2, IAS 4, attitude 8, heading 16, horizontal velocity 32.
    pub present: u32,
    /// Age of this immutable sample relative to its associated terrain frame, milliseconds.
    pub age_ms: f32,
    /// Source ground speed, metres per second; never substituted for IAS.
    pub ground_speed: f32,
    /// True ground track, radians.
    pub track: f32,
    /// Source altitude, metres above source-declared mean sea level.
    pub altitude_msl: f32,
    /// Upward vertical speed, metres per second.
    pub vertical_speed: f32,
    /// Measured indicated airspeed, metres per second, only when presence bit 4 is set.
    pub ias: f32,
    /// Measured body bank, radians, positive right.
    pub roll: f32,
    /// Measured pitch, radians, positive nose up.
    pub pitch: f32,
    /// Measured true heading, radians; never derived from ground track.
    pub heading: f32,
}

/// Resolves presence and validity through Indicate before any scene is emitted.
pub fn resolve_replay(input: ReplayTelemetry) -> PanelData {
    let stamp = Some(input.age_ms);
    let mut state = AircraftState {
        quality: EstimateQuality::Good,
        valid: ValidFlags {
            position: input.present & 1 != 0,
            velocity_horizontal: input.present & 32 != 0,
            velocity_vertical: input.present & 2 != 0,
            attitude: input.present & 8 != 0,
            heading: input.present & 16 != 0,
            ..ValidFlags::default()
        },
        ..AircraftState::default()
    };
    if input.present & 1 != 0 {
        state.kinematics = Stamped {
            data: Some(Kinematics {
                pos_ned_m: [0.0, 0.0, -input.altitude_msl],
                vel_ned_mps: [
                    input.ground_speed * libm::cosf(input.track),
                    input.ground_speed * libm::sinf(input.track),
                    -input.vertical_speed,
                ],
            }),
            age_ms: stamp,
        };
        state.altitude = AltitudeDeclaration {
            reference_class: AltitudeClass::GeometricMsl,
            sample_m: Some(input.altitude_msl),
            geoid_model: GeoidModelId(1),
            ..AltitudeDeclaration::default()
        };
        state.air = Stamped {
            data: Some(AirData::default()),
            age_ms: stamp,
        };
    }
    if input.present & 4 != 0 {
        state.air = Stamped {
            data: Some(AirData {
                ias_mps: Some(input.ias),
                ..AirData::default()
            }),
            age_ms: stamp,
        };
    }
    if input.present & 8 != 0 {
        state.attitude = Stamped {
            data: Some(Attitude {
                quat: Quat::from_euler(input.roll, input.pitch, input.heading),
                rates_rps: [0.0; 3],
            }),
            age_ms: stamp,
        };
    }
    if input.present & 16 != 0 {
        state.heading = Stamped {
            data: Some(HeadingSample {
                heading_rad: input.heading,
                reference: HeadingReference::True,
            }),
            age_ms: stamp,
        };
    }
    resolve(&state, &FreshnessPolicy::default())
}

#[cfg(test)]
mod tests;
