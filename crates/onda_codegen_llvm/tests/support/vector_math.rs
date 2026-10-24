#[path = "../../src/vector_math_spec.rs"]
mod spec;

pub fn operations() -> impl Iterator<Item = &'static str> {
    spec::FUNCTIONS.iter().map(|entry| entry.0)
}

#[derive(Clone, Copy, Debug)]
pub struct Case {
    pub operation: &'static str,
    pub precision: u32,
    pub left: u64,
    pub right: u64,
    kind: &'static str,
    lower: u64,
    upper: u64,
}

impl Case {
    pub fn accepts(self, raw: u64) -> bool {
        if self.kind == "N" {
            return if self.precision == 32 {
                f32::from_bits(raw as u32).is_nan()
            } else {
                f64::from_bits(raw).is_nan()
            };
        }
        let sign = 1 << (self.precision - 1);
        let mask = u64::MAX >> (64 - self.precision);
        let key = if raw & sign != 0 {
            !raw & mask
        } else {
            raw | sign
        };
        (self.lower..=self.upper).contains(&key)
    }

    pub fn safe_in_audio_mode(self) -> bool {
        let sign = 1 << (self.precision - 1);
        let mask = u64::MAX >> (64 - self.precision);
        let raw = |key: u64| {
            if key & sign != 0 {
                key ^ sign
            } else {
                !key & mask
            }
        };
        let normal = |bits| {
            let exponent = if self.precision == 32 {
                0x7f80_0000
            } else {
                0x7ff0_0000_0000_0000
            };
            bits & exponent != 0 && bits & exponent != exponent
        };
        let input = |bits| normal(bits) || bits & !sign == 0;
        self.kind == "R"
            && normal(raw(self.lower))
            && normal(raw(self.upper))
            && input(self.left)
            && (!matches!(self.operation, "atan2" | "pow") || input(self.right))
    }
}

pub fn cases() -> Vec<Case> {
    include_str!("../fixtures/vector_math_reference.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 7, "{line}");
            let hex = |index: usize| u64::from_str_radix(fields[index], 16).unwrap();
            Case {
                operation: fields[0],
                precision: fields[1].parse().unwrap(),
                left: hex(2),
                right: hex(3),
                kind: fields[4],
                lower: hex(5),
                upper: hex(6),
            }
        })
        .collect()
}

pub fn compile(operation: &str, precision: &str) -> onda_mir::OptimizedProgram {
    let expression = match operation {
        "exp2" => "pow(2.0, in1)".to_owned(),
        "atan2" | "pow" => format!("{operation}(in1, in2)"),
        _ => format!("{operation}(in1)"),
    };
    compile_expression(&expression, precision)
}

pub fn compile_expression(expression: &str, precision: &str) -> onda_mir::OptimizedProgram {
    let source = format!(
        "ins:\n  in1: {precision}\n  in2: {precision}\nouts:\n  out1: {precision}\nsample:\n  out1 = {expression}\n"
    );
    let parsed = onda_frontend::parse_program(&source).unwrap();
    let typed = onda_semantics::analyze_with_options(
        parsed,
        onda_semantics::AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 128,
        },
    )
    .unwrap();
    onda_semantics::lower_program_to_optimized_mir(&typed).unwrap()
}
