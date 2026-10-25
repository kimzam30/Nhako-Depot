mod packagekit;
mod system;
mod ui;
mod updates;

use gtk::glib;
use gtk::prelude::*;

pub const APP_ID: &str = "dev.kimzam.Depot";

const HELP: &str = "Usage: depot [OPTION]

  --list-only               Print everything installed as JSON and exit
  --updates [--refresh]     Print available updates as JSON and exit
  --simulate-remove NAME    Show what removing a system package would remove (changes nothing)
  --simulate-update         Show what installing all system updates would touch (changes nothing)
  --unused-runtimes         Print the Flatpak runtimes no app needs any more as JSON (changes nothing)
  --version                 Print the version";

fn print_json(v: &impl serde::Serialize) {
    match serde_json::to_string_pretty(v) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("{e}"),
    }
}

/// Diagnostics that run without a window. Returns `None` when the GUI should start.
fn headless(args: &[String]) -> Option<glib::ExitCode> {
    let flag = args.get(1)?.as_str();
    let ctx = glib::MainContext::default();
    match flag {
        "--list-only" => print_json(&depot_core::scan(&depot_core::Env::system())),
        "--unused-runtimes" => {
            let inv = depot_core::scan(&depot_core::Env::system());
            let unused: Vec<&depot_core::Package> = inv.unused_runtimes.iter().map(|&i| &inv.packages[i]).collect();
            print_json(&unused);
        }
        "--updates" => {
            let inv = depot_core::scan(&depot_core::Env::system());
            let refresh = args.iter().any(|a| a == "--refresh");
            print_json(&ctx.block_on(updates::check(refresh, &inv.packages)));
        }
        "--simulate-remove" => {
            let Some(name) = args.get(2) else {
                eprintln!("--simulate-remove needs a package name");
                return Some(glib::ExitCode::FAILURE);
            };
            let r = ctx.block_on(async {
                let ids = packagekit::resolve_installed(std::slice::from_ref(name)).await?;
                let ids: Vec<String> = ids.into_iter().map(|p| p.id).collect();
                packagekit::simulate_remove(&ids, true).await
            });
            match r {
                Ok(pk) => pk.iter().for_each(|p| println!("{:>2} {}", p.info, p.id)),
                Err(e) => {
                    eprintln!("error {}: {e}", e.code);
                    return Some(glib::ExitCode::FAILURE);
                }
            }
        }
        "--simulate-update" => {
            let r = ctx.block_on(async {
                let ids: Vec<String> = packagekit::get_updates().await?.into_iter().map(|p| p.id).collect();
                packagekit::simulate_update(&ids).await
            });
            match r {
                Ok(pk) => pk.iter().for_each(|p| println!("{:>2} {}", p.info, p.id)),
                Err(e) => {
                    eprintln!("error {}: {e}", e.code);
                    return Some(glib::ExitCode::FAILURE);
                }
            }
        }
        "--version" => println!("depot {}", env!("CARGO_PKG_VERSION")),
        "--help" | "-h" => println!("{HELP}"),
        _ => return None,
    }
    Some(glib::ExitCode::SUCCESS)
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = headless(&args) {
        return code;
    }
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(ui::activate);
    // our own flags are handled above; don't let GTK reject them
    app.run_with_args(&args[..1])
}
