//! Small, backend-independent realtime execution utilities.

use std::cell::Cell;

thread_local! {
    static AUDIO_FP_MODE_CONFIGURED: Cell<bool> = const { Cell::new(false) };
}

/// Configures the current thread for predictable realtime floating-point DSP.
///
/// On x86 this enables both flush-to-zero for subnormal results and
/// denormals-are-zero for subnormal inputs. Feedback filters otherwise suffer
/// severe, core-dependent stalls as their state decays through the subnormal
/// range. The mode is thread-local and installed at most once per thread.
/// Other architectures currently require no explicit setup.
#[inline]
pub fn configure_current_thread_audio_fp_mode() {
    AUDIO_FP_MODE_CONFIGURED.with(|configured| {
        if configured.get() {
            return;
        }
        configure_audio_fp_mode();
        configured.set(true);
    });
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[inline]
fn configure_audio_fp_mode() {
    const DENORMALS_ARE_ZERO: u32 = 1 << 6;
    const FLUSH_TO_ZERO: u32 = 1 << 15;

    unsafe {
        let mut current = 0_u32;
        std::arch::asm!(
            "stmxcsr [{}]",
            in(reg) &mut current,
            options(nostack, preserves_flags),
        );
        let desired = current | DENORMALS_ARE_ZERO | FLUSH_TO_ZERO;
        if desired != current {
            std::arch::asm!(
                "ldmxcsr [{}]",
                in(reg) &desired,
                options(nostack, preserves_flags),
            );
        }
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
#[inline]
fn configure_audio_fp_mode() {}

#[cfg(test)]
mod tests {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn configures_x86_ftz_and_daz() {
        super::configure_current_thread_audio_fp_mode();
        let mut current = 0_u32;
        unsafe {
            std::arch::asm!(
                "stmxcsr [{}]",
                in(reg) &mut current,
                options(nostack, preserves_flags),
            );
        }
        assert_ne!(current & (1 << 6), 0);
        assert_ne!(current & (1 << 15), 0);
    }
}
