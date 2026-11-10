use onda_ui::RunUi;
use std::{hint::black_box, time::Instant};

fn timed<T: Clone>(updates: [T; 2], n: usize, mut apply: impl FnMut(T)) -> f64 {
    for i in 0..100 { apply(updates[i % 2].clone()); }
    let start = Instant::now();
    for i in 0..n { apply(black_box(updates[i % 2].clone())); }
    start.elapsed().as_secs_f64() * 1e6 / n as f64
}
fn main() {
    println!("{{\"metric\":\"microseconds per RunUi::message including input Value clone, no rendering or JSON parsing\",\"rounds\":7,\"rows\":[");
    let mut first = true;
    for controls in [64, 512, 4096] {
        let params = (0..controls).map(|i| format!(r#"{{"name":"p{i}","type":"f64","value":0.5,"default":0.5,"rangeMin":0,"rangeMax":1,"step":0.01,"stepCount":100}}"#)).collect::<Vec<_>>().join(",");
        let full = ["a", "b"].map(|status| format!(r#"{{"type":"state","state":{{"path":"bench.onda","supportsScope":false,"params":[{params}],"status":"{status}"}}}}"#));
        let partial = ["a", "b"].map(|status| format!(r#"{{"type":"state","state":{{"status":"{status}"}}}}"#));
        let values = [0.4, 0.6].map(|v| format!(r#"{{"type":"parameterValues","state":{{"params":[{{"name":"p0","value":{v}}}]}}}}"#));
        let mut ui = RunUi::default();
        ui.message(full[0].parse().unwrap());
        for (name, updates) in [("full_state_for_status", &full), ("partial_status", &partial), ("single_value", &values)] {
            let mut results = vec![];
            for _ in 0..7 {
                results.push(timed([updates[0].parse().unwrap(), updates[1].parse().unwrap()], 500, |value| ui.message(value)));
            }
            results.sort_by(f64::total_cmp);
            if !first { println!(","); } first = false;
            print!("{{\"controls\":{controls},\"publication\":\"{name}\",\"median_us\":{}}}", results[3]);
        }
        black_box(ui.revisions());
    }
    println!("]}}");
}
