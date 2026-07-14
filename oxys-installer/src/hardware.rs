use oxys::{
    detect::{detect_cpu_count, detect_disks, detect_gpu, detect_ram, is_laptop, is_vendor},
    manifest::{Gpu, GpuVendor, GB},
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Debug)]
pub(crate) enum HardwareDetectEvent {
    Row(String, String),
    Done,
}

pub(crate) async fn stream_hardware(tx: UnboundedSender<HardwareDetectEvent>) {
    let rows = detect_hardware_rows();

    for (k, v) in &rows {
        let _ = tx.send(HardwareDetectEvent::Row(k.clone(), v.clone()));
    }

    let _ = tx.send(HardwareDetectEvent::Done);
}

fn detect_hardware_rows() -> Vec<(String, String)> {
    let ram = detect_ram()
        .map(|bytes| format!("{:.1} GiB", bytes as f64 / GB as f64))
        .unwrap_or_else(|| "unknown".to_string());

    let power = match (is_laptop(), is_vendor("asus")) {
        (true, true) => "laptop (asus_ctl)",
        (true, false) => "laptop (tlp)",
        (false, _) => "desktop",
    };

    vec![
        ("CPU".to_string(), detect_cpu_model()),
        ("RAM".to_string(), ram),
        ("GPU".to_string(), gpu_display(detect_gpu())),
        ("Disks".to_string(), disks_display()),
        ("Power".to_string(), power.to_string()),
    ]
}

fn detect_cpu_model() -> String {
    let fallback = || format!("{} logical CPUs", detect_cpu_count());

    let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") else {
        return fallback();
    };

    cpuinfo
        .lines()
        .find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|line| line.split_once(':'))
        })
        .map(|(_, model)| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(fallback)
}

/// Strips marketing filler ("12th Gen", "Intel(R) Core(TM)", clock speed,
/// core-count suffixes) from a `/proc/cpuinfo` model name, e.g. turns
/// "12th Gen Intel(R) Core(TM) i9-12900H" into "i9-12900H".
pub(crate) fn shorten_cpu_model(model: &str) -> String {
    let cleaned = model
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace("(C)", "");

    let is_ordinal = |w: &str| {
        w.len() > 2
            && w[..w.len() - 2].chars().all(|c| c.is_ascii_digit())
            && matches!(&w[w.len() - 2..], "th" | "st" | "nd" | "rd")
    };

    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if is_ordinal(w) && words.get(i + 1) == Some(&"Gen") {
            continue;
        }
        match *w {
            "Gen" | "Intel" | "Core" | "AMD" | "Genuine" | "Processor" | "CPU" => continue,
            "@" => continue,
            w if w.ends_with("GHz") || w.ends_with("-Core") => continue,
            _ => out.push(w),
        }
    }

    let result = out.join(" ");
    if result.trim().is_empty() {
        model.trim().to_string()
    } else {
        result
    }
}

fn disks_display() -> String {
    let disks = detect_disks();
    if disks.is_empty() {
        return "no installable disks detected".to_string();
    }

    disks
        .iter()
        .map(|disk| format!("{:.1} GiB {}", disk.size as f64 / GB as f64, disk.model))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn gpu_display(gpu: Gpu) -> String {
    match gpu {
        Gpu::Auto => "undetected".to_string(),
        Gpu::Single(vendor) => vendor_name(vendor).to_string(),
        Gpu::Hybrid { igpu, dgpu } => {
            format!("{} + {} (hybrid)", vendor_name(igpu), vendor_name(dgpu))
        }
    }
}

fn vendor_name(vendor: GpuVendor) -> &'static str {
    match vendor {
        GpuVendor::Amd => "AMD",
        GpuVendor::Intel => "Intel",
        GpuVendor::Nvidia => "NVIDIA",
    }
}
