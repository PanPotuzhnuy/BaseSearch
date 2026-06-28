#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    if matches!(args.next().as_deref(), Some("--web" | "web" | "serve")) {
        let config = match parse_web_config(args, base_search::app::default_db_path()) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Base Search web argument error: {err}");
                std::process::exit(2);
            }
        };
        if let Err(err) = base_search::web::run(config) {
            eprintln!("Base Search web error: {err}");
            std::process::exit(1);
        }
        return;
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Base Search")
            .with_inner_size([1360.0, 850.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    if let Err(err) = eframe::run_native(
        "Base Search",
        options,
        Box::new(|cc| Ok(Box::new(base_search::app::App::new(cc)))),
    ) {
        eprintln!("Base Search error: {err}");
        std::process::exit(1);
    }
}

fn parse_web_config(
    args: impl IntoIterator<Item = String>,
    default_db_path: PathBuf,
) -> Result<base_search::web::WebConfig, String> {
    let mut config = base_search::web::WebConfig::new(default_db_path);
    let mut db_path_set = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => config.host = take_web_value(&mut args, "--host")?,
            "--port" => {
                let value = take_web_value(&mut args, "--port")?;
                config.port = value
                    .parse()
                    .map_err(|_| "--port must be a number from 0 to 65535".to_string())?;
            }
            "--db" => {
                if db_path_set {
                    return Err("Only one database path can be supplied".to_string());
                }
                config.db_path = take_web_value(&mut args, "--db")?.into();
                db_path_set = true;
            }
            "--token" => config.token = Some(take_web_value(&mut args, "--token")?),
            "--no-open" => config.open_browser = false,
            value if value.starts_with("--") => return Err(format!("Unknown web option: {value}")),
            value => {
                if db_path_set {
                    return Err("Only one database path can be supplied".to_string());
                }
                config.db_path = value.into();
                db_path_set = true;
            }
        }
    }
    Ok(config)
}

fn take_web_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    match args.next() {
        Some(value) if !value.starts_with("--") => Ok(value),
        _ => Err(format!("{flag} requires a value")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_web_config_accepts_explicit_options() {
        let config = parse_web_config(
            args(&[
                "custom.db",
                "--host",
                "localhost",
                "--port",
                "9000",
                "--token",
                "secret",
                "--no-open",
            ]),
            PathBuf::from("default.db"),
        )
        .unwrap();

        assert_eq!(config.db_path, PathBuf::from("custom.db"));
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 9000);
        assert_eq!(config.token.as_deref(), Some("secret"));
        assert!(!config.open_browser);
    }

    #[test]
    fn parse_web_config_rejects_missing_option_values() {
        for flag in ["--host", "--port", "--db", "--token"] {
            assert!(parse_web_config(args(&[flag]), PathBuf::from("default.db")).is_err());
        }
    }

    #[test]
    fn parse_web_config_rejects_invalid_or_unknown_options() {
        assert!(parse_web_config(args(&["--port", "nope"]), PathBuf::from("default.db")).is_err());
        assert!(parse_web_config(args(&["--unknown"]), PathBuf::from("default.db")).is_err());
    }

    #[test]
    fn parse_web_config_rejects_multiple_database_paths() {
        assert!(
            parse_web_config(args(&["one.db", "two.db"]), PathBuf::from("default.db")).is_err()
        );
    }
}
