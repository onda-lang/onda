use super::*;
use onda_daemon::{RunOutputCounts, RunPackedOutput};

#[cfg(test)]
mod tests;

const PACKET_COUNT: usize = 8;

/// A fixed packet pool bounds queued output and keeps decoding, formatting and
/// decoded payload destruction off the render producer.
pub(super) struct OutputWorker {
    sender: Option<mpsc::SyncSender<(RunPackedOutput, u64)>>,
    available: mpsc::Receiver<RunPackedOutput>,
    output: Option<OutputTransport>,
    prints: PrintTransport,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl OutputWorker {
    pub(super) fn new(
        run: &RunSession,
        output: Option<OutputTransport>,
        prints: PrintTransport,
    ) -> Self {
        let decoder = run.output_decoder();
        let (sender, receiver) = mpsc::sync_channel::<(RunPackedOutput, u64)>(PACKET_COUNT);
        let (free, available) = mpsc::sync_channel(PACKET_COUNT);
        for _ in 0..PACKET_COUNT {
            assert!(
                free.try_send(decoder.allocate_packet()).is_ok(),
                "empty packet pool"
            );
        }
        let error = Arc::new(Mutex::new(None));
        let worker_error = error.clone();
        let worker_output = output.clone();
        let worker_prints = prints.clone();
        let thread = thread::spawn(move || {
            while let Ok((mut packet, subscription)) = receiver.recv() {
                match decoder.decode(&mut packet) {
                    Ok((delegates, prints)) => publish_decoded(
                        delegates,
                        prints,
                        worker_output.as_ref(),
                        &worker_prints,
                        subscription,
                    ),
                    Err(diagnostic) => {
                        *worker_error.lock().unwrap() = Some(format_single_diagnostic(
                            "daemon play output decoding failed",
                            &diagnostic,
                        ));
                    }
                }
                let _ = free.try_send(packet);
            }
        });
        Self {
            sender: Some(sender),
            available,
            output,
            prints,
            error,
            thread: Some(thread),
        }
    }

    pub(super) fn publish(&self, run: &mut RunSession, subscription: u64) -> Result<(), String> {
        if let Ok(error) = self.error.try_lock() {
            if let Some(error) = &*error {
                return Err(error.clone());
            }
        }
        let counts = run.output_counts();
        if counts.is_empty() {
            return Ok(());
        }
        if let Ok(mut packet) = self.available.try_recv() {
            run.swap_output(&mut packet);
            // A packet removed from the free pool always leaves room in the
            // ready queue: exactly PACKET_COUNT packets exist in total.
            self.sender
                .as_ref()
                .unwrap()
                .try_send((packet, subscription))
                .map_err(|_| "output worker disconnected".to_owned())?;
        } else {
            self.record_loss(counts);
            run.discard_output();
        }
        Ok(())
    }

    fn record_loss(&self, counts: RunOutputCounts) {
        if let Some(output) = &self.output {
            record_transport_loss(
                &output.dropped_delegates,
                &output.pending_delegate_overflow,
                counts.delegates as usize,
                counts.delegate_overflow,
            );
            record_transport_loss(
                &output.dropped_prints,
                &output.pending_print_overflow,
                counts.prints as usize,
                counts.print_overflow,
            );
        } else {
            record_transport_loss(
                &self.prints.dropped_occurrences,
                &self.prints.pending_overflow,
                counts.prints as usize,
                counts.print_overflow,
            );
        }
    }
}

impl Drop for OutputWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn publish_decoded(
    delegates: RunDelegateBatch,
    prints: RunPrintBatch,
    output_transport: Option<&OutputTransport>,
    print_transport: &PrintTransport,
    delegate_subscription_id: u64,
) {
    if delegates.occurrences.is_empty()
        && delegates.overflow_count == 0
        && prints.entries.is_empty()
        && prints.overflow_count == 0
    {
        return;
    }
    if let Some(transport) = output_transport {
        let output = RunOutputBatch {
            delegate_subscription_id,
            delegates,
            prints,
        };
        if let Err(mpsc::TrySendError::Full(output)) = transport.sender.try_send(output) {
            record_transport_loss(
                &transport.dropped_delegates,
                &transport.pending_delegate_overflow,
                output.delegates.occurrences.len(),
                output.delegates.overflow_count,
            );
            record_transport_loss(
                &transport.dropped_prints,
                &transport.pending_print_overflow,
                output.prints.entries.len(),
                output.prints.overflow_count,
            );
        }
        return;
    }
    if prints.entries.is_empty() && prints.overflow_count == 0 {
        return;
    }
    if let Err(mpsc::TrySendError::Full(batch)) = print_transport.sender.try_send(prints) {
        record_transport_loss(
            &print_transport.dropped_occurrences,
            &print_transport.pending_overflow,
            batch.entries.len(),
            batch.overflow_count,
        );
    }
}
