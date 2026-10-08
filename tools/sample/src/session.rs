use std::io::{self, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ToolError;

pub const CSV_HEADER: &str = "utc,pid,private_bytes,working_set_bytes,handles,user_objects,gdi_objects,cpu_time_100ns,cpu_percent,wakeups_per_sec";

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub pid: u32,
    pub private_bytes: u64,
    pub working_set_bytes: u64,
    pub handles: u64,
    pub user_objects: u64,
    pub gdi_objects: u64,
    pub cpu_time_100ns: u64,
    pub wakeups_per_sec: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub utc: String,
    pub pid: u32,
    pub private_bytes: u64,
    pub working_set_bytes: u64,
    pub handles: u64,
    pub user_objects: u64,
    pub gdi_objects: u64,
    pub cpu_time_100ns: u64,
    pub cpu_percent: f64,
    pub wakeups_per_sec: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    DurationReached { samples: usize },
    ProcessExited { samples: usize },
}

pub trait Probe {
    fn read(&mut self) -> Result<Option<Reading>, ToolError>;
}

pub trait Clock {
    fn elapsed(&mut self) -> Duration;
    fn sleep(&mut self, duration: Duration);
    fn now(&mut self) -> SystemTime;
}

#[derive(Debug)]
pub struct SystemClock {
    start: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn elapsed(&mut self) -> Duration {
        self.start.elapsed()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn now(&mut self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Debug, Clone)]
pub struct ManualClock {
    start: SystemTime,
    at: Duration,
}

impl ManualClock {
    pub fn new(start: SystemTime) -> Self {
        Self {
            start,
            at: Duration::ZERO,
        }
    }
}

impl Clock for ManualClock {
    fn elapsed(&mut self) -> Duration {
        self.at
    }

    fn sleep(&mut self, duration: Duration) {
        self.at += duration;
    }

    fn now(&mut self) -> SystemTime {
        self.start + self.at
    }
}

/// 先读一次作为基线，不写入 CSV。之后按固定节拍写一行。
/// 节拍从进入本函数的时刻对齐：`origin + k * interval`。读完计数器后只睡到下一个节拍。
/// 某次读取超过一个间隔时，跳到其后的下一个节拍，不补采落下的拍，也不把后续间隔越拉越长。
/// 目标进程退出时停止，已经写出的行保留在 `out` 里。
pub fn sample_to_writer<P, C, W>(
    probe: &mut P,
    clock: &mut C,
    duration: Duration,
    interval: Duration,
    out: &mut W,
) -> Result<StopReason, ToolError>
where
    P: Probe,
    C: Clock,
    W: Write,
{
    if duration.is_zero() {
        return Err(ToolError::new("采样时长必须大于 0"));
    }
    if interval.is_zero() {
        return Err(ToolError::new("采样间隔必须大于 0"));
    }
    writeln!(out, "{CSV_HEADER}")?;
    out.flush()?;

    let origin = clock.elapsed();
    let end = origin.saturating_add(duration);
    let Some(baseline) = probe.read()? else {
        return Ok(StopReason::ProcessExited { samples: 0 });
    };
    let mut previous_cpu = baseline.cpu_time_100ns;
    let mut previous_at = clock.elapsed();
    let mut samples = 0usize;

    loop {
        let now = clock.elapsed();
        if now >= end {
            return Ok(StopReason::DurationReached { samples });
        }
        let deadline = next_deadline(origin, interval, now);
        if deadline > end {
            return Ok(StopReason::DurationReached { samples });
        }
        while clock.elapsed() < deadline {
            let remain = deadline.saturating_sub(clock.elapsed());
            clock.sleep(remain);
        }
        let sampled_at = clock.elapsed();
        let Some(reading) = probe.read()? else {
            return Ok(StopReason::ProcessExited { samples });
        };
        let sample = Sample {
            utc: format_utc(clock.now()),
            pid: reading.pid,
            private_bytes: reading.private_bytes,
            working_set_bytes: reading.working_set_bytes,
            handles: reading.handles,
            user_objects: reading.user_objects,
            gdi_objects: reading.gdi_objects,
            cpu_time_100ns: reading.cpu_time_100ns,
            cpu_percent: cpu_percent(
                reading.cpu_time_100ns.saturating_sub(previous_cpu),
                sampled_at.saturating_sub(previous_at),
            ),
            wakeups_per_sec: reading.wakeups_per_sec,
        };
        previous_cpu = reading.cpu_time_100ns;
        previous_at = sampled_at;
        write_sample(out, &sample)?;
        out.flush()?;
        samples += 1;
    }
}

/// 严格晚于 `now` 的下一个节拍：`origin + k * interval`，k 从 1 起。
/// 落在节拍上时取再下一拍。读取超过一个间隔时，落下的拍直接跳过，网格仍相对 `origin`。
pub fn next_deadline(origin: Duration, interval: Duration, now: Duration) -> Duration {
    let elapsed = now.saturating_sub(origin);
    let step = interval.as_nanos();
    let k = (elapsed.as_nanos() / step).saturating_add(1);
    origin.saturating_add(mul_duration(interval, k))
}

fn mul_duration(interval: Duration, k: u128) -> Duration {
    let Ok(factor) = u32::try_from(k) else {
        return Duration::MAX;
    };
    interval.saturating_mul(factor)
}

pub fn p95_index_1based(n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    Some(usize::try_from((n as u128 * 95).div_ceil(100)).expect("rank fits in usize"))
}

pub fn p95(values: &[u64]) -> Option<u64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = p95_index_1based(sorted.len())?;
    sorted.get(index - 1).copied()
}

