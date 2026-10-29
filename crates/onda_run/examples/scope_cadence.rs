//! Measure fresh scope publications through the real playback child, without a
//! window. Output is attenuated to inaudibility; drawing is outside this probe.
use onda_run::{RunController, RunHostOptions};
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let binary = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/release/onda"))
        .canonicalize()?;
    let path = std::env::temp_dir().join(format!(
        "onda-scope-cadence-{}-{}.onda",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    std::fs::write(&path,"params:\n  freq = 110.0 {20.0, 1000.0}\ninit:\n  phase = 0.0\nsample:\n  out1 = sin(phase) * 0.00000001\n  out2 = sin(phase) * 0.00000001\n  phase = phase + freq * TWO_PI / SR\n  if phase > TWO_PI:\n    phase = phase - TWO_PI\n")?;
    let result = (|| {
        let selected = std::env::var("ONDA_SCOPE_INTERVAL_MS")
            .ok()
            .map(|s| s.parse::<u64>())
            .transpose()
            .map_err(|e| e.to_string())?;
        let seconds = std::env::var("ONDA_SCOPE_SECONDS")
            .ok()
            .map(|s| s.parse::<f64>())
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or(5.0);
        let minimum = std::env::var("ONDA_SCOPE_MIN_HZ")
            .ok()
            .map(|s| s.parse::<f64>())
            .transpose()
            .map_err(|e| e.to_string())?;
        for ms in selected.map(|ms| vec![ms]).unwrap_or_else(|| vec![50, 16]) {
            let interval = Duration::from_millis(ms);
            let mut controller = RunController::new(
                &path,
                RunHostOptions {
                    onda_bin: binary.display().to_string(),
                    ..Default::default()
                },
            )?;
            let (wake_tx, wake_rx) = std::sync::mpsc::sync_channel(1);
            controller.set_wakeup(move || {
                let _ = wake_tx.try_send(());
            });
            controller.set_scope_poll_interval(interval);
            if let Ok(columns) = std::env::var("ONDA_SCOPE_COLUMNS") {
                controller.set_scope_columns(columns.parse::<usize>().map_err(|e| e.to_string())?);
            }
            let start = Instant::now();
            while controller
                .scope_envelope()
                .map(|s| s.columns() == 0)
                .unwrap_or_else(|| controller.state().scope_samples.is_empty())
            {
                controller.poll();
                if let Some(error) = &controller.state().error {
                    return Err(error.clone());
                }
                if start.elapsed() > Duration::from_secs(10) {
                    return Err(format!("No scope data: {}", controller.state().status));
                }
                let _ = wake_rx.recv_timeout(
                    controller
                        .next_poll_deadline()
                        .saturating_duration_since(Instant::now()),
                );
            }
            let start = Instant::now();
            let mut times = vec![];
            let mut polls = 0;
            while start.elapsed() < Duration::from_secs_f64(seconds) {
                polls += 1;
                if controller.poll().scope_changed {
                    times.push(start.elapsed().as_secs_f64());
                }
                let deadline = controller
                    .next_poll_deadline()
                    .min(start + Duration::from_secs_f64(seconds));
                let _ = wake_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
            }
            let mut gaps: Vec<_> = times.windows(2).map(|w| (w[1] - w[0]) * 1000.0).collect();
            gaps.sort_by(f64::total_cmp);
            println!(
                "{}",
                json!({"poll_interval_ms":ms,"duration_seconds":start.elapsed().as_secs_f64(),"fresh_snapshots":times.len(),"controller_polls":polls,"event_driven":true,
                "updates_per_second":times.len() as f64/start.elapsed().as_secs_f64(),"median_gap_ms":gaps.get(gaps.len()/2),"packet_bytes":controller.scope_envelope().map(|s|s.packet().len())})
            );
            let hz = times.len() as f64 / start.elapsed().as_secs_f64();
            controller.set_scope_visible(false);
            let hidden_start = Instant::now();
            let hidden_end = hidden_start + Duration::from_millis(1200);
            let mut hidden_polls = 0;
            while Instant::now() < hidden_end {
                assert!(
                    !controller.poll().scope_changed,
                    "hidden capture reached the viewer"
                );
                hidden_polls += 1;
                let deadline = controller.next_poll_deadline().min(hidden_end);
                let _ = wake_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
            }
            println!(
                "{}",
                json!({"scope_hidden":true,"duration_seconds":hidden_start.elapsed().as_secs_f64(),"controller_polls":hidden_polls})
            );
            if hidden_polls > 4 {
                return Err(format!(
                    "Hidden scope caused {hidden_polls} controller polls"
                ));
            }
            if minimum.is_some_and(|minimum| hz < minimum) {
                return Err(format!(
                    "Scope delivered {hz:.1} Hz; expected at least {:.1} Hz",
                    minimum.unwrap_or(0.0)
                ));
            }
        }
        Ok::<_, String>(())
    })();
    std::fs::remove_file(path)?;
    result.map_err(Into::into)
}
