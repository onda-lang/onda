use std::io::{self, BufRead};

fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.expect("read FMA oracle request");
        let mut fields = line.split_ascii_whitespace();
        match fields.next().expect("missing FMA scalar type") {
            "f32" => {
                let a = parse_u32(fields.next());
                let b = parse_u32(fields.next());
                let c = parse_u32(fields.next());
                assert!(fields.next().is_none(), "too many f32 FMA operands");
                println!(
                    "{:08x}",
                    f32::from_bits(a)
                        .mul_add(f32::from_bits(b), f32::from_bits(c))
                        .to_bits()
                );
            }
            "f64" => {
                let a = parse_u64(fields.next());
                let b = parse_u64(fields.next());
                let c = parse_u64(fields.next());
                assert!(fields.next().is_none(), "too many f64 FMA operands");
                println!(
                    "{:016x}",
                    f64::from_bits(a)
                        .mul_add(f64::from_bits(b), f64::from_bits(c))
                        .to_bits()
                );
            }
            scalar => panic!("unsupported FMA scalar type '{}'", scalar),
        }
    }
}

fn parse_u32(value: Option<&str>) -> u32 {
    u32::from_str_radix(value.expect("missing f32 FMA operand"), 16)
        .expect("invalid f32 FMA operand")
}

fn parse_u64(value: Option<&str>) -> u64 {
    u64::from_str_radix(value.expect("missing f64 FMA operand"), 16)
        .expect("invalid f64 FMA operand")
}
