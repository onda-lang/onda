//! Host-selected surface presentation, independent of frame scheduling.

/// Surface presentation policy, shared by Rust, C, and Wasm hosts.
/// Capability and current-mode queries return concrete modes, never fallback policies.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub enum PresentMode {
    /// Prefer relaxed FIFO, otherwise FIFO. Relaxed FIFO can tear.
    AutoVsync = 0,
    /// Prefer immediate, then mailbox, then FIFO. Immediate can tear.
    AutoNoVsync = 1,
    Fifo = 2,
    FifoRelaxed = 3,
    Immediate = 4,
    Mailbox = 5,
    /// Prefer tear-free mailbox, otherwise FIFO. Both are synchronized to the display.
    MailboxOrFifo = 6,
}

impl From<wgpu::PresentMode> for PresentMode {
    fn from(mode: wgpu::PresentMode) -> Self {
        match mode {
            wgpu::PresentMode::AutoVsync => Self::AutoVsync,
            wgpu::PresentMode::AutoNoVsync => Self::AutoNoVsync,
            wgpu::PresentMode::Fifo => Self::Fifo,
            wgpu::PresentMode::FifoRelaxed => Self::FifoRelaxed,
            wgpu::PresentMode::Immediate => Self::Immediate,
            wgpu::PresentMode::Mailbox => Self::Mailbox,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationOptions {
    pub present_mode: PresentMode,
    /// A backend-clamped hint, not a frame-rate limit.
    pub maximum_frame_latency: u32,
}

impl PresentationOptions {
    #[cfg(any(feature = "ffi", target_arch = "wasm32", test))]
    pub(crate) fn from_raw(mode: u32, maximum_frame_latency: u32) -> Result<Self, String> {
        use PresentMode::*;
        let present_mode = match mode {
            0 => AutoVsync,
            1 => AutoNoVsync,
            2 => Fifo,
            3 => FifoRelaxed,
            4 => Immediate,
            5 => Mailbox,
            6 => MailboxOrFifo,
            _ => return Err("Unknown presentation mode".into()),
        };
        Ok(Self {
            present_mode,
            maximum_frame_latency,
        })
    }

    pub(crate) fn resolve(self, supported: &[PresentMode]) -> Result<wgpu::PresentMode, String> {
        use wgpu::PresentMode::*;
        if self.maximum_frame_latency == 0 {
            return Err("Maximum frame latency must be positive".into());
        }
        let choices: &[wgpu::PresentMode] = match self.present_mode {
            PresentMode::AutoVsync => &[FifoRelaxed, Fifo],
            PresentMode::AutoNoVsync => &[Immediate, Mailbox, Fifo],
            PresentMode::MailboxOrFifo => &[Mailbox, Fifo],
            PresentMode::Fifo => &[Fifo],
            PresentMode::FifoRelaxed => &[FifoRelaxed],
            PresentMode::Immediate => &[Immediate],
            PresentMode::Mailbox => &[Mailbox],
        };
        choices
            .iter()
            .find(|mode| supported.contains(&PresentMode::from(**mode)))
            .copied()
            .ok_or_else(|| "Presentation mode is unsupported by this surface".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PresentMode::*;

    #[test]
    fn explicit_modes_are_strict_and_fallback_policies_resolve_to_concrete_modes() {
        for mode in [
            wgpu::PresentMode::Fifo,
            wgpu::PresentMode::FifoRelaxed,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
        ] {
            let options = PresentationOptions {
                present_mode: mode.into(),
                maximum_frame_latency: 1,
            };
            assert_eq!(options.resolve(&[mode.into()]).unwrap(), mode);
            assert_eq!(
                options.resolve(&[Fifo]).is_ok(),
                mode == wgpu::PresentMode::Fifo
            );
        }
        for mode in [AutoVsync, AutoNoVsync, MailboxOrFifo] {
            let options = PresentationOptions {
                present_mode: mode,
                maximum_frame_latency: 2,
            };
            assert_eq!(options.resolve(&[Fifo]).unwrap(), wgpu::PresentMode::Fifo);
        }
        assert_eq!(
            PresentationOptions::from_raw(1, 1)
                .unwrap()
                .resolve(&[Fifo, Mailbox, Immediate])
                .unwrap(),
            wgpu::PresentMode::Immediate
        );
        let mailbox = PresentationOptions::from_raw(6, 2).unwrap();
        assert_eq!(
            mailbox
                .resolve(&[Fifo, FifoRelaxed, Mailbox, Immediate])
                .unwrap(),
            wgpu::PresentMode::Mailbox
        );
        assert!(mailbox.resolve(&[FifoRelaxed, Immediate]).is_err());
        assert!(PresentationOptions::from_raw(2, 0)
            .unwrap()
            .resolve(&[Fifo])
            .is_err());
        assert!(PresentationOptions::from_raw(7, 1).is_err());
    }
}
