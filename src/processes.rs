use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use crate::gpu::GpuReader;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessSort {
    Cpu,
    Gpu,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessUsage {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub gpu_percent: Option<f64>,
}

pub enum ProcessMessage {
    Snapshot {
        processes: Vec<ProcessUsage>,
        gpu_available: bool,
        gpu_error: Option<String>,
    },
    Error(String),
}

pub struct ProcessMonitor {
    pub rx: Receiver<ProcessMessage>,
    stop: Arc<AtomicBool>,
}

struct GpuBaseline {
    sampled_at: Instant,
    times: HashMap<u32, u64>,
    names: HashMap<u32, String>,
}

impl ProcessMonitor {
    pub fn spawn() -> ProcessMonitor {
        let (tx, rx) = sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        thread::spawn(move || {
            let gpu_reader = GpuReader::new();
            let mut gpu_baseline = None;

            while !worker_stop.load(Ordering::Relaxed) {
                let sampled_at = Instant::now();
                match sample_ps(std::process::id()) {
                    Ok(mut processes) => {
                        let (gpu_available, gpu_error) = match &gpu_reader {
                            Ok(reader) => match reader.read_times() {
                                Ok(times) => {
                                    apply_gpu_rates(
                                        &mut processes,
                                        &times,
                                        gpu_baseline.as_ref(),
                                        sampled_at,
                                    );
                                    gpu_baseline = Some(GpuBaseline {
                                        sampled_at,
                                        times,
                                        names: process_names(&processes),
                                    });
                                    (true, None)
                                }
                                Err(error) => {
                                    gpu_baseline = None;
                                    (false, Some(error.to_string()))
                                }
                            },
                            Err(error) => (false, Some(error.to_string())),
                        };
                        let _ = tx.try_send(ProcessMessage::Snapshot {
                            processes,
                            gpu_available,
                            gpu_error,
                        });
                    }
                    Err(error) => {
                        let _ = tx.try_send(ProcessMessage::Error(format!(
                            "process sampling failed: {error}"
                        )));
                    }
                }
                for _ in 0..10 {
                    if worker_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
        ProcessMonitor { rx, stop }
    }

    pub fn try_recv(&self) -> Result<ProcessMessage, TryRecvError> {
        self.rx.try_recv()
    }
}

impl Drop for ProcessMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn sample_ps(ignored_pid: u32) -> std::io::Result<Vec<ProcessUsage>> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,pcpu=,comm="])
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "ps exited with {}",
            output.status
        )));
    }

    Ok(parse_ps(
        &String::from_utf8_lossy(&output.stdout),
        ignored_pid,
    ))
}

fn parse_ps(output: &str, ignored_pid: u32) -> Vec<ProcessUsage> {
    let mut processes = Vec::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(cpu)) = (fields.next(), fields.next()) else {
            continue;
        };
        let command = fields.collect::<Vec<_>>().join(" ");
        if command.is_empty() {
            continue;
        }
        let (Ok(pid), Ok(cpu_percent)) = (pid.parse::<u32>(), cpu.parse::<f64>()) else {
            continue;
        };
        if pid == ignored_pid || !cpu_percent.is_finite() || cpu_percent < 0.0 {
            continue;
        }
        let name = std::path::Path::new(&command)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&command)
            .to_string();
        processes.push(ProcessUsage {
            pid,
            name,
            cpu_percent,
            gpu_percent: None,
        });
    }
    processes
}

fn apply_gpu_rates(
    processes: &mut [ProcessUsage],
    current: &HashMap<u32, u64>,
    previous: Option<&GpuBaseline>,
    sampled_at: Instant,
) {
    let elapsed_ns = previous
        .map(|baseline| sampled_at.duration_since(baseline.sampled_at).as_nanos() as f64)
        .filter(|elapsed| *elapsed > 0.0);

    for process in processes {
        let gpu_percent = previous
            .zip(elapsed_ns)
            .filter(|(baseline, _)| baseline.names.get(&process.pid) == Some(&process.name))
            .and_then(|(baseline, elapsed)| {
                let current_time = current.get(&process.pid)?;
                let previous_time = baseline.times.get(&process.pid)?;
                current_time
                    .checked_sub(*previous_time)
                    .map(|delta| delta as f64 / elapsed * 100.0)
            })
            .filter(|percent| percent.is_finite() && *percent >= 0.0)
            .unwrap_or(0.0);
        process.gpu_percent = Some(gpu_percent);
    }
}

fn process_names(processes: &[ProcessUsage]) -> HashMap<u32, String> {
    processes
        .iter()
        .map(|process| (process.pid, process.name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_columns_and_names_with_spaces() {
        let output = "  12  88.4 /Applications/Render Worker\n  77   1.2 /usr/bin/macfan\n";
        let processes = parse_ps(output, 77);

        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].pid, 12);
        assert_eq!(processes[0].name, "Render Worker");
        assert_eq!(processes[0].cpu_percent, 88.4);
        assert_eq!(processes[0].gpu_percent, None);
    }

    #[test]
    fn calculates_gpu_rate_and_ignores_reused_pid() {
        let sampled_at = Instant::now();
        let baseline = GpuBaseline {
            sampled_at: sampled_at - Duration::from_secs(1),
            times: HashMap::from([(12, 1_000_000_000), (20, 1_000_000_000)]),
            names: HashMap::from([(12, "Renderer".into()), (20, "Old app".into())]),
        };
        let current = HashMap::from([(12, 1_250_000_000), (20, 1_500_000_000)]);
        let mut processes = vec![
            ProcessUsage {
                pid: 12,
                name: "Renderer".into(),
                cpu_percent: 0.0,
                gpu_percent: None,
            },
            ProcessUsage {
                pid: 20,
                name: "New app".into(),
                cpu_percent: 0.0,
                gpu_percent: None,
            },
        ];

        apply_gpu_rates(&mut processes, &current, Some(&baseline), sampled_at);

        assert!((processes[0].gpu_percent.unwrap() - 25.0).abs() < 0.001);
        assert_eq!(processes[1].gpu_percent, Some(0.0));
    }
}
