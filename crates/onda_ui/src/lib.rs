//! Shared run screen. Hosts supply state and input, consume commands, and own scheduling.
//! The UI never opens audio devices, starts threads, or owns a native event loop.
//! Use [`Renderer::from_device`] to share a host's GPU resources. The complete
//! host protocol is documented in `docs/ui-api.md`; `examples/host.rs` demonstrates
//! an independent host with its own render target and command handling.

mod controls;
mod editing;
mod events;
mod fonts;
mod frame_pacer;
mod icons;
mod interaction;
mod layout;
mod meters;
mod model;
mod panels;
mod presentation;
mod presentation_options;
mod renderer;
mod scene;
mod scheduling;
mod scrollbar;
mod surface;
mod theme;
mod value;

#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
mod ffi;
#[cfg(target_arch = "wasm32")]
mod web;

pub use frame_pacer::FramePacer;
pub use interaction::{Input, Key};
pub use model::RunUi;
pub use presentation_options::{PresentMode, PresentationOptions};
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

#[cfg(test)]
mod scope_tests;
