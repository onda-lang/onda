include!("execution_and_runtime/part_1.rs");
include!("execution_and_runtime/part_2.rs");
include!("execution_and_runtime/part_3.rs");

#[path = "execution_and_runtime/oscillator_timing.rs"]
mod oscillator_timing;

#[path = "execution_and_runtime/state_promotion.rs"]
mod state_promotion;

#[path = "execution_and_runtime/oversampling_scratch.rs"]
mod oversampling_scratch;
