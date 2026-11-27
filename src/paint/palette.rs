//! The Cable Grid colour tokens (`docs/design/cable-grid.md#palette`).

use super::Rgba;

/// Top of the violet field gradient; the mark's tile.
pub const FIELD_TOP: Rgba = Rgba::hex(0x2e2180);
/// Bottom of the field gradient.
pub const FIELD_BOTTOM: Rgba = Rgba::hex(0x1a1150);
/// Channel cells, headers and menu rows.
pub const CELL: Rgba = Rgba::hex(0x3a2ca6);
/// The focused cell or row, under the yellow outline.
pub const FOCUS: Rgba = Rgba::hex(0x4a3cc4);
/// The top and left edge of a bevel.
pub const BEVEL_LIGHT: Rgba = Rgba::hex(0x8e84ff);
/// The bottom and right edge of a bevel.
pub const BEVEL_DARK: Rgba = Rgba::hex(0x0e0833);
/// Program cells for series.
pub const SERIES: Rgba = Rgba::hex(0x2646c4);
/// Program cells for films.
pub const MOVIE: Rgba = Rgba::hex(0x7c2144);
/// Program cells for music and mixed blocks. Titles on it stay white.
pub const MUSIC: Rgba = Rgba::hex(0x1c6c86);
/// Channel numbers, times, the focus outline and the mark.
pub const YELLOW: Rgba = Rgba::hex(0xffd23f);
/// Top of the ochre heading band.
pub const OCHRE_TOP: Rgba = Rgba::hex(0xf7c64c);
/// Bottom of the ochre heading band; the mark's bottom row.
pub const OCHRE_BOTTOM: Rgba = Rgba::hex(0xde7f1c);
/// Text on ochre.
pub const OCHRE_INK: Rgba = Rgba::hex(0x2a1200);
/// Titles and body text.
pub const WHITE: Rgba = Rgba::hex(0xffffff);
/// The hard text shadow.
pub const SHADOW: Rgba = Rgba::hex(0x05020f);
