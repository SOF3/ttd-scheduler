pub mod dispatch_flow;
#[cfg(feature = "gui")]
pub mod gui;
pub mod schema;

#[derive(clap::Subcommand)]
pub enum Command {
    DispatchFlow(dispatch_flow::Command),
    #[cfg(feature = "gui")]
    Gui(gui::Command),
}

impl Command {
    pub fn run(&self) -> anyhow::Result<()> {
        match self {
            Self::DispatchFlow(command) => command.run(),
            #[cfg(feature = "gui")]
            Self::Gui(command) => command.run(),
        }
    }
}
