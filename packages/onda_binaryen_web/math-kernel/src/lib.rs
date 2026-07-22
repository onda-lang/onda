#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

macro_rules! export_unary {
    ($export:ident, $input:ty, $implementation:path) => {
        #[no_mangle]
        pub extern "C" fn $export(value: $input) -> $input {
            $implementation(value)
        }
    };
}

macro_rules! export_binary {
    ($export:ident, $input:ty, $implementation:path) => {
        #[no_mangle]
        pub extern "C" fn $export(lhs: $input, rhs: $input) -> $input {
            $implementation(lhs, rhs)
        }
    };
}

macro_rules! export_ternary {
    ($export:ident, $input:ty, $implementation:path) => {
        #[no_mangle]
        pub extern "C" fn $export(a: $input, b: $input, c: $input) -> $input {
            $implementation(a, b, c)
        }
    };
}

export_unary!(onda_math_sin_f32, f32, libm::sinf);
export_unary!(onda_math_sin_f64, f64, libm::sin);
export_unary!(onda_math_cos_f32, f32, libm::cosf);
export_unary!(onda_math_cos_f64, f64, libm::cos);
export_unary!(onda_math_tan_f32, f32, libm::tanf);
export_unary!(onda_math_tan_f64, f64, libm::tan);
export_unary!(onda_math_tanh_f32, f32, libm::tanhf);
export_unary!(onda_math_tanh_f64, f64, libm::tanh);
export_unary!(onda_math_atan_f32, f32, libm::atanf);
export_unary!(onda_math_atan_f64, f64, libm::atan);
export_binary!(onda_math_atan2_f32, f32, libm::atan2f);
export_binary!(onda_math_atan2_f64, f64, libm::atan2);
export_unary!(onda_math_exp_f32, f32, libm::expf);
export_unary!(onda_math_exp_f64, f64, libm::exp);
export_unary!(onda_math_log_f32, f32, libm::logf);
export_unary!(onda_math_log_f64, f64, libm::log);
export_binary!(onda_math_pow_f32, f32, libm::powf);
export_binary!(onda_math_pow_f64, f64, libm::pow);
export_binary!(onda_math_remainder_f32, f32, libm::fmodf);
export_binary!(onda_math_remainder_f64, f64, libm::fmod);
export_ternary!(onda_math_fma_f32, f32, libm::fmaf);
export_ternary!(onda_math_fma_f64, f64, libm::fma);
