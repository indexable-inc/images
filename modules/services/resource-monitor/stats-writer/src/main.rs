// Numeric arithmetic here is metric/billing math (bytes, seconds, USD) where
// f64 precision and i64↔u32 date conversions are intentional and bounded by
// domain. Allow the pedantic cast lints at the crate level rather than
// peppering call sites.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SECONDS_PER_HOUR: f64 = 60.0 * 60.0;
const BILLING_MONTH_SECONDS: f64 = 30.0 * 24.0 * SECONDS_PER_HOUR;

#[derive(Debug)]
struct Config {
    output_dir: PathBuf,
    df: PathBuf,
    interval_seconds: u64,
    total_cores: f64,
    total_memory_gib: f64,
    total_storage_tib: f64,
    cpu_usd_per_vcpu_month: f64,
    memory_usd_per_gib_hour: f64,
    storage_usd_per_tib_hour: f64,
    margin_multiplier: f64,
}

#[derive(Debug)]
struct CpuSample {
    total: u64,
    idle: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let config = parse_args(&args)?;
    fs::create_dir_all(&config.output_dir)?;

    loop {
        write_stats(&config)?;
        thread::sleep(Duration::from_secs(config.interval_seconds));
    }
}

fn parse_args(args: &[String]) -> Result<Config, Box<dyn Error>> {
    let mut config = Config {
        output_dir: PathBuf::from("/run/resource-monitor"),
        df: PathBuf::from("df"),
        interval_seconds: 1,
        total_cores: 64.0,
        total_memory_gib: 256.0,
        total_storage_tib: 1024.0,
        cpu_usd_per_vcpu_month: 20.0,
        memory_usd_per_gib_hour: 0.005,
        storage_usd_per_tib_hour: 0.0031,
        margin_multiplier: 2.0,
    };

    let mut pairs = args.chunks_exact(2);
    if !pairs.remainder().is_empty() {
        return Err("arguments must be --flag value pairs".into());
    }

    for pair in pairs.by_ref() {
        let flag = pair[0].as_str();
        let value = pair[1].as_str();
        match flag {
            "--output-dir" => config.output_dir = PathBuf::from(value),
            "--df" => config.df = PathBuf::from(value),
            "--interval-seconds" => config.interval_seconds = parse_positive(value, flag)?,
            "--total-cores" => config.total_cores = parse_positive(value, flag)?,
            "--total-memory-gib" => config.total_memory_gib = parse_positive(value, flag)?,
            "--total-storage-tib" => config.total_storage_tib = parse_positive(value, flag)?,
            "--cpu-usd-per-vcpu-month" => {
                config.cpu_usd_per_vcpu_month = parse_positive(value, flag)?;
            }
            "--memory-usd-per-gib-hour" => {
                config.memory_usd_per_gib_hour = parse_positive(value, flag)?;
            }
            "--storage-usd-per-tib-hour" => {
                config.storage_usd_per_tib_hour = parse_positive(value, flag)?;
            }
            "--margin-multiplier" => config.margin_multiplier = parse_positive(value, flag)?,
            _ => return Err(format!("unknown argument: {flag}").into()),
        }
    }

    Ok(config)
}

fn parse_positive<T>(value: &str, flag: &str) -> Result<T, Box<dyn Error>>
where
    T: std::str::FromStr + PartialOrd + From<u8>,
    T::Err: Error + 'static,
{
    let parsed: T = value.parse()?;
    if parsed <= T::from(0) {
        return Err(format!("{flag} must be positive").into());
    }
    Ok(parsed)
}

