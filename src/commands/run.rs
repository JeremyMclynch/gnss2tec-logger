use crate::args::{ConvertArgs, RunArgs};
use crate::commands::convert::run_convert;
use crate::commands::log::run_log_with_signal;
use crate::shared::signal::install_ctrlc_handler;
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// Public run command entrypoint. This mode keeps logging active while conversion runs on a timer.
pub fn run_mode(args: RunArgs) -> Result<()> {
    // One shared run flag coordinates Ctrl-C shutdown for both logger and converter.
    let running = install_ctrlc_handler()?;
    let log_args = args.to_log_args();
    let convert_args = args.to_convert_args();
    let convert_interval = Duration::from_secs(args.convert_interval_secs.max(1));
    let convert_on_start = args.convert_on_start;

    // Start conversion in a background thread so logger I/O stays uninterrupted.
    let convert_running = Arc::clone(&running);
    let convert_handle = spawn_convert_loop(
        convert_args,
        convert_running,
        convert_interval,
        convert_on_start,
    );

    // Keep logger on main thread; regardless of logger result, stop converter and join cleanly.
    let log_result = run_log_with_signal(log_args, Arc::clone(&running));
    running.store(false, Ordering::SeqCst);
    join_convert_loop(convert_handle);
    log_result
}

// Spawn periodic conversion worker. Any conversion error is logged and the loop keeps going.
fn spawn_convert_loop(
    convert_args: ConvertArgs,
    running: Arc<AtomicBool>,
    interval: Duration,
    run_immediately: bool,
) -> JoinHandle<()> {
    thread::spawn(move || {
        if run_immediately && running.load(Ordering::SeqCst) {
            execute_convert_once(&convert_args);
        }

        while running.load(Ordering::SeqCst) {
            if !sleep_until_next_cycle(&running, interval) {
                break;
            }
            execute_convert_once(&convert_args);
        }
    })
}

// Join conversion worker and report panic if it occurred.
fn join_convert_loop(handle: JoinHandle<()>) {
    if let Err(err) = handle.join() {
        eprintln!("Convert loop thread terminated unexpectedly: {:?}", err);
    }
}

// Sleep in small slices so shutdown reacts quickly to Ctrl-C.
fn sleep_until_next_cycle(running: &AtomicBool, interval: Duration) -> bool {
    let started = Instant::now();
    while running.load(Ordering::SeqCst) && started.elapsed() < interval {
        thread::sleep(Duration::from_millis(250));
    }
    running.load(Ordering::SeqCst)
}

// Run one conversion cycle and swallow errors so logger continuity is preserved.
fn execute_convert_once(convert_args: &ConvertArgs) {
    if let Err(err) = run_convert(convert_args.clone()) {
        eprintln!("Convert cycle failed (logger continues): {err:#}");
    }
}
