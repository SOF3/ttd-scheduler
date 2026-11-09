use clap::Parser as _;
use ttd_scheduler as lib;

fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    options.command.unwrap_or(lib::Command::Gui(options.gui_args)).run()
}

#[derive(clap::Parser)]
struct Options {
    #[clap(subcommand)]
    command: Option<lib::Command>,

    #[cfg(feature = "gui")]
    #[clap(flatten)]
    gui_args: lib::gui::Command,
}
