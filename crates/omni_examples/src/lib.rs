pub const GAIN: &str = r#"
ins {
  in1
}
outs {
  out1
}
params {
  gain = 1.0
}
sample {
  out1 = in1 * gain
}
"#;

pub const SINE: &str = r#"
outs {
  out1
}
params {
  freq = 440.0
}
init {
  phase = 0.0
}
sample {
  phase = phase + freq * TWO_PI / SR
  out1 = sin(phase)
}
"#;

pub const ONE_POLE: &str = r#"
ins {
  in1
}
outs {
  out1
}
params {
  a = 0.1
}
init {
  z = 0.0
}
sample {
  z = z + a * (in1 - z)
  out1 = z
}
"#;
