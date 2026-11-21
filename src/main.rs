use clap::Parser as _;
use ttd_scheduler as lib;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    env_logger::init();
    let options = Options::parse();
    options.command.unwrap_or(lib::Command::Gui(options.gui_args)).run()
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(clap::Parser)]
struct Options {
    #[clap(subcommand)]
    command: Option<lib::Command>,

    #[cfg(feature = "gui")]
    #[clap(flatten)]
    gui_args: lib::gui::Command,
}

#[cfg(target_arch = "wasm32")]
fn main() { unimplemented!("use bin/web instead") }
