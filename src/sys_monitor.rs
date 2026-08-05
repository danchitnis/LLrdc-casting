/*
 * System Monitor & Process Priority Module
 * Handles process priority elevation and dmesg kernel monitoring for hardware decoder alerts.
 */

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

pub fn elevate_process_priority() {
    unsafe {
        let param = libc::sched_param { sched_priority: 20 };
        if libc::sched_setscheduler(0, libc::SCHED_FIFO, &param as *const _) == 0 {
            println!("[PRIORITY] Successfully elevated main process to SCHED_FIFO priority 20");
        } else {
            let err = *libc::__errno_location();
            println!("[PRIORITY] SCHED_FIFO elevation not permitted (errno={err}); setting niceness to -10...");
            libc::setpriority(libc::PRIO_PROCESS, 0, -10);
        }
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
