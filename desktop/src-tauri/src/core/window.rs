//! Window chrome and backdrop.
//!
//! The window ships with `decorations: false` and `transparent: true`, so the
//! frontend draws its own title bar and rounded shell. The blurred backdrop
//! behind that shell is a native effect rather than a CSS filter: CSS
//! `backdrop-filter` can only blur what the page itself painted, so it cannot
//! pick up the desktop wallpaper underneath a transparent window.

use tauri::window::{Color, Effect, EffectsBuilder};
use tauri::WebviewWindow;

/// Backdrop tint for the light theme. Warm near-white so white cards still
/// separate from the surface behind them.
const LIGHT_TINT: Color = Color(244, 243, 240, 210);

/// Backdrop tint for the dark theme.
const DARK_TINT: Color = Color(18, 18, 20, 219);

/// Apply the acrylic backdrop for the given theme.
///
/// Acrylic is a Windows compositor effect. On platforms that do not provide it
/// the call fails and the window simply keeps the solid background the
/// frontend paints, which is why the error is intentionally not propagated.
pub fn apply_backdrop(window: &WebviewWindow, dark: bool) {
    let tint = if dark { DARK_TINT } else { LIGHT_TINT };
    let _ = window.set_effects(
        EffectsBuilder::new()
            .effect(Effect::Acrylic)
            .color(tint)
            .build(),
    );
}
