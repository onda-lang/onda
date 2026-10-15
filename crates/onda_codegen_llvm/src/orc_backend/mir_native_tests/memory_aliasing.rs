use super::*;

#[test]
fn scalar_reads_observe_writes_through_aliased_arguments() {
    let source = r#"
struct Counter:
  value: f32

def observe(read: Counter, write: Counter):
  first = read.value
  second = read.value
  write.value += 1.0
  return first + second + read.value

sample:
  x = Counter(value = 2.0)
  y = Counter(value = 10.0)
  out1 = observe(x, x)
  out2 = observe(x, y)
"#;
    for level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(
            run_native_outputs_with_opt_level(source, 4, level),
            [vec![7.0; 4], vec![9.0; 4]]
        );
    }
}
