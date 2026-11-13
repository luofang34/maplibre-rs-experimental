//! Platform-neutral input event and modifier types for window-system-independent gestures.

/// Press or release state of a pointer button or keyboard key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ButtonState {
    /// The button or key was pressed down.
    Pressed,
    /// The button or key was released.
    Released,
}

/// Logical pointer or mouse button identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    /// Primary pointer button (left mouse button).
    Primary,
    /// Secondary pointer button (right mouse button).
    Secondary,
    /// Auxiliary / middle pointer button (wheel click).
    Middle,
    /// Additional device-specific button index.
    Other(u16),
}

/// Scroll-wheel or two-finger trackpad scroll delta in logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseScrollDelta {
    /// Discrete wheel notch movement measured in lines (`y > 0` scrolls up / zooms in).
    LineDelta {
        /// Horizontal line delta.
        x: f32,
        /// Vertical line delta.
        y: f32,
    },
    /// Continuous or pixel-stepped scroll delta in logical window pixels.
    PixelDelta {
        /// Horizontal logical pixel delta.
        x: f64,
        /// Vertical logical pixel delta (`y > 0` zooms in / scrolls down-content).
        y: f64,
    },
}

/// Lifecycle phase of a touch contact or trackpad multi-touch gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TouchPhase {
    /// Contact or gesture began.
    Started,
    /// Contact moved or gesture updated.
    Moved,
    /// Contact lifted or gesture ended normally.
    Ended,
    /// Contact or gesture was interrupted by the system.
    Cancelled,
}

/// Active keyboard modifier state accompanying pointer, wheel, and key events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModifiersState {
    /// Whether `Shift` is held.
    pub shift: bool,
    /// Whether `Alt` / `Option` is held.
    pub alt: bool,
    /// Whether `Control` / `Command` / `Super` is held.
    pub ctrl: bool,
}

/// Window-system-independent keyboard navigation action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavKey {
    /// Pan north/up (or tilt up when `Alt` is held): `ArrowUp` or `W`.
    Up,
    /// Pan south/down (or tilt down when `Alt` is held): `ArrowDown` or `S`.
    Down,
    /// Pan west/left (or orbit counter-clockwise when `Alt` is held): `ArrowLeft` or `A`.
    Left,
    /// Pan east/right (or orbit clockwise when `Alt` is held): `ArrowRight` or `D`.
    Right,
    /// Continuous counter-clockwise bearing orbit (`Q`).
    OrbitCcw,
    /// Continuous clockwise bearing orbit (`E`).
    OrbitCw,
    /// Smooth zoom in (`I`, `+`, `=`).
    ZoomIn,
    /// Smooth zoom out (`K`, `-`, `_`).
    ZoomOut,
    /// Toggle between `Map` and `Rts` pointer profiles (`Tab`).
    TogglePointerProfile,
    /// Cycle 2D / 3D / Auto-Tilt camera mode (`T`).
    Cycle3dMode,
    /// Reset bearing to north or toggle North-Lock (`N`).
    Compass,
    /// Fly back to the home camera (`R` or `Home`).
    Home,
    /// Show or hide the navigation HUD overlay (`H`).
    ToggleHud,
    /// Expand the debug left viewport inset (`V`).
    DebugInsetLeft,
    /// Expand the debug top viewport inset (`B`).
    DebugInsetTop,
    /// Expand the debug bottom viewport inset.
    DebugInsetBottom,
    /// Expand the debug right viewport inset (`M`).
    DebugInsetRight,
}

/// Window-system-independent input event consumed by [`super::InputController`].
///
/// All pointer, touch, and pixel-scroll coordinates are expressed in **logical** window pixels
/// (physical pixels divided by the display scale factor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    /// Active keyboard modifier flags changed.
    ModifiersChanged(ModifiersState),
    /// Pointer / cursor moved to `position` in logical window pixels.
    PointerMoved {
        /// Logical `(x, y)` coordinates in window pixels.
        position: [f64; 2],
    },
    /// Pointer / mouse button pressed or released.
    PointerButton {
        /// Button that changed state.
        button: PointerButton,
        /// Press or release state.
        state: ButtonState,
    },
    /// Scroll wheel or two-finger trackpad scroll delta.
    MouseWheel {
        /// Scroll delta in lines or logical pixels.
        delta: MouseScrollDelta,
    },
    /// Multi-touch contact event in logical window pixels.
    Touch {
        /// Unique identifier of the touch contact.
        id: u64,
        /// Lifecycle phase of the touch contact.
        phase: TouchPhase,
        /// Logical `(x, y)` coordinates in window pixels.
        position: [f64; 2],
    },
    /// Native trackpad pinch magnification gesture.
    PinchGesture {
        /// Incremental magnification delta (`> 0` spreading apart, `< 0` pinching together).
        delta: f64,
        /// Lifecycle phase of the pinch gesture.
        phase: TouchPhase,
    },
    /// Native trackpad two-finger rotation gesture in degrees.
    RotationGesture {
        /// Incremental rotation angle in degrees.
        delta_deg: f64,
    },
    /// Smart-zoom / double-tap gesture at the current pointer position.
    DoubleTapGesture,
    /// Continuous trackpad pinch magnification step without explicit start/end phases
    /// (used for WebKit `GestureEvent` and browser `ctrlKey + wheel` streams).
    WebPinchWheel {
        /// Incremental magnification delta.
        magnification: f64,
    },
    /// Logical navigation key pressed or released.
    Key {
        /// Logical navigation key.
        key: NavKey,
        /// Press or release state.
        state: ButtonState,
    },
}
