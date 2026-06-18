#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args().skip(1);
    if matches!(args.next().as_deref(), Some("--web" | "web" | "serve")) {
        let mut config = base_search::web::WebConfig::new(base_search::app::default_db_path());
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--host" => {
                    if let Some(host) = args.next() {
                        config.host = host;
                    }
                }
                "--port" => {
                    if let Some(port) = args.next().and_then(|value| value.parse::<u16>().ok()) {
                        config.port = port;
                    }
                }
                "--db" => {
                    if let Some(path) = args.next() {
                        config.db_path = path.into();
                    }
                }
                "--no-open" => config.open_browser = false,
                _ => {}
            }
        }
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
