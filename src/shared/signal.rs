use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Install Ctrl-C handler and return a shared run flag.
// Commands poll this flag to stop cleanly without abrupt termination.
pub fn install_ctrlc_handler() -> Result<Arc<AtomicBool>> {
    let running = Arc::new(AtomicBool::new(true));
    let running_for_signal = Arc::clone(&running);
    ctrlc::set_handler(move || {
        running_for_signal.store(false, Ordering::SeqCst);
    })
    .context("installing Ctrl-C handler failed")?;
    Ok(running)
}
