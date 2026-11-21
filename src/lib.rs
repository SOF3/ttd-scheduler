pub mod dispatch_flow;
#[cfg(feature = "gui")]
pub mod gui;
pub mod schema;

#[cfg(not(target_arch = "wasm32"))]
#[derive(clap::Subcommand)]
pub enum Command {
    DispatchFlow(dispatch_flow::Command),
    #[cfg(feature = "gui")]
    Gui(gui::Command),
}

#[cfg(not(target_arch = "wasm32"))]
impl Command {
    pub fn run(&self) -> anyhow::Result<()> {
        match self {
            Self::DispatchFlow(command) => command.run(),
            #[cfg(feature = "gui")]
            Self::Gui(command) => command.run(),
        }
    }
}
