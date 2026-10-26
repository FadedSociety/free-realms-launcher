//! Opt-in startup diagnostics. Never log launch arguments, tokens or form fields.
use std::{fs::File, io::Write, sync::Mutex};

struct FileLogger(Mutex<(File, usize)>);

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Debug
            && matches!(
                metadata.target().split("::").next(),
                Some("osfr_launcher" | "eframe" | "egui_glow" | "glutin" | "winit")
            )
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            if let Ok(mut output) = self.0.lock() {
                let line = format!(
                    "{} {}: {}\n",
                    record.level(),
                    record.target(),
                    record.args()
                );
                if line.len() <= output.1 && output.0.write_all(line.as_bytes()).is_ok() {
                    output.1 -= line.len();
                }
            }
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_file_stays_within_budget() {
        let path = std::env::temp_dir().join(format!("osfr-log-budget-{}.log", std::process::id()));
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let logger = FileLogger(Mutex::new((file, 64)));
        for _ in 0..10 {
            log::Log::log(
                &logger,
                &log::Record::builder()
                    .level(log::Level::Error)
                    .target("winit")
                    .args(format_args!("diagnostic"))
                    .build(),
            );
        }
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 0 && size <= 64);
        drop(logger);
        std::fs::remove_file(path).unwrap();
    }
}

pub fn init() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = std::env::var_os("OSFR_LAUNCHER_LOG") else {
        return Ok(());
    };
    let file = File::options().write(true).create_new(true).open(path)?;
    log::set_boxed_logger(Box::new(FileLogger(Mutex::new((file, 1024 * 1024)))))?;
    log::set_max_level(log::LevelFilter::Debug);
    std::panic::set_hook(Box::new(|info| {
        log::error!("Launcher panic: {info}");
        eprintln!("Launcher panic: {info}");
    }));
    log::info!("Launcher startup; renderer=glow");
    Ok(())
}
