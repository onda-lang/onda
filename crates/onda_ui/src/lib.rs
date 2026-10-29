//! Shared run screen. Hosts supply state and input, consume commands, and own scheduling.
//! The UI never opens audio devices, starts threads, or owns a native event loop.

mod controls;
mod editing;
mod events;
mod fonts;
mod icons;
mod interaction;
mod layout;
mod model;
mod panels;
mod presentation;
mod renderer;
mod scene;
mod scrollbar;
mod surface;
mod theme;

#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
mod ffi;
#[cfg(target_arch = "wasm32")]
mod web;

pub use interaction::{Input, Key};
pub use model::RunUi;
pub use renderer::Renderer;
pub use surface::SurfaceRenderer;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod panel_tests;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod render_tests;

#[cfg(test)]
mod parity_tests;

#[cfg(test)]
mod interaction_tests;
