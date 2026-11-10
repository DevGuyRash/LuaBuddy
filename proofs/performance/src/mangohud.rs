use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Row {
    milliseconds: f64,
    elapsed_ns: u64,
}

pub(crate) struct Log {
    path: PathBuf,
    rows: Vec<Row>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Cadence {
    path: PathBuf,
    prior_boundary_elapsed_ns: u64,
    final_elapsed_ns: u64,
    csv_clock_span_ms: f64,
    interval_sum_ms: f64,
    observer_window_ms: f64,
    interval_tolerance_ms: f64,
    boundary_tolerance_ms: f64,
}

impl Log {
    pub(crate) fn read(directory: &Path) -> Result<Self, String> {
        let mut files = fs::read_dir(directory)
            .map_err(|e| format!("Reading owned MangoHud directory: {e}"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        files.retain(|path| {
            path.extension().is_some_and(|value| value == "csv")
                && !path
                    .file_name()
                    .is_some_and(|value| value.to_string_lossy().ends_with("_summary.csv"))
        });
        if files.len() != 1 {
            return Err(format!(
                "Expected exactly one owned raw MangoHud CSV, found {}",
                files.len()
            ));
        }
        let path = files.pop().ok_or("Missing MangoHud CSV")?;
        let text = fs::read_to_string(&path).map_err(|e| format!("Reading MangoHud CSV: {e}"))?;
        Ok(Self {
            path,
            rows: rows(&text)?,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn values(&self) -> Vec<f64> {
        self.rows.iter().map(|row| row.milliseconds).collect()
    }

    pub(crate) fn window(
        &self,
        first: usize,
        observer_window_ms: f64,
    ) -> Result<(Vec<f64>, Cadence), String> {
        if first == 0 || self.rows.len() <= first + 1 {
            return Err(
                "MangoHud needs a preceding boundary row and two completed measured frames".into(),
            );
        }
        let selected = &self.rows[first..];
        let prior_boundary_elapsed_ns = self.rows[first - 1].elapsed_ns;
        let final_elapsed_ns = self
            .rows
            .last()
            .ok_or("Missing MangoHud final row")?
            .elapsed_ns;
        let csv_clock_span_ms = (final_elapsed_ns - prior_boundary_elapsed_ns) as f64 / 1_000_000.0;
        let interval_sum_ms = selected.iter().map(|row| row.milliseconds).sum::<f64>();
        let largest = selected
            .iter()
            .map(|row| row.milliseconds)
            .fold(0.0, f64::max);
        let interval_tolerance_ms = 100.0_f64.max(csv_clock_span_ms * 0.02);
        let boundary_tolerance_ms = 500.0 + largest;
        if !observer_window_ms.is_finite()
            || observer_window_ms <= 0.0
            || (interval_sum_ms - csv_clock_span_ms).abs() > interval_tolerance_ms
            || (csv_clock_span_ms - observer_window_ms).abs() > boundary_tolerance_ms
        {
            return Err(format!(
                "MangoHud cadence differs from consecutive real frames: interval sum {interval_sum_ms}ms, CSV span {csv_clock_span_ms}ms, observer window {observer_window_ms}ms"
            ));
        }
        let cadence = Cadence {
            path: self.path.clone(),
            prior_boundary_elapsed_ns,
            final_elapsed_ns,
            csv_clock_span_ms,
            interval_sum_ms,
            observer_window_ms,
            interval_tolerance_ms,
            boundary_tolerance_ms,
        };
        Ok((
            selected.iter().map(|row| row.milliseconds).collect(),
            cadence,
        ))
    }
}

fn rows(text: &str) -> Result<Vec<Row>, String> {
    let mut header = None;
    let mut output: Vec<Row> = Vec::new();
    for line in text
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
    {
        let fields = line
            .trim_end_matches(['\r', '\n'])
            .split(',')
            .collect::<Vec<_>>();
        let Some((count, time, elapsed)) = header else {
            if fields.contains(&"frametime") && fields.contains(&"elapsed") {
                if fields.iter().filter(|&&value| value == "frametime").count() != 1
                    || fields.iter().filter(|&&value| value == "elapsed").count() != 1
                {
                    return Err("Ambiguous MangoHud frame headers".into());
                }
                header = Some((
                    fields.len(),
                    fields
                        .iter()
                        .position(|&value| value == "frametime")
                        .ok_or("Missing frametime header")?,
                    fields
                        .iter()
                        .position(|&value| value == "elapsed")
                        .ok_or("Missing elapsed header")?,
                ));
            }
            continue;
        };
        if fields.len() != count {
            return Err("MangoHud row does not match its frame header".into());
        }
        let milliseconds = fields[time]
            .parse::<f64>()
            .map_err(|_| "MangoHud frametime is not numeric")?;
        let elapsed_ns = fields[elapsed]
            .parse::<u64>()
            .map_err(|_| "MangoHud elapsed is not integer nanoseconds")?;
        if !milliseconds.is_finite()
            || milliseconds < 0.0
            || output
                .last()
                .is_some_and(|previous| elapsed_ns <= previous.elapsed_ns)
        {
            return Err("Invalid MangoHud frame duration or elapsed order".into());
        }
        output.push(Row {
            milliseconds,
            elapsed_ns,
        });
    }
    if header.is_none() || output.is_empty() {
        return Err("MangoHud CSV has no complete frame data".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{Log, rows};
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    const HEADER: &str = "fps,frametime,cpu_load,cpu_power,gpu_load,cpu_temp,gpu_temp,gpu_core_clock,gpu_mem_clock,gpu_vram_used,gpu_power,ram_used,swap_used,process_rss,cpu_mhz,elapsed\n";

    #[test]
    fn actual_log_format_keeps_complete_frame_rows_and_source_units() {
        let input = format!(
            "os,cpu,gpu,ram,kernel,driver,cpuscheduler\nLinux,CPU,GPU,1,kernel,,performance\n{HEADER}100,10,0,0,0,0,0,0,0,0,0,0,0,0,0,10000000\n50,20,0,0,0,0,0,0,0,0,0,0,0,0,0,30000000\n62.5,16"
        );
        let parsed = rows(&input).unwrap();
        assert_eq!(
            parsed
                .iter()
                .map(|row| row.milliseconds)
                .collect::<Vec<_>>(),
            [10.0, 20.0]
        );
        assert_eq!(
            parsed.iter().map(|row| row.elapsed_ns).collect::<Vec<_>>(),
            [10_000_000, 30_000_000]
        );
        let reordered = rows("elapsed,unused,frametime\n40000000,x,12.5\n").unwrap();
        assert_eq!(reordered[0].milliseconds, 12.5);
        assert_eq!(reordered[0].elapsed_ns, 40_000_000);
    }

    #[test]
    fn malformed_or_ambiguous_frames_cannot_be_fast_successes() {
        for input in [
            "frametime,elapsed\n",
            "frametime,frametime,elapsed\n1,1,1\n",
            "frametime,elapsed\n1,1,extra\n",
            "frametime,elapsed\nNaN,1\n",
            "frametime,elapsed\ninf,1\n",
            "frametime,elapsed\n-1,1\n",
            "frametime,elapsed\n1,1.5\n",
            "frametime,elapsed\n1,-1\n",
            "frametime,elapsed\n1,18446744073709551616\n",
            "frametime,elapsed\n1,2\n1,2\n",
            "frametime,elapsed\n1,2\n1,1\n",
        ] {
            assert!(rows(input).is_err(), "{input}");
        }
    }

    fn log(input: &str) -> Log {
        Log {
            path: PathBuf::from("/owned/frames.csv"),
            rows: rows(input).unwrap(),
        }
    }

    #[test]
    fn selected_rows_use_the_preceding_boundary_and_independent_clocks() {
        let data = log(
            "frametime,elapsed\n16,1000000000\n100,1100000000\n150,1250000000\n250,1500000000\n",
        );
        let (values, cadence) = data.window(1, 500.0).unwrap();
        assert_eq!(values, [100.0, 150.0, 250.0]);
        assert_eq!(cadence.prior_boundary_elapsed_ns, 1_000_000_000);
        assert_eq!(cadence.final_elapsed_ns, 1_500_000_000);
        assert_eq!(cadence.interval_sum_ms, 500.0);
        assert_eq!(cadence.csv_clock_span_ms, 500.0);
        assert!(data.window(0, 500.0).is_err());
        assert!(data.window(3, 500.0).is_err());
        assert!(data.window(1, f64::NAN).is_err());
        assert!(data.window(1, 2_000.0).is_err());
        let gap = log(
            "frametime,elapsed\n16,1000000000\n100,1100000000\n150,1250000000\n250,3000000000\n",
        );
        assert!(gap.window(1, 2_000.0).is_err());
    }

    #[test]
    fn raw_log_discovery_ignores_summaries_and_refuses_multiple_logs() {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let name = format!(
            "luabuddy-mangohud-log-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = Directory(std::env::temp_dir().join(name));
        fs::create_dir(&directory.0).unwrap();
        assert!(Log::read(&directory.0).is_err());
        fs::write(
            directory.0.join("java.csv"),
            "frametime,elapsed\n10,10000000\n20,30000000\n",
        )
        .unwrap();
        fs::write(
            directory.0.join("java_summary.csv"),
            "Different summary schema\n",
        )
        .unwrap();
        assert_eq!(Log::read(&directory.0).unwrap().values(), [10.0, 20.0]);
        fs::write(
            directory.0.join("another.csv"),
            "frametime,elapsed\n10,10000000\n",
        )
        .unwrap();
        assert!(Log::read(&directory.0).is_err());
    }
}
