use ratatui::style::Color;

// ---------------------------------------------------------------------------
// Palette
//
// ACCENT means exactly one thing on screen: "this is active / selected."
// Everything else borrows a quieter neighbor so accent keeps its punch.
// ---------------------------------------------------------------------------
pub(crate) const BG: Color = Color::Rgb(5, 4, 5);
pub(crate) const SURFACE: Color = Color::Rgb(12, 10, 12);
pub(crate) const ACCENT: Color = Color::Rgb(255, 82, 34); // #FF5222 active / selected
pub(crate) const ACCENT_DIM: Color = Color::Rgb(58, 56, 57);
pub(crate) const SUCCESS: Color = Color::Rgb(122, 158, 112); // ok / provisioned
pub(crate) const WARN: Color = ACCENT; // warnings use accent, never red
pub(crate) const FG: Color = Color::Rgb(233, 234, 234);
pub(crate) const DIM: Color = Color::Rgb(143, 143, 144);
pub(crate) const FAINT: Color = Color::Rgb(58, 56, 57);

pub(crate) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub(crate) const ASCII_SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
pub(crate) const FILL: [&str; 5] = [" ", "░", "▒", "▓", "█"];
