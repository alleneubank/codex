use clap::ArgAction;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(disable_version_flag = true)]
struct Cli {
    /// Transport endpoint: `stdio`, `stdio://`, `ws://IP:PORT`, or `grpc://IP:PORT`.
    #[arg(
        long,
        value_name = "URL",
        default_value = codex_code_mode_host::DEFAULT_LISTEN_URL
    )]
    listen: String,

    /// Print version information and exit.
    #[arg(short = 'V', long = "version", action = ArgAction::SetTrue)]
    version: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.version {
        // Emit CODEX_CLI_VERSION verbatim. Clap's own version action renders
        // "<display-name> <version>", which would prefix the binary name onto a
        // string that already carries the "codex-cli" product name, and
        // verify-fork-release-bundle.sh requires both release binaries to report
        // exactly the same version string as the CLI.
        println!("{}", env!("CODEX_CLI_VERSION"));
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    codex_code_mode_host::run_main(&cli.listen).await
}
