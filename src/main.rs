mod ui;

use gtk::glib;
use gtk::prelude::*;

pub const APP_ID: &str = "dev.kimzam.Depot";

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list-only") {
        let inv = depot_core::scan(&depot_core::Env::system());
        match serde_json::to_string_pretty(&inv) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("{e}"),
        }
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("depot {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(ui::activate);
    // our own flags are handled above; don't let GTK reject them
    app.run_with_args(&args[..1])
}
