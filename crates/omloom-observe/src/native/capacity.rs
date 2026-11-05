//! Selected execution-context descriptors, never permanent host identifiers.
use super::*;
#[cfg(test)]
mod tests;

fn positive(name: &str, value: &str) -> Result<wire::Field> {
    let value = value
        .trim()
        .parse::<i64>()
        .map_err(|_| "Invalid capacity integer")?;
    if value <= 0 {
        return Err("Invalid capacity integer".into());
    }
    Ok(wire::Field {
        name: name.into(),
        value: Some(wire::field::Value::Integer(value)),
    })
}

pub(super) async fn collect(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let mut fields = Vec::with_capacity(3);
    let mut unavailable = 0;
    match report.platform.as_str() {
        "macos" => {
            for (field, property) in [
                ("logical_cpus", "hw.logicalcpu"),
                ("visible_memory_bytes", "hw.memsize"),
            ] {
                match runner.run("/usr/sbin/sysctl", &["-n", property]).await {
                    Ok(v) => fields.push(positive(field, &v.text)?),
                    Err(_) => unavailable += 1,
                }
            }
            match runner
                .run("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"])
                .await
            {
                Ok(v)
                    if !v.text.trim().is_empty()
                        && v.text.trim().len() <= 512
                        && !v.text.trim().chars().any(char::is_control) =>
                {
                    fields.push(super::super::text("cpu_model", v.text.trim()))
                }
                _ => unavailable += 1,
            }
        }
        "linux" => {
            match runner.run("getconf", &["_NPROCESSORS_ONLN"]).await {
                Ok(v) => fields.push(positive("logical_cpus", &v.text)?),
                Err(_) => unavailable += 1,
            }
            match runner.run("cat", &["/proc/meminfo"]).await {
                Ok(v) => {
                    let raw = v
                        .text
                        .lines()
                        .find_map(|v| v.strip_prefix("MemTotal:"))
                        .ok_or("Memory capacity unavailable")?;
                    let mut words = raw.split_whitespace();
                    let kib = words
                        .next()
                        .ok_or("Memory capacity unavailable")?
                        .parse::<i64>()
                        .map_err(|_| "Invalid memory capacity")?;
                    if words.next() != Some("kB") || kib <= 0 {
                        return Err("Unsupported memory unit".into());
                    }
                    // Linux fs/proc/meminfo.c::show_val_kb shifts page counts
                    // by PAGE_SHIFT-10: its kB values represent 1024 bytes.
                    let bytes = kib.checked_mul(1024).ok_or("Memory capacity overflow")?;
                    fields.push(positive("visible_memory_bytes", &bytes.to_string())?);
                }
                Err(_) => unavailable += 1,
            }
            match runner.run("cat", &["/proc/cpuinfo"]).await {
                Ok(v) => {
                    let mut models = std::collections::BTreeSet::new();
                    let mut bytes = 0;
                    for (name, model) in v.text.lines().filter_map(|v| v.split_once(':')) {
                        if !matches!(name.trim(), "model name" | "Hardware") {
                            continue;
                        }
                        let model = model.trim();
                        if model.chars().any(char::is_control) {
                            unavailable += 1;
                            continue;
                        }
                        if models.contains(model) {
                            continue;
                        }
                        if model.is_empty() || model.len() + 2 > 512 - bytes || models.len() == 16 {
                            return Err("CPU model description limit exceeded".into());
                        }
                        bytes += model.len() + 2;
                        models.insert(model);
                    }
                    if bytes == 0 {
                        unavailable += 1;
                    } else {
                        fields.push(super::super::text(
                            "cpu_model",
                            models.into_iter().collect::<Vec<_>>().join("; "),
                        ));
                    }
                }
                Err(_) => unavailable += 1,
            }
        }
        "windows" => {
            let output = ps(runner, "Windows execution capacity (Win32_ComputerSystem, Win32_Processor)", "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $cs=Get-CimInstance Win32_ComputerSystem -Property NumberOfLogicalProcessors,TotalPhysicalMemory; $cpu=Get-CimInstance Win32_Processor -Property Name; @{cpus=$cs.NumberOfLogicalProcessors; memory=$cs.TotalPhysicalMemory; model=(@($cpu.Name | Sort-Object -Unique) -join '; ')} | ConvertTo-Json -Compress").await?;
            let value = json(&output)?;
            for (field, key) in [("logical_cpus", "cpus"), ("visible_memory_bytes", "memory")] {
                if let Some(n) = value.get(key).and_then(Value::as_i64) {
                    fields.push(positive(field, &n.to_string())?);
                } else {
                    unavailable += 1;
                }
            }
            match value.get("model").and_then(Value::as_str) {
                Some(model)
                    if !model.trim().is_empty()
                        && model.trim().len() <= 512
                        && !model.trim().chars().any(char::is_control) =>
                {
                    fields.push(super::super::text("cpu_model", model.trim()))
                }
                _ => unavailable += 1,
            }
        }
        _ => return Err("Unsupported execution-capacity platform".into()),
    }
    let installation = report
        .findings
        .iter_mut()
        .find(|v| v.key == "installation")
        .ok_or("Installation finding unavailable")?;
    installation.fields.extend(fields);
    if unavailable != 0 {
        coverage(
            report,
            "execution-capacity",
            "partial",
            "Some CPU or visible-memory descriptors are unavailable; missing values are not zero or identity evidence",
        );
    } else {
        coverage(
            report,
            "execution-capacity",
            "verified",
            "OS-visible logical CPUs, memory bytes and reported CPU model; not physical capacity certification or permanent identity",
        );
    }
    Ok(())
}