fn write_stats(config: &Config) -> Result<(), Box<dyn Error>> {
    let cpu_a = read_cpu_sample()?;
    thread::sleep(Duration::from_millis(200));
    let cpu_b = read_cpu_sample()?;

    let total_delta = cpu_b.total.saturating_sub(cpu_a.total);
    let idle_delta = cpu_b.idle.saturating_sub(cpu_a.idle);
    let cpu_percent = if total_delta == 0 {
        0.0
    } else {
        ((total_delta - idle_delta) as f64 / total_delta as f64) * 100.0
    };

    let memory_used_bytes = read_memory_used_bytes()?;
    let disk_used_bytes = read_disk_used_bytes(&config.df)?;
    let memory_total_bytes = config.total_memory_gib * 1024.0 * 1024.0 * 1024.0;
    let disk_total_bytes = config.total_storage_tib * 1024.0 * 1024.0 * 1024.0 * 1024.0;
    let cpu_used_cores = config.total_cores * cpu_percent / 100.0;
    let memory_used_gib = memory_used_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let disk_used_tib = disk_used_bytes as f64 / 1024.0 / 1024.0 / 1024.0 / 1024.0;
    let cost_per_second_usd = cpu_used_cores
        * (config.cpu_usd_per_vcpu_month / BILLING_MONTH_SECONDS)
        + memory_used_gib
            * ((config.memory_usd_per_gib_hour / SECONDS_PER_HOUR) * config.margin_multiplier)
        + disk_used_tib
            * ((config.storage_usd_per_tib_hour / SECONDS_PER_HOUR) * config.margin_multiplier);

    // Hand-rolled JSON instead of serde: this daemon ships with an empty
    // `[dependencies]` (see Cargo.toml) and runs on every resource-monitor VM,
    // so keeping the closure tiny is worth emitting the object by hand. The
    // shape below is the wire contract checked by the `usageStatsSchema` valibot
    // schema in the site's `src/lib/stats.ts`; keep the two in sync.
    let stats = format!(
        concat!(
            "{{",
            "\"generatedAt\":\"{}\",",
            "\"cpu\":{{\"usedCores\":{},\"totalCores\":{},\"percent\":{}}},",
            "\"memory\":{{\"usedBytes\":{},\"totalBytes\":{},\"percent\":{}}},",
            "\"disk\":{{\"usedBytes\":{},\"totalBytes\":{},\"percent\":{}}},",
            "\"costPerSecondUsd\":{}",
            "}}\n"
        ),
        iso_timestamp(SystemTime::now())?,
        round(cpu_used_cores, 4),
        trim_float(config.total_cores),
        round(cpu_percent, 4),
        memory_used_bytes,
        trim_float(memory_total_bytes),
        round(memory_used_bytes as f64 / memory_total_bytes * 100.0, 4),
        disk_used_bytes,
        trim_float(disk_total_bytes),
        round(disk_used_bytes as f64 / disk_total_bytes * 100.0, 6),
        trim_float(cost_per_second_usd),
    );

    write_atomic(&config.output_dir, "stats.json", stats.as_bytes())?;
    Ok(())
}

fn read_cpu_sample() -> Result<CpuSample, Box<dyn Error>> {
    let stat = fs::read_to_string("/proc/stat")?;
    let cpu = stat.lines().next().ok_or("missing /proc/stat cpu line")?;
    let values: Vec<u64> = cpu
        .split_whitespace()
        .skip(1)
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if values.len() < 5 {
        return Err("incomplete /proc/stat cpu line".into());
    }

    Ok(CpuSample {
        total: values.iter().sum(),
        idle: values[3] + values[4],
    })
}

fn read_memory_used_bytes() -> Result<u64, Box<dyn Error>> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    let mut mem_total = None;
    let mut mem_available = None;

    for line in meminfo.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("MemTotal:") => mem_total = fields.next().map(str::parse).transpose()?,
            Some("MemAvailable:") => mem_available = fields.next().map(str::parse).transpose()?,
            _ => {}
        }
    }

    let total: u64 = mem_total.ok_or("missing MemTotal in /proc/meminfo")?;
    let available: u64 = mem_available.ok_or("missing MemAvailable in /proc/meminfo")?;
    Ok(total.saturating_sub(available) * 1024)
}

fn read_disk_used_bytes(df: &Path) -> Result<u64, Box<dyn Error>> {
    let output = Command::new(df)
        .args(["-B1", "--output=used", "/"])
        .output()?;
    if !output.status.success() {
        return Err(format!("df failed with {}", output.status).into());
    }

    let stdout = String::from_utf8(output.stdout)?;
    let used = stdout
        .lines()
        .nth(1)
        .ok_or("df output did not include a used byte row")?
        .trim()
        .parse()?;
    Ok(used)
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let dest = dir.join(name);
    fs::write(&tmp, bytes)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
    fs::rename(tmp, dest)
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn round(value: f64, places: i32) -> String {
    let factor = 10_f64.powi(places);
    trim_float((value * factor).round() / factor)
}

fn trim_float(value: f64) -> String {
    let text = format!("{value:.12}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn iso_timestamp(time: SystemTime) -> Result<String, Box<dyn Error>> {
    let unix = time.duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3600;
    let minute = (seconds % 3600) / 60;
    let second = seconds % 60;

    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

const fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + (month <= 2) as i64;

    (year, month as u32, day as u32)
}
