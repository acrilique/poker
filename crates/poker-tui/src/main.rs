use clap::Parser;

mod client;
mod tui;

#[derive(Parser)]
#[command(name = "poker")]
#[command(about = "Connect to a poker server room", long_about = None)]
struct Cli {
    /// WebSocket server URL
    #[arg(short, long, default_value = "ws://127.0.0.1:8080")]
    server: String,

    /// Room ID to create or join
    #[arg(short, long)]
    room: String,

    /// Player name
    #[arg(short, long)]
    name: String,

    /// Create the room (instead of joining an existing one)
    #[arg(short, long)]
    create: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let action = if cli.create { "Creating" } else { "Joining" };
    println!(
        "{} room '{}' on {} as '{}'...",
        action, cli.room, cli.server, cli.name
    );

    if let Err(e) = client::start_client(&cli.server, &cli.room, &cli.name, cli.create).await {
        eprintln!("Error: {}", e);
    }
}
