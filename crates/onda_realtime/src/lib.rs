//! Small, backend-independent realtime execution utilities.

/// Establishes the current thread's audio floating-point policy without allocation.
///
/// Enables x86 FTZ/DAZ or ARM64 FZ, preserving all other control bits. Each call
/// checks the register and writes only when needed. Hosts own when to call this
/// function and whether to restore their previous mode; runtime execution never
/// changes it implicitly. Other architectures leave the mode unchanged.
#[inline]
pub fn configure_current_thread_audio_fp_mode() {
    let current = unsafe { read_fp_control() };
    let desired = current | AUDIO_FP_MASK;
    if desired != current {
        unsafe { write_fp_control(desired) };
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
const AUDIO_FP_MASK: u64 = (1 << 6) | (1 << 15); // DAZ | FTZ
#[cfg(target_arch = "aarch64")]
const AUDIO_FP_MASK: u64 = 1 << 24; // FZ
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
const AUDIO_FP_MASK: u64 = 0;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[inline]
unsafe fn read_fp_control() -> u64 {
    let mut current = 0_u32;
    std::arch::asm!("stmxcsr [{}]", in(reg) &mut current, options(nostack, preserves_flags));
    u64::from(current)
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[inline]
unsafe fn write_fp_control(value: u64) {
    let value = value as u32;
    std::arch::asm!("ldmxcsr [{}]", in(reg) &value, options(nostack, preserves_flags));
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn read_fp_control() -> u64 {
    let current: u64;
    std::arch::asm!("mrs {}, fpcr", out(reg) current, options(nostack, preserves_flags));
    current
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn write_fp_control(value: u64) {
    std::arch::asm!("msr fpcr, {}", in(reg) value, options(nostack, preserves_flags));
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
#[inline]
unsafe fn read_fp_control() -> u64 {
    0
}
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
#[inline]
unsafe fn write_fp_control(_: u64) {}

#[cfg(all(
    test,
    any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")
))]
mod tests {
    use super::*;

    #[test]
    fn explicit_setup_reapplies_policy_and_preserves_other_control_bits() {
        struct Restore(u64);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe { write_fp_control(self.0) }
            }
        }
        let original = Restore(unsafe { read_fp_control() });
        for _ in 0..2 {
            let cleared = original.0 & !AUDIO_FP_MASK;
            unsafe { write_fp_control(cleared) };
            configure_current_thread_audio_fp_mode();
            assert_eq!(unsafe { read_fp_control() }, cleared | AUDIO_FP_MASK);
            configure_current_thread_audio_fp_mode();
            assert_eq!(unsafe { read_fp_control() }, cleared | AUDIO_FP_MASK);
            let tiny = std::hint::black_box(1.0e-20_f32);
            assert_eq!((tiny * tiny).to_bits(), 0);
        }
    }
}
