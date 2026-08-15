/*
 * System Monitor & Process Priority Module
 * Handles process priority elevation and dmesg kernel monitoring for hardware decoder alerts.
 */

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use thread_priority::unix::{RealtimeThreadSchedulePolicy, ThreadSchedulePolicy};
use thread_priority::{set_thread_priority_and_policy, ThreadPriority, ThreadPriorityValue};

pub fn elevate_process_priority() {
    let fifo_policy = ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo);
    let fifo_priority = ThreadPriority::Crossplatform(
        ThreadPriorityValue::try_from(20u8).expect("20 is a valid cross-platform priority"),
    );
    if let Err(error) = set_thread_priority_and_policy(
        thread_priority::unix::thread_native_id(),
        fifo_priority,
        fifo_policy,
    ) {
        println!(
            "[PRIORITY] SCHED_FIFO elevation not permitted ({error}); setting niceness to -10..."
        );
        let _ = rustix::process::setpriority_process(None, -10);
    } else {
        println!("[PRIORITY] Successfully elevated main process to SCHED_FIFO priority 20");
    }
}

pub fn spawn_dmesg_kernel_monitor() {
    elevate_process_priority();
    std::thread::spawn(|| {
        if let Ok(mut child) = Command::new("dmesg").args(["-w"]).stdout(Stdio::piped()).spawn() {
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines().flatten() {
                    let lower = line.to_lowercase();
                    if lower.contains("rkvdec") || lower.contains("v4l2") || lower.contains("rockchip-drm") {
                        if lower.contains("error") || lower.contains("fault") || lower.contains("failed") || lower.contains("corrupt") || lower.contains("warn") {
                            println!("[LAYER 1 ALERT] Kernel Driver Event: {}", line);
                        }
                    }
                }
            }
        }
    });
}
