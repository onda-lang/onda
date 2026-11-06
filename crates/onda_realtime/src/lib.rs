//! Small, backend-independent realtime execution utilities.

/// Configures the current thread for predictable realtime floating-point DSP.
///
/// On x86 this enables both flush-to-zero for subnormal results and
/// denormals-are-zero for subnormal inputs. Feedback filters otherwise suffer
/// severe, core-dependent stalls as their state decays through the subnormal
/// range. Every call establishes the mode on the current thread and leaves it installed.
/// Hosts own when to call this function and whether to restore their previous mode.
/// Other architectures currently require no explicit setup.
#[inline]
pub fn configure_current_thread_audio_fp_mode() {
    configure_audio_fp_mode();
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
    fn explicit_setup_reapplies_policy_and_preserves_other_control_bits() {
        unsafe fn read() -> u32 {
            let mut value = 0;
            std::arch::asm!("stmxcsr [{}]", in(reg) &mut value, options(nostack, preserves_flags));
            value
        }
        unsafe fn write(value: u32) {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &value, options(nostack, preserves_flags));
        }
        struct Restore(u32);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { write(self.0) }
            }
        }
        let original = Restore(unsafe { read() });
        let policy = (1 << 6) | (1 << 15);
        for _ in 0..2 {
            let cleared = original.0 & !policy;
            unsafe {
                write(cleared);
            }
            super::configure_current_thread_audio_fp_mode();
            assert_eq!(unsafe { read() }, cleared | policy);
        }
    }
}