/// 进程 CPU 百分比，分母是墙钟，分子是该进程全部线程的 kernel+user。
/// 多核上可以超过 100。架构没有规定这个百分比怎么算，验收以 `cpu_time_100ns` 的差值为主。
pub fn cpu_percent(delta_cpu_100ns: u64, wall: Duration) -> f64 {
    let wall_100ns = wall.as_nanos() / 100;
    if wall_100ns == 0 {
        return 0.0;
    }
    (delta_cpu_100ns as f64) / (wall_100ns as f64) * 100.0
}

pub fn format_utc(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX);
    let tod = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn write_sample(out: &mut impl Write, sample: &Sample) -> io::Result<()> {
    writeln!(
        out,
        "{},{},{},{},{},{},{},{},{:.6},{:.6}",
        sample.utc,
        sample.pid,
        sample.private_bytes,
        sample.working_set_bytes,
        sample.handles,
        sample.user_objects,
        sample.gdi_objects,
        sample.cpu_time_100ns,
        sample.cpu_percent,
        sample.wakeups_per_sec
    )
}

/// Howard Hinnant 的 civil_from_days。只用于非负 Unix 天数。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year as i32, month as u32, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Scripted {
        steps: VecDeque<Result<Option<Reading>, ToolError>>,
    }

    impl Probe for Scripted {
        fn read(&mut self) -> Result<Option<Reading>, ToolError> {
            self.steps.pop_front().unwrap_or(Ok(None))
        }
    }

    fn reading(cpu: u64, wakeups: f64) -> Reading {
        Reading {
            pid: 42,
            private_bytes: 1000,
            working_set_bytes: 2000,
            handles: 7,
            user_objects: 3,
            gdi_objects: 4,
            cpu_time_100ns: cpu,
            wakeups_per_sec: wakeups,
        }
    }

    #[test]
    fn p95_of_1_through_100_is_95() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(p95_index_1based(100), Some(95));
        assert_eq!(p95(&values), Some(95));
        let mut reversed = values.clone();
        reversed.reverse();
        assert_eq!(p95(&reversed), Some(95));
        assert_eq!(p95(&[]), None);
        assert_eq!(p95(&[8]), Some(8));
    }

    #[test]
    fn process_exit_keeps_rows_already_written() {
        let mut probe = Scripted {
            steps: VecDeque::from([
                Ok(Some(reading(1_000, 0.0))),
                Ok(Some(reading(1_000 + 5_000_000, 12.5))),
                Ok(None),
            ]),
        };
        let mut clock = ManualClock::new(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let mut csv = Vec::new();
        let stop = sample_to_writer(
            &mut probe,
            &mut clock,
            Duration::from_secs(3600),
            Duration::from_secs(1),
            &mut csv,
        )
        .unwrap();
        assert_eq!(stop, StopReason::ProcessExited { samples: 1 });
        let text = String::from_utf8(csv).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(CSV_HEADER));
        let row = lines.next().unwrap();
        assert!(lines.next().is_none());
        let fields: Vec<_> = row.split(',').collect();
        assert_eq!(fields.len(), 10);
        assert_eq!(fields[0], "2023-11-14T22:13:21.000Z");
        assert_eq!(fields[1], "42");
        assert_eq!(fields[2], "1000");
        assert_eq!(fields[3], "2000");
        assert_eq!(fields[4], "7");
        assert_eq!(fields[5], "3");
        assert_eq!(fields[6], "4");
        assert_eq!(fields[7], "5001000");
        assert_eq!(fields[8], "50.000000");
        assert_eq!(fields[9], "12.500000");
    }

    #[test]
    fn deadlines_stay_on_the_origin_grid() {
        let origin = Duration::from_secs(10);
        let interval = Duration::from_secs(1);
        assert_eq!(
            next_deadline(origin, interval, origin),
            origin + Duration::from_secs(1)
        );
        assert_eq!(
            next_deadline(origin, interval, Duration::from_secs(9)),
            origin + Duration::from_secs(1)
        );
        assert_eq!(
            next_deadline(origin, interval, origin + Duration::from_secs(1)),
            origin + Duration::from_secs(2)
        );
        assert_eq!(
            next_deadline(origin, interval, origin + Duration::from_millis(1_100)),
            origin + Duration::from_secs(2)
        );
        assert_eq!(
            next_deadline(origin, interval, origin + Duration::from_millis(2_400)),
            origin + Duration::from_secs(3)
        );
        assert_eq!(
            next_deadline(origin, interval, origin + Duration::from_millis(100_050)),
            origin + Duration::from_secs(101)
        );
        assert_eq!(
            next_deadline(
                Duration::ZERO,
                Duration::from_millis(250),
                Duration::from_millis(250)
            ),
            Duration::from_millis(500)
        );
        assert_eq!(
            next_deadline(
                Duration::ZERO,
                Duration::from_millis(250),
                Duration::from_millis(260)
            ),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn three_hundred_seconds_writes_three_hundred_rows_on_the_grid() {
        let mut steps = VecDeque::new();
        steps.push_back(Ok(Some(reading(0, 0.0))));
        for i in 1..=300 {
            steps.push_back(Ok(Some(reading(0, i as f64))));
        }
        let mut probe = Scripted { steps };
        let mut clock = ManualClock::new(UNIX_EPOCH);
        let mut csv = Vec::new();
        let stop = sample_to_writer(
            &mut probe,
            &mut clock,
            Duration::from_secs(300),
            Duration::from_secs(1),
            &mut csv,
        )
        .unwrap();
        assert_eq!(stop, StopReason::DurationReached { samples: 300 });
        let text = String::from_utf8(csv).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(CSV_HEADER));
        let rows: Vec<_> = lines.collect();
        assert_eq!(rows.len(), 300);
        assert!(rows[0].starts_with("1970-01-01T00:00:01.000Z,"));
        assert!(rows[299].starts_with("1970-01-01T00:05:00.000Z,"));
        assert!(rows[1].starts_with("1970-01-01T00:00:02.000Z,"));
    }

    #[test]
    fn a_read_longer_than_the_interval_skips_to_the_next_beat() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct Shared {
            at: Rc<Cell<Duration>>,
        }

        impl Clock for Shared {
            fn elapsed(&mut self) -> Duration {
                self.at.get()
            }

            fn sleep(&mut self, duration: Duration) {
                self.at.set(self.at.get().saturating_add(duration));
            }

            fn now(&mut self) -> SystemTime {
                UNIX_EPOCH + self.at.get()
            }
        }

        struct Slow {
            at: Rc<Cell<Duration>>,
            left: usize,
        }

        impl Probe for Slow {
            fn read(&mut self) -> Result<Option<Reading>, ToolError> {
                self.at
                    .set(self.at.get().saturating_add(Duration::from_millis(1_200)));
                if self.left == 0 {
                    return Ok(None);
                }
                self.left -= 1;
                Ok(Some(reading(0, 0.0)))
            }
        }

        let at = Rc::new(Cell::new(Duration::ZERO));
        let mut clock = Shared { at: Rc::clone(&at) };
        let mut probe = Slow { at, left: 8 };
        let mut csv = Vec::new();
        let stop = sample_to_writer(
            &mut probe,
            &mut clock,
            Duration::from_secs(5),
            Duration::from_secs(1),
            &mut csv,
        )
        .unwrap();
        assert_eq!(stop, StopReason::DurationReached { samples: 2 });
        let text = String::from_utf8(csv).unwrap();
        let rows: Vec<_> = text.lines().skip(1).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].starts_with("1970-01-01T00:00:03.200Z,"));
        assert!(rows[1].starts_with("1970-01-01T00:00:05.200Z,"));
    }

    #[test]
    fn duration_end_writes_one_row_per_interval() {
        let mut probe = Scripted {
            steps: VecDeque::from([
                Ok(Some(reading(0, 1.0))),
                Ok(Some(reading(0, 2.0))),
                Ok(Some(reading(0, 3.0))),
                Ok(Some(reading(0, 4.0))),
            ]),
        };
        let mut clock = ManualClock::new(UNIX_EPOCH);
        let mut csv = Vec::new();
        let stop = sample_to_writer(
            &mut probe,
            &mut clock,
            Duration::from_secs(3),
            Duration::from_secs(1),
            &mut csv,
        )
        .unwrap();
        assert_eq!(stop, StopReason::DurationReached { samples: 3 });
        let rows = String::from_utf8(csv).unwrap().lines().count();
        assert_eq!(rows, 4);
    }

    #[test]
    fn exit_before_the_first_interval_keeps_the_header() {
        let mut probe = Scripted {
            steps: VecDeque::from([Ok(Some(reading(0, 0.0))), Ok(None)]),
        };
        let mut clock = ManualClock::new(UNIX_EPOCH);
        let mut csv = Vec::new();
        let stop = sample_to_writer(
            &mut probe,
            &mut clock,
            Duration::from_secs(10),
            Duration::from_secs(1),
            &mut csv,
        )
        .unwrap();
        assert_eq!(stop, StopReason::ProcessExited { samples: 0 });
        assert_eq!(String::from_utf8(csv).unwrap(), format!("{CSV_HEADER}\n"));
    }

    #[test]
    fn utc_format_matches_known_instants() {
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_utc(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20.000Z"
        );
    }

    #[test]
    fn cpu_percent_uses_process_time_over_wall_time() {
        assert_eq!(cpu_percent(5_000_000, Duration::from_secs(1)), 50.0);
        assert_eq!(cpu_percent(20_000_000, Duration::from_secs(1)), 200.0);
        assert_eq!(cpu_percent(1, Duration::ZERO), 0.0);
    }
}
